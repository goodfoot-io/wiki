//! Concurrency test: a second `wiki` process must return results without
//! waiting on a held refresh lock.
//!
//! Contract: when `.wiki/wiki-refresh.lock` is held by an external process,
//! a `wiki search <query>` invocation exits with code 0 without blocking on
//! the held lock (under 2s — the budget is generous to absorb VM/CI noise;
//! blocking on a lock would take seconds or hang).

mod common;

use std::fs::OpenOptions;
use std::time::Instant;

use assert_cmd::Command;
use fs4::fs_std::FileExt;

#[test]
fn second_process_returns_without_waiting_on_lock() {
    let repo = common::make_parity_fixture();

    // Hold the refresh lock ourselves to simulate a long-running refresh in
    // a sibling process.
    let wiki_dir = repo.root.join(".wiki");
    std::fs::create_dir_all(&wiki_dir).expect("create .wiki dir");
    let lock_path = wiki_dir.join("wiki-refresh.lock");
    let lock_file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&lock_path)
        .expect("open lock file");
    lock_file
        .try_lock_exclusive()
        .expect("acquire refresh lock for test");

    let start = Instant::now();

    // Run a second `wiki` process.  It should serve the existing snapshot
    // (possibly empty) rather than blocking on the held lock.
    let output = Command::cargo_bin("wiki")
        .expect("cargo_bin wiki")
        .current_dir(&repo.root)
        .args(["committed"])
        .output()
        .expect("wiki process");

    let elapsed = start.elapsed();

    // Must not block. Budget is generous to absorb VM/CI overhead;
    // actually waiting on a lock would take seconds or hang.
    assert!(
        elapsed.as_millis() < 2_000,
        "second process waited {}ms — exceeded 2s budget",
        elapsed.as_millis()
    );

    // Must exit cleanly.
    assert!(
        output.status.success(),
        "wiki exited with {:?}",
        output.status.code()
    );

    // Release the lock.
    lock_file.unlock().expect("unlock");
}

// ── Target-layout port (plan merged-store-generations, Phase 1; wired in
// Phase 4 per D7) ────────────────────────────────────────────────────────
//
// The refresh-publication guard moves to
// `<git-common-dir>/wiki/rendezvous.lock`; the contention contract is
// unchanged: a second process serves without blocking.

#[test]
#[ignore = "rendezvous lock wiring lands in Phase 4; production still uses .wiki/wiki-refresh.lock"]
fn second_process_returns_without_waiting_on_rendezvous_lock() {
    let repo = common::make_parity_fixture();

    // Hold the rendezvous lock exclusively to simulate a long-running
    // refresh publication in a sibling process.
    let wiki_dir = common::git_common_dir(&repo.root).join("wiki");
    std::fs::create_dir_all(&wiki_dir).expect("create common wiki dir");
    let lock_path = wiki_dir.join("rendezvous.lock");
    let lock_file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&lock_path)
        .expect("open rendezvous lock file");
    lock_file
        .try_lock_exclusive()
        .expect("acquire rendezvous lock for test");

    let start = Instant::now();

    // Run a second `wiki` process. It should serve (uncached or stale)
    // rather than blocking on the held rendezvous lock.
    let output = Command::cargo_bin("wiki")
        .expect("cargo_bin wiki")
        .current_dir(&repo.root)
        .args(["committed"])
        .output()
        .expect("wiki process");

    let elapsed = start.elapsed();

    assert!(
        elapsed.as_millis() < 2_000,
        "second process waited {}ms — exceeded 2s budget",
        elapsed.as_millis()
    );
    assert!(
        output.status.success(),
        "wiki exited with {:?}",
        output.status.code()
    );

    lock_file.unlock().expect("unlock");
}
