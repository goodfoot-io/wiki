//! Reproduction tests for worktree freshness bug:
//! worktree-only changes (new untracked pages, unstaged edits) are invisible
//! to the wiki index because `fast_gate` never validates worktree state.
//!
//! Every test in this file MUST FAIL against the current unfixed code.

mod common;

use common::FixtureRepo;
use wiki::index::{DocSource, WikiIndex};

#[test]
fn new_untracked_page_not_found_after_cold_index() {
    let repo = FixtureRepo::new();

    // One committed wiki page so the repo is non-empty.
    repo.write_wiki_md(
        "alpha.md",
        "Alpha",
        "First page summary.",
        "Alpha body.",
    );
    repo.git_add("alpha.md");
    repo.git_commit("initial");

    // Cold prepare — builds the index DB.
    let index = WikiIndex::prepare_for_source(repo.root.as_path(), DocSource::WorkingTree)
        .expect("cold prepare");
    let pages: Vec<String> = index
        .list_pages(None, 0, None)
        .unwrap()
        .into_iter()
        .map(|r| r.title)
        .collect();
    assert!(
        pages.contains(&"Alpha".to_string()),
        "cold index should contain Alpha, got {pages:?}"
    );
    // Drop so the next prepare re-opens and goes through fast_gate.
    drop(index);

    // Create a brand-new untracked wiki page.
    repo.write_wiki_md(
        "brandnew.md",
        "Brandnew",
        "A freshly created untracked page.",
        "Brandnew body.",
    );

    // Warm prepare — must detect the untracked page.
    let index2 = WikiIndex::prepare_for_source(repo.root.as_path(), DocSource::WorkingTree)
        .expect("warm prepare");
    let pages2: Vec<String> = index2
        .list_pages(None, 0, None)
        .unwrap()
        .into_iter()
        .map(|r| r.title)
        .collect();
    assert!(
        pages2.contains(&"Brandnew".to_string()),
        "warm index should contain Brandnew after untracked page created, got {pages2:?}"
    );
}

#[test]
fn unstaged_edit_shows_stale_summary() {
    let repo = FixtureRepo::new();

    repo.write_wiki_md(
        "page.md",
        "Original Title",
        "Original summary.",
        "Original body.",
    );
    repo.git_add("page.md");
    repo.git_commit("initial");

    // Cold prepare.
    let index = WikiIndex::prepare_for_source(repo.root.as_path(), DocSource::WorkingTree)
        .expect("cold prepare");
    let resolved = index.resolve_page("Original Title").expect("resolve").expect("found");
    assert_eq!(resolved.summary, "Original summary.");
    drop(index);

    // Edit in place — replace title and summary, do NOT stage.
    std::thread::sleep(std::time::Duration::from_millis(20));
    repo.write_wiki_md(
        "page.md",
        "Updated Title",
        "Updated summary.",
        "Updated body.",
    );

    // Warm prepare — must see the new title and summary.
    let index2 = WikiIndex::prepare_for_source(repo.root.as_path(), DocSource::WorkingTree)
        .expect("warm prepare");
    let resolved2 = index2
        .resolve_page("Updated Title")
        .expect("resolve2")
        .expect("found2");
    assert_eq!(
        resolved2.summary, "Updated summary.",
        "warm index should serve updated summary after unstaged edit"
    );
}

#[test]
fn tree_unchanged_returns_false_on_dirty_worktree() {
    let repo = FixtureRepo::new();

    repo.write_wiki_md(
        "page.md",
        "Page",
        "Summary.",
        "Body.",
    );
    repo.git_add("page.md");
    repo.git_commit("initial");

    // Cold prepare — builds the index DB so tree_unchanged has a state row
    // to compare against.
    let _index = WikiIndex::prepare_for_source(repo.root.as_path(), DocSource::WorkingTree)
        .expect("cold prepare");

    // tree_unchanged must return true immediately after a refresh.
    let clean = wiki::index::tree_unchanged(repo.root.as_path());
    assert!(
        clean,
        "tree_unchanged must return true on a clean worktree just after refresh"
    );

    // Mutate the worktree without staging.
    repo.write_wiki_md(
        "page.md",
        "Page Modified",
        "Modified summary.",
        "Modified body.",
    );

    // tree_unchanged must return false on a dirty worktree.
    let dirty = wiki::index::tree_unchanged(repo.root.as_path());
    assert!(
        !dirty,
        "tree_unchanged must return false on a dirty-but-unstaged worktree"
    );
}

#[test]
fn tree_unchanged_false_with_new_untracked_wiki_page() {
    let repo = FixtureRepo::new();

    repo.write_wiki_md(
        "page.md",
        "Page",
        "Summary.",
        "Body.",
    );
    repo.git_add("page.md");
    repo.git_commit("initial");

    // Build index.
    let _index = WikiIndex::prepare_for_source(repo.root.as_path(), DocSource::WorkingTree)
        .expect("cold prepare");

    // Add an untracked wiki page.
    repo.write_wiki_md(
        "untracked.md",
        "Untracked",
        "An untracked wiki page.",
        "Body.",
    );

    let dirty = wiki::index::tree_unchanged(repo.root.as_path());
    assert!(
        !dirty,
        "tree_unchanged must return false when an untracked wiki page exists"
    );
}
