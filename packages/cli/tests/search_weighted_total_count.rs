//! Reproduction test for `search_weighted` returning a page-size-capped total.
//!
//! Bug: `search_weighted` computes `total = out.len()` (line 177 of search.rs)
//! after the FTS query applies a `LIMIT cap` where `cap = limit + offset + 64`.
//! When the corpus has more matching documents than `cap`, the total underreports
//! the true match count. The caller in `commands/search.rs` uses `total` to
//! render a "*N other wiki matches.*" footer and expects the uncapped count.
//!
//! This test creates > 74 pages (exceeding `limit + offset + 64` for
//! `limit=10, offset=0`), each containing the search token "gadget" only in
//! the body (no exact title/alias match, no path match). It asserts that
//! `total` equals the true number of matching pages, NOT the capped count.
//!
//! MUST FAIL against the current unfixed code because `total` will be ~74
//! rather than the true count of ~85.

mod common;

use wiki::index::WikiIndex;

#[test]
fn search_weighted_total_is_uncapped() {
    let repo = common::FixtureRepo::new();

    // Number of pages must exceed cap = limit + offset + 64 = 10 + 0 + 64 = 74.
    // With 85 pages, the FTS LIMIT will truncate at 74, but the true match
    // count is 85.
    let n_pages = 85usize;

    for i in 0..n_pages {
        // Use unique titles that do NOT contain "gadget" (avoids exact-title
        // match phase) and a body that ONLY contains "gadget" (forces all
        // matches through the FTS phase, which has the LIMIT cap).
        repo.write_wiki_md(
            &format!("page_{i}.md"),
            &format!("Page {i}"),
            &format!("Summary for page {i}."),
            "This page has gadget in the body for testing purposes.",
        );
    }
    repo.git_add(".");
    repo.git_commit("add search_weighted total count fixtures");

    let index = WikiIndex::prepare(repo.root.as_path()).expect("prepare");
    let (_rows, total) = index
        .search_weighted("gadget", 10, 0)
        .expect("search_weighted");

    // The true uncapped match count should equal the number of pages created.
    // Against the current unfixed code, `total` will be at most
    // `10 + 0 + 64 = 74`, causing this assertion to fail.
    assert_eq!(
        total, n_pages,
        "search_weighted total must reflect the true uncapped match count, \
         not the FTS LIMIT-cap count. Expected {n_pages}, got {total}. \
         This test MUST FAIL against the unfixed code."
    );
}
