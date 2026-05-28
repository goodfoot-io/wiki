//! Concurrency test: a second `wiki` process must return results without
//! waiting on a held refresh lock.
//!
//! Contract: when `.git/wiki-refresh.lock` is held by an external process,
//! a `wiki search <query>` invocation exits with code 0 in < 200ms.

mod common;

use std::fs::OpenOptions;
use std::time::Instant;

use assert_cmd::Command;
use fs4::fs_std::FileExt;

#[test]
#[ignore = "tdd-bootstrap"]
fn second_process_returns_without_waiting_on_lock() {
    let repo = common::make_parity_fixture();

    // Hold the refresh lock ourselves to simulate a long-running refresh in
    // a sibling process.
    let lock_path = repo.root.join(".git").join("wiki-refresh.lock");
    let lock_file = OpenOptions::new()
        .create(true)
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

    // Must not block.
    assert!(
        elapsed.as_millis() < 200,
        "second process waited {}ms — exceeded 200ms budget",
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
