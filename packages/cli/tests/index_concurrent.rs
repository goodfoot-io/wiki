//! Concurrency test: a second `wiki` process must return results without
//! blocking forever on a held rendezvous lock (plan D7).
//!
//! Contract: when `<common>/wiki/rendezvous.lock` is held EXCLUSIVELY by an
//! external process, a `wiki search <query>` invocation still exits 0 — it
//! waits out the bounded budget (~10 s), then takes the floor: serve the
//! newest retained snapshot / proceed uncached after one diagnostic line.
//! The budget assertion is therefore generous-but-bounded: completing at
//! all proves no unbounded hang; staying under 15 s proves the wait is
//! bounded as designed.

mod common;

use std::time::Instant;

use assert_cmd::Command;
use wiki::cache::rendezvous;

#[test]
fn second_process_returns_without_waiting_on_rendezvous_lock() {
    let repo = common::make_parity_fixture();
    let common = common::git_common_dir(&repo.root);

    // Hold the rendezvous exclusively to simulate a long-running refresh
    // publication in a sibling process. Acquired through the production API
    // so the lock file and its private parent subtree are created exactly
    // as the open paths create them.
    let _held = rendezvous::try_acquire_exclusive(&common)
        .expect("rendezvous acquire")
        .expect("free store grants exclusive");

    let start = Instant::now();

    // Run a second `wiki` process. It must not hang: bounded wait, then the
    // serve-stale/uncached floor.
    let output = Command::cargo_bin("wiki")
        .expect("cargo_bin wiki")
        .current_dir(&repo.root)
        .args(["committed"])
        .output()
        .expect("wiki process");

    let elapsed = start.elapsed();

    // Bounded, not unbounded: under the 10 s acquisition budget plus normal
    // run overhead.
    assert!(
        elapsed.as_millis() < 15_000,
        "second process took {}ms — exceeded the bounded-wait envelope",
        elapsed.as_millis()
    );

    // Must exit cleanly regardless of which side of the floor it landed on.
    assert!(
        output.status.success(),
        "wiki exited with {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
}
