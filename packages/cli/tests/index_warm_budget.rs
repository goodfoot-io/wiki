//! Warm-budget test: 5 consecutive warm `search_weighted` calls must complete
//! with p95 < 50ms wall-clock.
//!
//! The 50ms headroom is intentional — the hard ceiling from CARD.md is 100ms;
//! the test catches regressions well before that ceiling is breached.

mod common;

use std::time::Instant;

use wiki::index::{WikiIndex, SEARCH_LIMIT};

#[test]
fn warm_search_p95_under_50ms() {
    let repo = common::make_parity_fixture();

    // Prime the index with one search so the DB is warm in the OS page cache.
    let index = WikiIndex::prepare(repo.root.as_path()).expect("prepare");
    let _ = index.search_weighted("committed", SEARCH_LIMIT, 0);

    // Measure 5 warm calls.
    let mut durations_ms: Vec<u128> = (0..5)
        .map(|_| {
            let t = Instant::now();
            let _ = index.search_weighted("committed", SEARCH_LIMIT, 0);
            t.elapsed().as_millis()
        })
        .collect();

    // p95 of 5 samples is the max (all five must pass the budget individually).
    durations_ms.sort_unstable();
    let p95 = *durations_ms.last().unwrap_or(&u128::MAX);

    assert!(
        p95 < 50,
        "warm-path p95 was {}ms — exceeded 50ms budget",
        p95
    );
}
