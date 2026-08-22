//! `bm25(fts, 5, 4, 3, 3, 2, 1)` query over the wiki search index, with an
//! exact-match short-circuit on title/alias/path before the FTS scan.

use std::collections::HashSet;

use std::path::Path;

use rusqlite::{Connection, params, params_from_iter};

use crate::index::{DocSource, ResolvedPage, SearchResult, Snippet};

/// Numeric discriminator stored in `paths.source` matching
/// [`crate::index::passes::source_id`].
pub(crate) fn source_filter_id(source: DocSource) -> i64 {
    match source {
        DocSource::Head => 0,
        DocSource::Index => 1,
        DocSource::WorkingTree => 2,
    }
}

/// OIDs per anti-join chunk when correcting the FTS total for pre-FTS
/// matches. SQLite's default `SQLITE_MAX_VARIABLE_NUMBER` is 32766 binds
/// per statement; 900 keeps every chunk far below any limit while a
/// realistic corpus needs only one statement.
const TOTAL_COUNT_CHUNK: usize = 900;

/// Build an FTS5 MATCH expression from a free-form user query.
///
/// Splits on whitespace, strips/escapes FTS5 syntax characters
/// (`"`, `:`, `(`, `)`), and appends a `*` to every token so prefix
/// matches resolve via the `prefix='2 3 4'` index. Multiple tokens are
/// AND-joined (FTS5's default for space separation).
pub(crate) fn make_fts_query(q: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    for raw in q.split_whitespace() {
        let cleaned: String = raw
            .chars()
            .filter(|c| !matches!(c, '"' | ':' | '(' | ')'))
            .collect();
        if cleaned.is_empty() {
            continue;
        }
        // Quote the bareword to defang remaining FTS5 operators (e.g. `-`,
        // `AND`, `NOT`, `*` inside the body), then append `*` for prefix
        // matching. The quoted form `"tok"*` is a valid FTS5 prefix query.
        parts.push(format!("\"{}\"*", cleaned));
    }
    parts.join(" ")
}

/// Build the column-scoped FTS5 phrase query `aliases_text:"<token>"` used
/// as a PREFILTER for the exact alias comparisons (`search_weighted` stage
/// 1b and `resolve_page`). The `fts` table indexes `blobs.aliases_text`
/// verbatim through the same `unicode61 remove_diacritics 2` tokenizer that will
/// tokenize the quoted phrase, so whenever an aliases_text token equals the
/// needle under ASCII-case-insensitive equality its indexed token sequence
/// occurs contiguously in the column — the phrase always retrieves it. The
/// prefilter may therefore over-approximate (extra rows are dropped by the
/// client-side exact compare) but can never miss. `None` when no safe phrase
/// can be built (an empty token matches nothing and cannot be quoted) —
/// callers fall back to their unfiltered scan.
///
/// Unlike [`make_fts_query`] (which strips quotes for bareword queries),
/// this keeps content intact by escaping per FTS5 string syntax: a double
/// quote inside the string is escaped by doubling it.
fn fts_alias_match(token: &str) -> Option<String> {
    if token.is_empty() {
        return None;
    }
    Some(format!(
        "aliases_text:\"{}\"",
        token.replace('"', "\"\"")
    ))
}

