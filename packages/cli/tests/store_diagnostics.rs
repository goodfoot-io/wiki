//! D10/D11 acceptance witnesses: countable store diagnostics surfaced
//! through the perf JSON-lines channel, and the tier-scoped clear contract.
//!
//! The corruption and schema-skew witnesses force their faults through the
//! debug fault-sequence env (`WIKI_ANCHOR_CACHE_TEST_FAULT_SEQUENCE`, whose
//! script items are `operational`, `schema`, `corrupt`) and assert against
//! the emitted `wiki.log` JSON lines — the durable structured channel (plan
//! D11) — never merely against the table. The `--perf` stderr echo prints
//! only name+duration by design, so the JSON-lines record is the only place
//! a counted diagnostic is visible.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

mod common;

use wiki::cache::diagnostics;
use wiki::cache::schema::{DB_FILE_NAME, STORE_EVENTS_DDL};

/// Spawn the wiki binary from `cwd` with a clean cache environment.
fn wiki(cwd: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_wiki"));
    cmd.current_dir(cwd)
        .env_remove("GIT_DIR")
        .env_remove("WIKI_ANCHOR_CACHE")
        .env_remove("WIKI_ANCHOR_CACHE_TEST_FAULT_SEQUENCE")
        .args(args);
    cmd
}

fn run(cwd: &Path, args: &[&str]) -> Output {
    wiki(cwd, args).output().expect("run wiki")
}

/// A fixture whose pages carry no line-range links: the drift engine's git
/// subprocesses never run, so only the store path under test is exercised.
fn plain_fixture() -> common::FixtureRepo {
    let repo = common::FixtureRepo::new();
    repo.write_wiki_md("wiki/page.md", "Page", "A plain page.", "Body without links.");
    repo.write_wiki_md(
        "wiki/other.md",
        "Other",
        "Another plain page.",
        "See [Page](page.md).",
    );
    repo.git_add("wiki/page.md");
    repo.git_add("wiki/other.md");
    repo.git_commit("add pages");
    repo
}

/// The merged-store directory: `<git-common-dir>/wiki`.
fn store_dir(repo_root: &Path) -> PathBuf {
    common::git_common_dir(repo_root).join("wiki")
}

/// Every parsed JSON-lines record in the perf log (D12 location:
/// `<git-common-dir>/wiki/wiki.log`).
fn log_records(repo_root: &Path) -> Vec<Value> {
    let contents =
        fs::read_to_string(store_dir(repo_root).join("wiki.log")).expect("read wiki.log");
    contents
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str::<Value>(line)
                .unwrap_or_else(|e| panic!("wiki.log line is not JSON: {e}\n{line}"))
        })
        .collect()
}

/// The last aggregated `anchor_cache` record — the one emitted by the most
/// recent run (the log appends across runs).
fn last_anchor_cache_record<'a>(records: &'a [Value]) -> &'a Value {
    records
        .iter()
        .rev()
        .find(|r| r["event"].as_str() == Some("anchor_cache"))
        .expect("an anchor_cache JSON-lines record must be emitted")
}

/// The diagnostics map of an `anchor_cache` record, if present.
fn diagnostics_of<'a>(record: &'a Value) -> Option<&'a Value> {
    let diagnostics = &record["meta"]["diagnostics"];
    diagnostics.is_object().then_some(diagnostics)
}

/// Count of `event` inside a diagnostics map, or 0 when absent.
fn counted(diagnostics: &Value, event: &str) -> u64 {
    diagnostics[event].as_u64().unwrap_or(0)
}

// ── D11 witness: forced corruption → quarantine + rebuild events ────────────

/// CARD.md's acceptance leg: corrupting the store yields a quarantine aside,
/// a rebuilt store, identical results (exit 0), and a visible countable
/// diagnostic event. The fault-sequence env overwrites the store with
/// garbage; the affected command path (`wiki check`) must quarantine it,
/// rebuild it, and the emitted JSON-lines record must carry both the
/// `quarantine_performed` and `rebuild_completed` events with count ≥ 1.
#[test]
fn forced_corruption_witnesses_quarantine_and_rebuild_in_json_lines() {
    let repo = plain_fixture();
    let warm = run(&repo.root, &["check"]);
    assert_eq!(warm.status.code(), Some(0), "warm run: {}", String::from_utf8_lossy(&warm.stderr));
    let dir = store_dir(&repo.root);
    assert!(dir.join(DB_FILE_NAME).exists(), "the warm run must have created the store");

    // The faulted run: corruption is forced through the env; keep the log
    // clean so the last anchor_cache record is unambiguously this run's.
    fs::remove_file(store_dir(&repo.root).join("wiki.log")).ok();
    let mut faulted = wiki(&repo.root, &["check"]);
    faulted.env("WIKI_ANCHOR_CACHE_TEST_FAULT_SEQUENCE", "corrupt");
    let out = faulted.output().expect("run corrupted check");
    assert_eq!(
        out.status.code(),
        Some(0),
        "quarantine-and-rebuild must not change the exit code: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // The suspect file was renamed aside, content preserved.
    let asides: Vec<PathBuf> = fs::read_dir(&dir)
        .expect("read store dir")
        .map(|e| e.expect("entry").path())
        .filter(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().ends_with(".quarantine"))
                .unwrap_or(false)
        })
        .collect();
    assert_eq!(asides.len(), 1, "exactly one quarantine rename-aside");
    let aside_bytes = fs::read(&asides[0]).expect("read aside");
    assert!(
        aside_bytes.starts_with(b"wiki test fault:"),
        "the aside must preserve the suspect bytes, got: {}",
        String::from_utf8_lossy(&aside_bytes)
    );

    // The rebuilt store serves again on the next plain run — identical
    // results continue after the quarantine.
    let after = run(&repo.root, &["check"]);
    assert_eq!(
        after.status.code(),
        Some(0),
        "the rebuilt store must serve: {}",
        String::from_utf8_lossy(&after.stderr)
    );

    // And the countable evidence is in the JSON-lines channel.
    let records = log_records(&repo.root);
    let carrier = last_anchor_cache_record(&records);
    let diagnostics = diagnostics_of(carrier)
        .expect("the corrupted run's anchor_cache record must carry meta.diagnostics");
    assert!(
        counted(diagnostics, "quarantine_performed") >= 1,
        "quarantine_performed must be counted ≥ 1 in {carrier}"
    );
    assert!(
        counted(diagnostics, "rebuild_completed") >= 1,
        "rebuild_completed must be counted ≥ 1 in {carrier}"
    );
}

