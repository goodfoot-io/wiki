//! Reproduction test for the per-OID FTS probe loop in `search_weighted`.
//!
//! Bug: the total-count correction iterates `pre_fts_seen` and issues a
//! separate `SELECT 1 FROM fts ... WHERE fts MATCH ? LIMIT 1` per OID
//! (search.rs L240-L257). Each probe re-parses and re-evaluates the entire
//! MATCH expression against the whole corpus just to test membership of one
//! document — O(pre-matches × corpus) instead of O(corpus).
//!
//! This test counts the FTS MATCH statements SQLite actually executes during
//! a single `search_weighted` call (via rusqlite's `trace` feature). With N
//! pre-FTS matches the unfixed code issues N + 2 MATCH statements (one
//! capped scan, one uncapped COUNT(*), plus N probes); the fixed code stays
//! at a bounded constant regardless of pre-match count.
//!
//! MUST FAIL against the current unfixed code.

mod common;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use rusqlite::{Connection, OpenFlags};
use wiki::index::search::search_weighted;
use wiki::index::{DocSource, WikiIndex};

static MATCH_STATEMENTS: AtomicUsize = AtomicUsize::new(0);

fn tracer(sql: &str) {
    if sql.to_uppercase().contains("MATCH") {
        MATCH_STATEMENTS.fetch_add(1, Ordering::SeqCst);
    }
}

/// Run `query` against the index DB with a trace hook installed and return
/// `(executed MATCH statements, reported total)`. Only the FTS stage, the
/// uncapped COUNT(*), and the correction probes carry MATCH; stages (1),
/// (1b), and (2) issue plain SELECT/LIKE statements.
fn measure_match_statements(db_path: &Path, query: &str) -> (usize, usize) {
    let mut conn =
        Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("open read-only index");

    MATCH_STATEMENTS.store(0, Ordering::SeqCst);
    conn.trace(Some(tracer));
    let (rows, total) =
        search_weighted(&conn, DocSource::WorkingTree, query, 10, 0)
            .expect("search_weighted");
    conn.trace(None);
    let _ = rows;

    (MATCH_STATEMENTS.load(Ordering::SeqCst), total)
}

fn build_index(repo: &common::FixtureRepo) -> PathBuf {
    let index = WikiIndex::prepare(repo.root.as_path()).expect("prepare");
    drop(index);

    let db_path = repo.root.join(".wiki").join("wiki-index.sqlite");
    assert!(db_path.exists(), "index DB should exist after prepare");
    db_path
}

#[test]
fn total_count_correction_does_not_reprobe_fts_per_pre_match() {
    // Scenario A: twelve distinct blobs sharing the exact title "Gadget".
    // The exact-title stage collects 12 pre-FTS OIDs, and because the title
    // text itself is indexed, every one of them also satisfies the FTS query.
    {
        let repo = common::FixtureRepo::new();
        const PRE_MATCHES: usize = 12;
        for i in 0..PRE_MATCHES {
            repo.write_wiki_md(
                &format!("gadget_{i}.md"),
                "Gadget",
                &format!("Summary variant {i}."),
                &format!("Distinct body content {i} without further tokens."),
            );
        }
        repo.git_add(".");
        repo.git_commit("add shared-title pre-match fixtures");
        let db_path = build_index(&repo);

        let (statements, total) = measure_match_statements(&db_path, "gadget");

        // Correctness guardrail: all pre-matches also match FTS, so the true
        // uncapped total is 12 and the correction must add zero extras.
        assert_eq!(total, PRE_MATCHES, "total must stay identical to the per-OID computation");

        assert!(
            statements <= 4,
            "search_weighted executed {statements} FTS MATCH statements with \
             {PRE_MATCHES} pre-FTS matches; expected <= 4 (capped scan + \
             COUNT(*) + a single set-based correction). Unfixed code issues \
             {} (2 + {PRE_MATCHES} probes).",
            2 + PRE_MATCHES
        );
    }

    // Scenario B (the card's required equivalence case): more than one
    // pre-FTS match where SOME satisfy the FTS query and some do not.
    // Exact-title twins are indexed text => they satisfy FTS; path-LIKE hits
    // match on unindexed path text => they do not, and must surface as extras.
    {
        let repo = common::FixtureRepo::new();
        for i in 0..3 {
            repo.write_wiki_md(
                &format!("twin_{i}.md"),
                "Gadget Notes",
                &format!("Twin summary {i}."),
                &format!("Filler body {i}."),
            );
        }
        for i in 0..3 {
            repo.write_wiki_md(
                &format!("docs/gadget notes b{i}.md"),
                &format!("Handbook {i}"),
                &format!("Handbook summary {i}."),
                &format!("Unrelated prose volume {i}."),
            );
        }
        repo.git_add(".");
        repo.git_commit("add mixed pre-match fixtures");
        let db_path = build_index(&repo);

        let (statements, total) = measure_match_statements(&db_path, "gadget notes");

        // Truth table: FTS finds the 3 twin docs; the 3 path-only pre-matches
        // do not satisfy the FTS expression and are added as extras => 6.
        assert_eq!(total, 6, "extras from path-only pre-matches must be counted exactly once");

        assert!(
            statements <= 4,
            "search_weighted executed {statements} FTS MATCH statements with \
             6 pre-FTS matches; expected <= 4. Unfixed code issues 8."
        );
    }
}