/// Run a weighted search. Returns up to `limit` rows starting at `offset`,
/// alongside the (uncapped) total match count.
pub fn search_weighted(
    conn: &Connection,
    source: DocSource,
    query: &str,
    limit: usize,
    offset: usize,
) -> rusqlite::Result<(Vec<SearchResult>, usize)> {
    let src = source_filter_id(source);
    let mut seen: HashSet<String> = HashSet::new(); // dedupe by blob OID
    let mut out: Vec<SearchResult> = Vec::new();

    // (1) Exact title / token-wise alias match (case-insensitive), filtered to
    // the requested DocSource.
    let q_lower = query.to_lowercase();
    {
        let mut stmt = conn.prepare(
            "SELECT b.oid, b.title, b.summary, p.path_rel
             FROM blobs b
             JOIN paths p ON p.oid = b.oid AND p.source = ?1
             WHERE lower(b.title) = ?2",
        )?;
        let rows = stmt.query_map(params![src, q_lower], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        for row in rows {
            let (oid, title, summary, path) = row?;
            if seen.insert(oid) {
                out.push(SearchResult {
                    title,
                    file: path,
                    summary,
                    alias: None,
                    snippets: Vec::new(),
                });
            }
        }
    }

    // (1b) Token-wise alias match (case-insensitive): split aliases_text on
    // whitespace/comma and compare each token individually, replicating the
    // approach in resolve_page at L246-L252. Candidate rows are narrowed
    // through the FTS aliases column first (P5): `aliases_text:"<query>"` is an
    // over-approximation of the exact token equality below (same tokenizer
    // folds indexed text and phrase identically — see `fts_alias_match`), so
    // streaming only FTS hits keeps the per-row `eq_ignore_ascii_case`
    // verdict byte-identical while skipping every corpus row that could not
    // match anyway. When no safe MATCH phrase exists (empty query) the
    // previous full scan runs unchanged.
    {
        let needle = q_lower.as_str();
        match fts_alias_match(needle) {
            Some(alias_expr) => {
                let mut stmt = conn.prepare(
                    "SELECT b.oid, b.title, b.summary, p.path_rel, b.aliases_text
                     FROM blobs b
                     JOIN paths p ON p.oid = b.oid AND p.source = ?1
                     JOIN fts ON fts.rowid = b.rowid
                     WHERE fts MATCH ?2
                     ORDER BY b.rowid",
                )?;
                let rows = stmt.query_map(params![src, alias_expr], map_alias_candidate_row)?;
                collect_exact_alias_hits(rows, needle, &mut seen, &mut out)?;
            }
            None => {
                let mut stmt = conn.prepare(
                    "SELECT b.oid, b.title, b.summary, p.path_rel, b.aliases_text
                     FROM blobs b
                     JOIN paths p ON p.oid = b.oid AND p.source = ?1
                     WHERE b.aliases_text != ''
                     ORDER BY b.rowid",
                )?;
                let rows = stmt.query_map(params![src], map_alias_candidate_row)?;
                collect_exact_alias_hits(rows, needle, &mut seen, &mut out)?;
            }
        }
    }

    // (2) Path-fragment LIKE (only when the query smells like a path).
    if query.contains('/') {
        let pat = format!("%{}%", query);
        let mut stmt = conn.prepare(
            "SELECT b.oid, b.title, b.summary, p.path_rel
             FROM paths p JOIN blobs b ON b.oid = p.oid
             WHERE p.path_rel LIKE ?1 AND p.source = ?2",
        )?;
        let rows = stmt.query_map(params![pat, src], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        for row in rows {
            let (oid, title, summary, path) = row?;
            if seen.insert(oid) {
                out.push(SearchResult {
                    title,
                    file: path,
                    summary,
                    alias: None,
                    snippets: Vec::new(),
                });
            }
        }
    }

    // Snapshot phase 1-2 OIDs before the FTS LIMIT-cap iteration so we
    // can compute the true uncapped total below.
    let pre_fts_seen: Vec<String> = seen.iter().cloned().collect();

    // (3) BM25-weighted FTS scan with the field weight tuple from CARD.md:
    // (title:5, aliases:4, tags:3, keywords:3, summary:2, body:1). FTS5
    // bm25() returns negative scores; smaller (more negative) = better, so
    // ORDER BY ... ASC puts the best matches first. snippet() column index
    // 5 = `body`.
    let fts_query = make_fts_query(query);
    if !fts_query.is_empty() {
        // SQL-side LIMIT keeps the FTS5 + bm25 + JOIN pipeline from
        // materializing every match: a common-token query against a 5k-doc
        // corpus returned ~5000 rows before the client-side `take(limit)`
        // capped them, costing ~3s warm. The cap below leaves headroom for
        // dedup across the exact / path / fts stages above without bloating
        // the row set.
        let cap = limit.saturating_add(offset).saturating_add(64) as i64;
        let mut stmt = conn.prepare(
            "SELECT b.oid, b.title, p.path_rel, b.summary,
                    snippet(fts, 5, '', '', '…', 24) AS snip
             FROM fts
             JOIN blobs b ON b.rowid = fts.rowid
             JOIN paths p ON p.oid   = b.oid AND p.source = ?2
             WHERE fts MATCH ?1
             ORDER BY bm25(fts, 5, 4, 3, 3, 2, 1) ASC
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![fts_query, src, cap], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?;
        for row in rows {
            let (oid, title, path, summary, snip): (String, String, String, String, String) = row?;
            if seen.insert(oid) {
                let snippets = if snip.is_empty() {
                    Vec::new()
                } else {
                    vec![Snippet {
                        line: 0,
                        text: snip,
                    }]
                };
                out.push(SearchResult {
                    title,
                    file: path,
                    summary,
                    alias: None,
                    snippets,
                });
            }
        }
    }

    // Compute the true uncapped total match count.
    // The FTS query above uses a performance LIMIT cap, but callers
    // (e.g. commands/search.rs) expect the real total.
    let total = if !fts_query.is_empty() {
        let true_fts_total: i64 = conn.query_row(
            "SELECT COUNT(*)
             FROM fts
             JOIN blobs b ON b.rowid = fts.rowid
             JOIN paths p ON p.oid = b.oid AND p.source = ?2
             WHERE fts MATCH ?1",
            params![fts_query, src],
            |r| r.get(0),
        )?;

        if pre_fts_seen.is_empty() {
            true_fts_total as usize
        } else {
            // Pre-FTS matches (exact title / alias / path) may also match
            // the FTS query. Count, in one anti-join per chunk of OIDs,
            // those that do NOT satisfy the MATCH expression and add them
            // to the FTS total. `blobs.oid` is a PRIMARY KEY, so the old
            // per-OID probe and this set-based form answer the identical
            // question — but each chunk evaluates MATCH once instead of
            // once per document.
            let extra = crate::perf::scope_result(
                "search.total_count",
                serde_json::json!({ "pre_matches": pre_fts_seen.len() }),
                || -> rusqlite::Result<usize> {
                    let mut extra: usize = 0;
                    for chunk in pre_fts_seen.chunks(TOTAL_COUNT_CHUNK) {
                        let placeholders = vec!["?"; chunk.len()].join(", ");
                        let sql = format!(
                            "SELECT COUNT(*) FROM blobs b \
                             WHERE b.oid IN ({placeholders}) \
                             AND NOT EXISTS (SELECT 1 FROM fts \
                             WHERE fts.rowid = b.rowid AND fts MATCH ?{})",
                            chunk.len() + 1
                        );
                        let bound = params_from_iter(
                            chunk.iter().chain(std::iter::once(&fts_query)),
                        );
                        let not_matching: i64 =
                            conn.query_row(sql.as_str(), bound, |r| r.get(0))?;
                        extra += not_matching as usize;
                    }
                    Ok(extra)
                },
            )?;
            (true_fts_total as usize) + extra
        }
    } else {
        out.len()
    };
    let paged: Vec<SearchResult> = out.into_iter().skip(offset).take(limit).collect();
    Ok((paged, total))
}

/// Row mapper shared by both stage-1b branches (FTS-prefiltered and fallback
/// scan): `(oid, title, summary, path_rel, aliases_text)`.
fn map_alias_candidate_row(
    r: &rusqlite::Row<'_>,
) -> rusqlite::Result<(String, String, String, String, String)> {
    Ok((
        r.get::<_, String>(0)?,
        r.get::<_, String>(1)?,
        r.get::<_, String>(2)?,
        r.get::<_, String>(3)?,
        r.get::<_, String>(4)?,
    ))
}

/// The exact half of search stage 1b, unchanged by the P5 prefilter: keep a
/// row when any whitespace/comma-delimited token of its `aliases_text`
/// equals `needle` under `eq_ignore_ascii_case`, deduped by blob OID.
fn collect_exact_alias_hits(
    rows: impl Iterator<Item = rusqlite::Result<(String, String, String, String, String)>>,
    needle: &str,
    seen: &mut HashSet<String>,
    out: &mut Vec<SearchResult>,
) -> rusqlite::Result<()> {
    for row in rows {
        let (oid, title, summary, path, aliases) = row?;
        for a in aliases.split(|c: char| c.is_whitespace() || c == ',') {
            let a = a.trim();
            if !a.is_empty() && a.eq_ignore_ascii_case(needle) {
                if seen.insert(oid.clone()) {
                    out.push(SearchResult {
                        title,
                        file: path,
                        summary,
                        alias: None,
                        snippets: Vec::new(),
                    });
                }
                break;
            }
        }
    }
    Ok(())
}

/// Resolve a single page by title, alias, repo-relative path, or `.md` link.
///
/// Returns the first match, prioritizing exact title/alias hits, then path
/// fragment lookups. The returned `ResolvedPage` carries the *absolute* file
/// path (joined against `repo_root`) so callers can `strip_prefix`.
pub fn resolve_page(
    conn: &Connection,
    repo_root: &Path,
    source: DocSource,
    input: &str,
) -> rusqlite::Result<Option<ResolvedPage>> {
    let src = source_filter_id(source);
    let q_lower = input.to_lowercase();

    // Exact title match.
    let mut stmt = conn.prepare(
        "SELECT b.rowid, b.title, b.summary, b.body, p.path_rel
         FROM blobs b
         JOIN paths p ON p.oid = b.oid AND p.source = ?2
         WHERE lower(b.title) = ?1
         ORDER BY b.rowid
         LIMIT 1",
    )?;
    let row: Option<(i64, String, String, String, String)> = stmt
        .query_row(params![q_lower, src], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })
        .ok();
    if let Some((rowid, title, summary, body, path_rel)) = row {
        return Ok(Some(ResolvedPage {
            title,
            file: repo_root.join(&path_rel).to_string_lossy().into_owned(),
            summary,
            content: body,
            alias: None,
            document_id: rowid,
        }));
    }

    // Alias match: aliases_text is whitespace-joined. Candidate rows are
    // narrowed through the FTS aliases column (P5) exactly as in
    // `search_weighted` stage 1b — `aliases_text:"<input>"` over-approximates the
    // exact token equality (`fts_alias_match`), and the surviving rows run
    // the same per-token compare, so the first hit under the old full scan's
    // rowid order is still the first hit here. When no safe MATCH phrase can
    // be built, the unfiltered scan runs unchanged. The SELECTs intentionally
    // omit b.body — body text is fetched only for the matching row below,
    // avoiding bulk IO when iterating candidates.
    let alias_hit: Option<AliasHit> = match fts_alias_match(&q_lower) {
        Some(alias_expr) => {
            let mut stmt = conn.prepare(
                "SELECT b.rowid, b.title, b.summary, b.aliases_text, p.path_rel
                 FROM blobs b
                 JOIN paths p ON p.oid = b.oid AND p.source = ?1
                 JOIN fts ON fts.rowid = b.rowid
                 WHERE fts MATCH ?2
                 ORDER BY b.rowid",
            )?;
            let rows = stmt.query_map(params![src, alias_expr], map_resolve_alias_row)?;
            first_exact_alias_hit(rows, &q_lower)?
        }
        None => {
            let mut stmt = conn.prepare(
                "SELECT b.rowid, b.title, b.summary, b.aliases_text, p.path_rel
                 FROM blobs b
                 JOIN paths p ON p.oid = b.oid AND p.source = ?1
                 ORDER BY b.rowid",
            )?;
            let rows = stmt.query_map(params![src], map_resolve_alias_row)?;
            first_exact_alias_hit(rows, &q_lower)?
        }
    };
    if let Some((rowid, title, summary, matched, path_rel)) = alias_hit {
        let body: String = conn.query_row(
            "SELECT b.body FROM blobs b WHERE b.rowid = ?1",
            params![rowid],
            |r| r.get(0),
        )?;
        return Ok(Some(ResolvedPage {
            title,
            file: repo_root.join(&path_rel).to_string_lossy().into_owned(),
            summary,
            content: body,
            alias: Some(matched),
            document_id: rowid,
        }));
    }

    // Path lookup.
    if input.contains('/') || input.ends_with(".md") {
        let mut stmt = conn.prepare(
            "SELECT b.rowid, b.title, b.summary, b.body, p.path_rel
             FROM paths p JOIN blobs b ON b.oid = p.oid
             WHERE (p.path_rel = ?1 OR instr(p.path_rel, ?2) > 0) AND p.source = ?3
             ORDER BY p.path_rel
             LIMIT 1",
        )?;
        let row: Option<(i64, String, String, String, String)> = stmt
            .query_row(params![input, input, src], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                ))
            })
            .ok();
        if let Some((rowid, title, summary, body, path_rel)) = row {
            return Ok(Some(ResolvedPage {
                title,
                file: repo_root.join(&path_rel).to_string_lossy().into_owned(),
                summary,
                content: body,
                alias: None,
                document_id: rowid,
            }));
        }
    }

    Ok(None)
}