// ── D11 witness: schema skew repair is counted per tier ─────────────────────

/// A deviating static shape for the anchor tier is skew, not corruption:
/// tier-scoped repair, no quarantine aside, and a countable
/// `skew_repair:anchor` event in the emitted JSON lines.
#[test]
fn forced_schema_skew_witnesses_the_tier_scoped_repair_in_json_lines() {
    let repo = plain_fixture();
    let warm = run(&repo.root, &["check"]);
    assert_eq!(warm.status.code(), Some(0));

    fs::remove_file(store_dir(&repo.root).join("wiki.log")).ok();
    let mut skewed = wiki(&repo.root, &["check"]);
    skewed.env("WIKI_ANCHOR_CACHE_TEST_FAULT_SEQUENCE", "schema");
    let out = skewed.output().expect("run skewed check");
    assert_eq!(
        out.status.code(),
        Some(0),
        "skew repair must not change the exit code: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Skew is structural invalidation, never a quarantine: no aside.
    let asides = fs::read_dir(store_dir(&repo.root))
        .expect("read store dir")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".quarantine"))
        .count();
    assert_eq!(asides, 0, "skew repair must not quarantine the store");

    let records = log_records(&repo.root);
    let carrier = last_anchor_cache_record(&records);
    let diagnostics = diagnostics_of(carrier)
        .expect("the skewed run's anchor_cache record must carry meta.diagnostics");
    assert!(
        counted(diagnostics, "skew_repair:anchor") >= 1,
        "skew_repair:anchor must be counted ≥ 1 in {carrier}"
    );
}

// ── D11: absent ⇒ field omitted ─────────────────────────────────────────────

/// A clean run over an empty ledger publishes nothing: the anchor_cache
/// record carries no diagnostics field at all — omitted, not empty.
#[test]
fn clean_run_omits_the_diagnostics_field_entirely() {
    let repo = plain_fixture();
    let out = run(&repo.root, &["check"]);
    assert_eq!(out.status.code(), Some(0));

    let records = log_records(&repo.root);
    let carrier = last_anchor_cache_record(&records);
    assert!(
        diagnostics_of(carrier).is_none(),
        "a run with no store connection publishing counts must omit meta.diagnostics, got {carrier}"
    );
}

// ── Ledger behavior: infallible record, bounded growth ──────────────────────

/// [`diagnostics::record`] can never fail its caller: without the ledger
/// table present at all, the insert silently drops and the caller carries on.
#[test]
fn record_is_infallible_without_the_ledger() {
    let dir = tempfile::tempdir().expect("tempdir");
    let conn = rusqlite::Connection::open(dir.path().join("bare.sqlite")).expect("open bare db");
    diagnostics::record(&conn, "quarantine_performed");
    // Publishing from an unreadable/absent ledger is equally silent.
    diagnostics::publish_counts(&conn);
}

/// The periodic prune keeps the ledger near its newest-1000 bound.
#[test]
fn pruning_keeps_the_newest_rows_only() {
    let dir = tempfile::tempdir().expect("tempdir");
    let conn = rusqlite::Connection::open(dir.path().join("ledger.sqlite")).expect("open db");
    conn.execute_batch(STORE_EVENTS_DDL).expect("create ledger");
    for i in 0..1200u64 {
        diagnostics::record(&conn, if i % 2 == 0 { "even" } else { "odd" });
    }
    let total: i64 = conn
        .query_row("SELECT count(*) FROM store_events", [], |r| r.get(0))
        .expect("count");
    assert!(
        (1000..1128).contains(&total),
        "prune must bound the ledger near 1000 rows, got {total}"
    );
}
