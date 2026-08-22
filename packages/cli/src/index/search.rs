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
    // approach in resolve_page at L246-L252.
    {
        let mut stmt = conn.prepare(
            "SELECT b.oid, b.title, b.summary, p.path_rel, b.aliases_text
             FROM blobs b
             JOIN paths p ON p.oid = b.oid AND p.source = ?1
             WHERE b.aliases_text != ''",
        )?;
        let rows = stmt.query_map(params![src], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?;
        for row in rows {
            let (oid, title, summary, path, aliases) = row?;
            let needle = q_lower.as_str();
            for a in aliases.split(|c: char| c.is_whitespace() || c == ',') {
                let a = a.trim();
                if !a.is_empty() && a.eq_ignore_ascii_case(needle) {
                    if seen.insert(oid) {
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

    // Alias match: aliases_text is whitespace-joined. The broad SELECT
    // intentionally omits b.body — body text is fetched only for the
    // matching row below, avoiding bulk IO when iterating the corpus.
    let mut stmt = conn.prepare(
        "SELECT b.rowid, b.title, b.summary, b.aliases_text, p.path_rel
         FROM blobs b
         JOIN paths p ON p.oid = b.oid AND p.source = ?1",
    )?;
    let rows = stmt.query_map(params![src], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
        ))
    })?;
    for row in rows {
        let (rowid, title, summary, aliases, path_rel) = row?;
        let needle = q_lower.as_str();
        let mut matched: Option<String> = None;
        for a in aliases.split(|c: char| c.is_whitespace() || c == ',') {
            let a = a.trim();
            if !a.is_empty() && a.eq_ignore_ascii_case(needle) {
                matched = Some(a.to_string());
                break;
            }
        }
        if matched.is_some() {
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
                alias: matched,
                document_id: rowid,
            }));
        }
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
