//! `.wiki/.wikiignore` exclusion at the worktree index pass.
//!
//! A path matched by `.wiki/.wikiignore` must never be carried into
//! `seen_paths` during `pass_worktree`, so the removal-reconciliation sweep
//! purges any row that was previously indexed. Un-ignoring a path (removing
//! its pattern) must restore visibility on the next refresh.

mod common;

use wiki::index::WikiIndex;

/// A file already in the index that becomes wikiignored on the next refresh
/// must be removed (its page no longer resolves).
#[test]
fn test_pass_worktree_removes_wikiignored_file_from_index() {
    let repo = common::FixtureRepo::new();
    repo.write_wiki_md("drafts/secret.md", "Secret", "Hidden draft.", "body");
    repo.git_add("drafts/secret.md");
    repo.git_commit("add secret");

    let index = WikiIndex::prepare(repo.root.as_path()).expect("prepare");
    assert!(
        index.resolve_page("Secret").expect("resolve").is_some(),
        "file is indexed before it is wikiignored"
    );
    drop(index);

    // Now wikiignore it and refresh.
    repo.write_file(".wiki/.wikiignore", "drafts/\n");
    let index = WikiIndex::prepare(repo.root.as_path()).expect("prepare after ignore");
    assert!(
        index.resolve_page("Secret").expect("resolve").is_none(),
        "wikiignored file must be removed from the index"
    );
}

/// A wikiignored file inside a directory whose mtime did not change (the
/// clean-dir fast path) must still be removed: it is not carried forward into
/// `seen_paths`, so the removal sweep deletes its row.
#[test]
fn test_pass_worktree_clean_dir_carry_forward_respects_wikiignore() {
    let repo = common::FixtureRepo::new();
    repo.write_wiki_md("docs/page.md", "Page", "A page.", "body");
    repo.write_wiki_md("docs/secret.md", "Secret", "Hidden.", "body");
    repo.git_add("docs/page.md");
    repo.git_add("docs/secret.md");
    repo.git_commit("add docs");

    let index = WikiIndex::prepare(repo.root.as_path()).expect("prepare");
    assert!(index.resolve_page("Secret").expect("resolve").is_some());
    drop(index);

    // Add the ignore without touching docs/ dir mtime. The .wikiignore mtime
    // is folded into the freshness gate, so a refresh runs and the clean-dir
    // carry-forward must omit the wikiignored child.
    repo.write_file(".wiki/.wikiignore", "docs/secret.md\n");
    let index = WikiIndex::prepare(repo.root.as_path()).expect("prepare after ignore");
    assert!(
        index.resolve_page("Secret").expect("resolve").is_none(),
        "wikiignored child in a clean dir must be removed"
    );
    assert!(
        index.resolve_page("Page").expect("resolve").is_some(),
        "co-located non-ignored sibling stays indexed"
    );
}

/// Removing a pattern from `.wiki/.wikiignore` un-ignores a file that was
/// never indexed (it was created already-ignored): the next refresh must make
/// it visible again. (Adopted from the competing plan.)
#[test]
fn test_pass_worktree_uningnore_restores_visibility() {
    let repo = common::FixtureRepo::new();
    // Create the file already wikiignored; it is never indexed.
    repo.write_wiki_md("drafts/page.md", "Draft", "A draft.", "body");
    repo.write_file(".wiki/.wikiignore", "drafts/\n");
    repo.git_add("drafts/page.md");
    repo.git_commit("add draft");

    let index = WikiIndex::prepare(repo.root.as_path()).expect("prepare");
    assert!(
        index.resolve_page("Draft").expect("resolve").is_none(),
        "never-indexed wikiignored file is absent"
    );
    drop(index);

    // Un-ignore: remove the pattern. The .wikiignore mtime changes, busting
    // the freshness gate; the WalkDir walker still visits the (never-indexed)
    // file and indexes it.
    repo.write_file(".wiki/.wikiignore", "# nothing ignored\n");
    let index = WikiIndex::prepare(repo.root.as_path()).expect("prepare after un-ignore");
    assert!(
        index.resolve_page("Draft").expect("resolve").is_some(),
        "un-ignored file must become visible again"
    );
}
