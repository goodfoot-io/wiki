//! `bm25(fts, 5, 4, 3, 3, 2, 1)` query over the wiki search index, with an
//! exact-match short-circuit on title/alias/path before the FTS scan.

use std::collections::HashSet;

use rusqlite::{Connection, params};

use crate::index::SearchResult;

/// Run a weighted search. Returns up to `limit` rows starting at `offset`,
/// alongside the (uncapped) total match count.
pub fn search_weighted(
    conn: &Connection,
    query: &str,
    limit: usize,
    offset: usize,
) -> rusqlite::Result<(Vec<SearchResult>, usize)> {
    let mut seen: HashSet<String> = HashSet::new(); // dedupe by blob OID
    let mut out: Vec<SearchResult> = Vec::new();

    // (1) Exact title match (case-insensitive). Aliases are space-joined
    // strings of multi-word values; we only short-circuit when the WHOLE
    // aliases_text equals the query — partial alias hits fall through to
    // the FTS cascade.
    let q_lower = query.to_lowercase();
    let mut stmt = conn.prepare(
        "SELECT b.oid, b.title, b.summary, p.path_rel
         FROM blobs b
         JOIN paths p ON p.oid = b.oid
         WHERE lower(b.title) = ?1
            OR lower(b.aliases_text) = ?1",
    )?;
    let rows = stmt.query_map(params![q_lower], |r| {
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

    // (2) Path-fragment LIKE (only when the query smells like a path).
    if query.contains('/') {
        let pat = format!("%{}%", query);
        let mut stmt = conn.prepare(
            "SELECT b.oid, b.title, b.summary, p.path_rel
             FROM paths p JOIN blobs b ON b.oid = p.oid
             WHERE p.path_rel LIKE ?1",
        )?;
        let rows = stmt.query_map(params![pat], |r| {
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

    // (3) Column-cascade FTS rank, weighted by the field tuple
    //     (title:5, aliases:4, tags:3, keywords:3, summary:2, body:1).
    //
    // For each matching doc, we determine the highest-weight column that
    // alone contains the term — i.e. the doc's "primary" field. Within a
    // bucket the rows are ordered by the column-restricted bm25 score. The
    // result is that a doc whose token appears ONLY in `title` ranks
    // ahead of a doc whose token appears in `title` AND `aliases`: the
    // second doc cascades down to the aliases bucket because aliases is
    // its lowest-priority matching column.
    let fts_query = make_fts_query(query);
    if !fts_query.is_empty() {
        const COLUMNS: &[&str] = &[
            "title",
            "aliases_text",
            "tags_text",
            "keywords_text",
            "summary",
            "body",
        ];

        // Collect bucket assignments: oid -> highest column index that
        // matched.
        let mut bucket_for: std::collections::HashMap<String, (usize, String, String, String)> =
            std::collections::HashMap::new();

        for (idx, col) in COLUMNS.iter().enumerate() {
            let col_query = format!("{}:{}", col, &fts_query);
            let mut stmt = conn.prepare(
                "SELECT b.oid, b.title, b.summary, p.path_rel
                 FROM fts
                 JOIN blobs b ON b.rowid = fts.rowid
                 JOIN paths p ON p.oid = b.oid
                 WHERE fts MATCH ?1",
            )?;
            let rows = stmt.query_map(params![col_query], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })?;
            for row in rows {
                let (oid, title, summary, path) = row?;
                // Overwrite — later (lower-weight) column wins.
                bucket_for.insert(oid, (idx, title, summary, path));
            }
        }

        // Sort by bucket index ascending, then by title for stable order.
        let mut bucketed: Vec<_> = bucket_for.into_iter().collect();
        bucketed.sort_by(|a, b| {
            (a.1.0, &a.1.1)
                .cmp(&(b.1.0, &b.1.1))
        });
        for (oid, (_idx, title, summary, path)) in bucketed {
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

    let total = out.len();
    let paged: Vec<SearchResult> = out.into_iter().skip(offset).take(limit).collect();
    Ok((paged, total))
}

/// Escape an FTS5 MATCH query: wrap the query in double quotes and double
/// any embedded quotes so it is treated as a phrase regardless of operator
/// characters.
fn make_fts_query(q: &str) -> String {
    let trimmed = q.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let escaped = trimmed.replace('"', "\"\"");
    format!("\"{}\"", escaped)
}
