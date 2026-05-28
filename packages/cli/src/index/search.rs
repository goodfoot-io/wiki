//! `bm25(fts, 5, 4, 3, 3, 2, 1)` query over the wiki search index, with an
//! exact-match short-circuit on title/alias/path before the FTS scan.

use std::collections::HashSet;

use std::path::Path;

use rusqlite::{Connection, params};

use crate::index::{DocSource, ResolvedPage, SearchResult, Snippet};

/// Numeric discriminator stored in `paths.source` matching
/// [`crate::index::passes::source_id`].
fn source_filter_id(source: DocSource) -> i64 {
    match source {
        DocSource::Head => 0,
        DocSource::Index => 1,
        DocSource::WorkingTree => 2,
    }
}

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

    // (1) Exact title / full-alias match (case-insensitive), filtered to
    // the requested DocSource.
    let q_lower = query.to_lowercase();
    {
        let mut stmt = conn.prepare(
            "SELECT b.oid, b.title, b.summary, p.path_rel
             FROM blobs b
             JOIN paths p ON p.oid = b.oid AND p.source = ?2
             WHERE lower(b.title) = ?1
                OR lower(b.aliases_text) = ?1",
        )?;
        let rows = stmt.query_map(params![q_lower, src], |r| {
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

    // (3) BM25-weighted FTS scan with the field weight tuple from CARD.md:
    // (title:5, aliases:4, tags:3, keywords:3, summary:2, body:1). FTS5
    // bm25() returns negative scores; smaller (more negative) = better, so
    // ORDER BY ... ASC puts the best matches first. snippet() column index
    // 5 = `body`.
    let fts_query = make_fts_query(query);
    if !fts_query.is_empty() {
        let mut stmt = conn.prepare(
            "SELECT b.oid, b.title, p.path_rel, b.summary,
                    snippet(fts, 5, '', '', '…', 24) AS snip
             FROM fts
             JOIN blobs b ON b.rowid = fts.rowid
             JOIN paths p ON p.oid   = b.oid AND p.source = ?2
             WHERE fts MATCH ?1
             ORDER BY bm25(fts, 5, 4, 3, 3, 2, 1) ASC",
        )?;
        let rows = stmt.query_map(params![fts_query, src], |r| {
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
                    vec![Snippet { line: 0, text: snip }]
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

    let total = out.len();
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

    // Alias match: aliases_text is whitespace-joined.
    let mut stmt = conn.prepare(
        "SELECT b.rowid, b.title, b.summary, b.body, b.aliases_text, p.path_rel
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
            r.get::<_, String>(5)?,
        ))
    })?;
    for row in rows {
        let (rowid, title, summary, body, aliases, path_rel) = row?;
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
             WHERE (p.path_rel = ?1 OR p.path_rel LIKE ?2) AND p.source = ?3
             LIMIT 1",
        )?;
        let pat = format!("%{input}%");
        let row: Option<(i64, String, String, String, String)> = stmt
            .query_row(params![input, pat, src], |r| {
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