/// Row mapper shared by both `resolve_page` alias-stage branches:
/// `(rowid, title, summary, aliases_text, path_rel)`.
fn map_resolve_alias_row(
    r: &rusqlite::Row<'_>,
) -> rusqlite::Result<(i64, String, String, String, String)> {
    Ok((
        r.get::<_, i64>(0)?,
        r.get::<_, String>(1)?,
        r.get::<_, String>(2)?,
        r.get::<_, String>(3)?,
        r.get::<_, String>(4)?,
    ))
}

/// One resolved alias-stage candidate: `(rowid, title, summary, matched
/// alias text, path_rel)`.
type AliasHit = (i64, String, String, String, String);

/// The exact half of the alias stage, unchanged by the P5 prefilter: return
/// the first row (in rowid order) with an aliases_text token equal to
/// `needle` under `eq_ignore_ascii_case`, paired with the matched alias
/// text.
fn first_exact_alias_hit(
    rows: impl Iterator<Item = rusqlite::Result<(i64, String, String, String, String)>>,
    needle: &str,
) -> rusqlite::Result<Option<AliasHit>> {
    for row in rows {
        let (rowid, title, summary, aliases, path_rel) = row?;
        let mut matched: Option<String> = None;
        for a in aliases.split(|c: char| c.is_whitespace() || c == ',') {
            let a = a.trim();
            if !a.is_empty() && a.eq_ignore_ascii_case(needle) {
                matched = Some(a.to_string());
                break;
            }
        }
        if let Some(alias) = matched {
            return Ok(Some((rowid, title, summary, alias, path_rel)));
        }
    }
    Ok(None)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::schema;

    /// In-memory index DB with the real schema (the FTS triggers populate
    /// the virtual table as rows are inserted).
    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        schema::bootstrap(&conn).expect("bootstrap schema");
        conn
    }

    fn insert_page(
        conn: &Connection,
        oid: &str,
        title: &str,
        aliases: &str,
        body: &str,
        path_rel: &str,
        source: i64,
    ) {
        conn.execute(
            "INSERT INTO blobs (oid, refcount, title, summary, body, aliases_text, tags_text, keywords_text)
             VALUES (?1, 1, ?2, 'A summary.', ?3, ?4, '', '')",
            params![oid, title, body, aliases],
        )
        .expect("insert blob");
        conn.execute(
            "INSERT INTO paths (path_rel, source, oid, stat_mtime_ns, stat_size, stat_ctime_ns, parent_dir)
             VALUES (?1, ?2, ?3, NULL, NULL, NULL, '')",
            params![path_rel, source, oid],
        )
        .expect("insert path");
    }

    fn files(results: &[SearchResult]) -> Vec<&str> {
        results.iter().map(|r| r.file.as_str()).collect()
    }

    // ── fts_alias_match: safe phrase construction ──

    #[test]
    fn fts_alias_match_escapes_double_quotes() {
        assert_eq!(
            fts_alias_match("ven\"ture").as_deref(),
            Some("aliases_text:\"ven\"\"ture\"")
        );
        assert_eq!(
            fts_alias_match("venture").as_deref(),
            Some("aliases_text:\"venture\"")
        );
        assert_eq!(fts_alias_match(""), None, "an empty token builds no phrase");
    }

    // ── P5: alias stages stream only FTS-prefiltered rows ──

    #[test]
    fn search_stage_1b_finds_alias_token_through_fts_prefilter() {
        let conn = test_conn();
        insert_page(
            &conn,
            "o1",
            "Venture Page",
            "Quixotic Venture",
            "body without the query word",
            "docs/v.md",
            source_filter_id(DocSource::WorkingTree),
        );
        insert_page(
            &conn,
            "o2",
            "Other Page",
            "",
            "filler",
            "docs/other.md",
            source_filter_id(DocSource::WorkingTree),
        );

        // Lowercase query against the mixed-case alias — the exact
        // `eq_ignore_ascii_case` verdict must survive the prefilter.
        let (results, total) =
            search_weighted(&conn, DocSource::WorkingTree, "venture", 10, 0).expect("search");
        assert_eq!(files(&results), vec!["docs/v.md"]);
        assert_eq!(total, 1);
    }

    /// The stage-1b exact compare gates the prefilter's over-approximation:
    /// a row whose aliases_text satisfies the FTS phrase (`e-mail` → token
    /// pair [e, mail], which the space-separated form also forms) but fails
    /// the token-wise `eq_ignore_ascii_case` verdict is dropped here. The
    /// dropped row may still surface later through the independent stage-3
    /// FTS scan (pre-existing behavior, unchanged) — this asserts the stage-
    /// 1b contribution only: the real alias hit leads the result list.
    #[test]
    fn search_stage_1b_exact_compare_gates_over_approximated_prefilter_hits() {
        let conn = test_conn();
        insert_page(
            &conn,
            "o1",
            "Mail Page",
            "e-mail",
            "no query words here",
            "docs/mail.md",
            source_filter_id(DocSource::WorkingTree),
        );
        insert_page(
            &conn,
            "o2",
            "Decoy",
            "e mail",
            "also no query words",
            "docs/decoy.md",
            source_filter_id(DocSource::WorkingTree),
        );

        let (results, _) =
            search_weighted(&conn, DocSource::WorkingTree, "e-mail", 10, 0).expect("search");
        assert_eq!(
            results.first().expect("alias hit").file,
            "docs/mail.md",
            "the exact alias hit must lead; got {:?}",
            files(&results)
        );
    }

    /// The same gate, isolated from every other stage: fed candidate rows in
    /// which the over-approximated row comes first, the exact compare must
    /// skip it and keep only the true alias token equality.
    #[test]
    fn collect_exact_alias_hits_drops_phrase_only_rows() {
        let rows = [
            Ok(("o-decoy".into(), "Decoy".into(), "s".into(), "d.md".into(), "e mail".into())),
            Ok(("o-real".into(), "Mail".into(), "s".into(), "m.md".into(), "e-mail".into())),
        ];
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        collect_exact_alias_hits(rows.into_iter(), "e-mail", &mut seen, &mut out)
            .expect("collect");
        assert_eq!(files(&out), vec!["m.md"], "phrase-only row must be dropped");
    }

    #[test]
    fn search_stage_1b_handles_quoted_alias_tokens() {
        let conn = test_conn();
        insert_page(
            &conn,
            "o1",
            "Quote Page",
            "say\"hi",
            "nothing relevant",
            "docs/quoted.md",
            source_filter_id(DocSource::WorkingTree),
        );

        let (results, _) =
            search_weighted(&conn, DocSource::WorkingTree, "say\"hi", 10, 0).expect("search");
        assert_eq!(files(&results), vec!["docs/quoted.md"]);
    }

    #[test]
    fn resolve_page_alias_match_streams_prefiltered_rows_only() {
        let conn = test_conn();
        let src = source_filter_id(DocSource::WorkingTree);
        insert_page(&conn, "o1", "Guide", "", "plain body", "docs/g.md", src);
        insert_page(
            &conn,
            "o2",
            "Alias Target",
            "Handbook HB",
            "the resolved content",
            "docs/h.md",
            src,
        );

        let repo = Path::new("/repo");
        let page = resolve_page(&conn, repo, DocSource::WorkingTree, "hb").expect("resolve");
        let page = page.expect("alias hit");
        assert_eq!(page.title, "Alias Target");
        assert_eq!(page.alias.as_deref(), Some("HB"));
        assert_eq!(page.content, "the resolved content");
        assert_eq!(page.file, "/repo/docs/h.md");

        // No alias anywhere → falls through to Ok(None) after title/path miss.
        let miss =
            resolve_page(&conn, repo, DocSource::WorkingTree, "nonexistent").expect("resolve");
        assert!(miss.is_none());
    }

    // ── P5: aggregate total count equals the per-OID loop ──

    /// Hand-computed expectation for the uncapped total. Query `docs/zeta`
    /// compiles to the single phrase-prefix `"docs/zeta"*` (adjacent token
    /// pair) and drives stage 2 (path LIKE `%docs/zeta%`) so pre-FTS OIDs
    /// exist, while the FTS expression matches an overlapping but different
    /// set:
    ///
    /// - zeta.md      : pre-FTS hit (path), NOT in FTS (no docs–zeta pair)
    ///   → extra += 1
    /// - advanced.md  : pre-FTS hit (path, `docs/zeta-advanced.md`) AND FTS
    ///   hit (`docs zeta` adjacent in body) → extra += 0
    /// - combined.md  : FTS hit only (`docs zeta` adjacent; its path lacks
    ///   the literal `docs/zeta` substring)
    /// - intro.md     : in neither set
    ///
    /// Expected total = true FTS total (2) + non-matching pre-FTS OIDs (1).
    #[test]
    fn search_total_matches_hand_computed_expectation() {
        let conn = test_conn();
        let src = source_filter_id(DocSource::WorkingTree);
        insert_page(&conn, "o1", "Intro", "", "alpha beta gamma", "docs/intro.md", src);
        insert_page(
            &conn,
            "o2",
            "Zeta Page",
            "",
            "unrelated words entirely",
            "docs/zeta.md",
            src,
        );
        insert_page(
            &conn,
            "o3",
            "Combined",
            "",
            "read our docs zeta coverage now",
            "notes/combined.md",
            src,
        );
        insert_page(
            &conn,
            "o4",
            "Advanced Zeta",
            "",
            "covers docs zeta topics deeply",
            "docs/zeta-advanced.md",
            src,
        );

        let (_, total) =
            search_weighted(&conn, DocSource::WorkingTree, "docs/zeta", 10, 0).expect("search");
        assert_eq!(total, 3, "2 FTS hits + 1 non-FTS pre-stage OID");
    }

    /// Chunking: 600 pre-FTS OIDs force three chunked COUNT queries. 200 of
    /// the docs match the FTS expression (`bulk item payload` bodies carry
    /// both query words), 400 do not; every doc is a stage-2 pre-FTS hit via
    /// its `bulk/item…` path. Expected total = 200 + 400 = 600.
    #[test]
    fn search_total_chunking_six_hundred_pre_fts_oids() {
        let conn = test_conn();
        let src = source_filter_id(DocSource::WorkingTree);
        for i in 0..600 {
            let matching = i % 3 == 0;
            let body = if matching { "bulk item payload" } else { "filler text" };
            insert_page(
                &conn,
                &format!("bulk-o{i}"),
                &format!("Entry {i}"),
                "",
                body,
                &format!("bulk/item{i:03}.md"),
                src,
            );
        }

        let (_, total) =
            search_weighted(&conn, DocSource::WorkingTree, "bulk/item", 10, 0).expect("search");
        assert_eq!(total, 600, "200 FTS-matching + 400 non-matching pre-stage OIDs");

        let (paged, _) =
            search_weighted(&conn, DocSource::WorkingTree, "bulk/item", 7, 5).expect("paged");
        assert_eq!(paged.len(), 7, "paging still applies to the result window");
    }
}
