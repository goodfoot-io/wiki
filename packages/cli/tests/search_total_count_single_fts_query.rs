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
    // The served generation is whichever one the prepare left newest; a
    // freshly prepared single-generation store serves gen_id 1's corpus.
    let gen_id: i64 = conn
        .query_row(
            "SELECT gen_id FROM generations ORDER BY created_at DESC, gen_id DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .expect("served generation");
    let (rows, total) =
        search_weighted(&conn, gen_id, DocSource::WorkingTree, query, 10, 0)
            .expect("search_weighted");
    conn.trace(None);
    let _ = rows;

    (MATCH_STATEMENTS.load(Ordering::SeqCst), total)
}

/// Target-layout port of [`build_index`]: the merged store at
/// `<git-common-dir>/wiki/store.sqlite`.
#[test]
fn build_index_at_merged_store() {
    let repo = common::FixtureRepo::new();
    let index = WikiIndex::prepare(repo.root.as_path()).expect("prepare");
    drop(index);

    let db_path = common::target_db_path(&repo.root);
    assert!(db_path.exists(), "merged store should exist after prepare");
}

// ── Target-layout port (plan merged-store-generations, Phase 1) ──────────
//
// The tracer methodology and exact uncapped totals are layout-independent;
// only the DB path moves. Under the merged store the same bound must hold
// against `fts_<served_gen>` — the per-generation FTS children preserve
// both the single-query correction and the totals.

#[test]
fn total_count_probes_do_not_scale_at_merged_store() {
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
    let index = WikiIndex::prepare(repo.root.as_path()).expect("prepare");
    drop(index);
    let db_path = common::target_db_path(&repo.root);
    assert!(db_path.exists(), "merged store should exist after prepare");

    let (statements, total) = measure_match_statements(&db_path, "gadget");

    assert_eq!(total, PRE_MATCHES, "total must stay identical to the per-OID computation");
    assert!(
        statements <= 4,
        "search_weighted executed {statements} FTS MATCH statements at the \
         merged store with {PRE_MATCHES} pre-FTS matches; expected <= 4"
    );
}
