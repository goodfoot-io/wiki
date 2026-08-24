//! Repo-root `.wikiignore` exclusion across all three index passes (the
//! promoted location, plan D14).
//!
//! Pass 3 (Worktree): A path matched by `.wikiignore` must never be
//! carried into `seen_paths`, so the removal-reconciliation sweep purges any
//! previously-indexed row. Un-ignoring a path must restore visibility.
//!
//! Pass 1 (Tree / `--source head`) and Pass 2 (Index / `--source index`):
//! The same filter must apply so wikiignored committed or staged files are
//! never returned by any wiki CLI surface regardless of the active source.

mod common;

use wiki::index::{DocSource, WikiIndex};

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
    repo.write_file(".wikiignore", "drafts/\n");
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
    repo.write_file(".wikiignore", "docs/secret.md\n");
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

/// Removing a pattern from `.wikiignore` un-ignores a file that was
/// never indexed (it was created already-ignored): the next refresh must make
/// it visible again. (Adopted from the competing plan.)
#[test]
fn test_pass_worktree_uningnore_restores_visibility() {
    let repo = common::FixtureRepo::new();
    // Create the file already wikiignored; it is never indexed.
    repo.write_wiki_md("drafts/page.md", "Draft", "A draft.", "body");
    repo.write_file(".wikiignore", "drafts/\n");
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
    repo.write_file(".wikiignore", "# nothing ignored\n");
    let index = WikiIndex::prepare(repo.root.as_path()).expect("prepare after un-ignore");
    assert!(
        index.resolve_page("Draft").expect("resolve").is_some(),
        "un-ignored file must become visible again"
    );
}

// ---------------------------------------------------------------------------
// Pass 1 (Tree / --source head) exclusion tests
// ---------------------------------------------------------------------------

/// A committed wikiignored file must be absent from the Tree-source index
/// (`--source head`): `pass_tree` must not insert it into the DB.
#[test]
fn test_pass_tree_excludes_wikiignored_committed_file() {
    let repo = common::FixtureRepo::new();

    // Write the wikiignore BEFORE committing so it is present from the start.
    repo.write_file(".wikiignore", "drafts/\n");
    repo.write_wiki_md("drafts/secret.md", "Secret", "Hidden draft.", "body");
    repo.git_add(".wikiignore");
    repo.git_add("drafts/secret.md");
    repo.git_commit("add secret and wikiignore");

    let index =
        WikiIndex::prepare_for_source(repo.root.as_path(), DocSource::Head).expect("prepare head");
    assert!(
        index.resolve_page("Secret").expect("resolve").is_none(),
        "wikiignored committed file must be absent from --source head"
    );
}

/// A committed wikiignored file that was indexed BEFORE the wikiignore was
/// introduced must be removed from the Tree-source index on the next refresh.
#[test]
fn test_pass_tree_removes_newly_wikiignored_committed_file() {
    let repo = common::FixtureRepo::new();

    // First commit: file is not wikiignored.
    repo.write_wiki_md("drafts/secret.md", "Secret", "Hidden draft.", "body");
    repo.git_add("drafts/secret.md");
    repo.git_commit("add secret");

    // Populate the index for the Tree source so a row exists for this file.
    let index =
        WikiIndex::prepare_for_source(repo.root.as_path(), DocSource::Head).expect("prepare head");
    assert!(
        index.resolve_page("Secret").expect("resolve").is_some(),
        "file is visible before wikiignore"
    );
    drop(index);

    // Second commit: introduce the wikiignore.
    repo.write_file(".wikiignore", "drafts/\n");
    repo.git_add(".wikiignore");
    repo.git_commit("add wikiignore");

    let index =
        WikiIndex::prepare_for_source(repo.root.as_path(), DocSource::Head).expect("prepare head after ignore");
    assert!(
        index.resolve_page("Secret").expect("resolve").is_none(),
        "wikiignored committed file must be removed from --source head after wikiignore commit"
    );
}

/// Un-ignoring a committed file via a commit that only edits
/// `.wikiignore` (the file's blob unchanged across that commit) must
/// restore the file to `--source head`. The Tree pass is diff-based and emits
/// no delta for the unchanged file, so the bidirectional reconciliation
/// triggered by the wikiignore content change must re-add the previously
/// removed Tree row. Mirrors the 3-commit runtime repro.
#[test]
fn test_pass_tree_uningnore_unchanged_blob_restores_head() {
    let repo = common::FixtureRepo::new();

    // Commit 1: two wiki pages, neither ignored.
    repo.write_wiki_md("wiki/pub.md", "Pub", "A public page.", "body");
    repo.write_wiki_md("wiki/doc.md", "Doc", "A doc page.", "body");
    repo.git_add("wiki/pub.md");
    repo.git_add("wiki/doc.md");
    repo.git_commit("add pub and doc");

    // Seed the Head DB.
    let index =
        WikiIndex::prepare_for_source(repo.root.as_path(), DocSource::Head).expect("prepare head");
    assert!(index.resolve_page("Doc").expect("resolve").is_some());
    assert!(index.resolve_page("Pub").expect("resolve").is_some());
    drop(index);

    // Commit 2: ignore doc.md — its blob is NOT touched in this commit.
    repo.write_file(".wikiignore", "wiki/doc.md\n");
    repo.git_add(".wikiignore");
    repo.git_commit("ignore doc");

    let index = WikiIndex::prepare_for_source(repo.root.as_path(), DocSource::Head)
        .expect("prepare head after ignore");
    assert!(
        index.resolve_page("Doc").expect("resolve").is_none(),
        "ignored committed file must be removed from --source head"
    );
    assert!(index.resolve_page("Pub").expect("resolve").is_some());
    drop(index);

    // Commit 3: remove the pattern — doc.md's blob is STILL unchanged.
    repo.write_file(".wikiignore", "# nothing ignored\n");
    repo.git_add(".wikiignore");
    repo.git_commit("un-ignore doc");

    let index = WikiIndex::prepare_for_source(repo.root.as_path(), DocSource::Head)
        .expect("prepare head after un-ignore");
    assert!(
        index.resolve_page("Doc").expect("resolve").is_some(),
        "un-ignored committed file (blob unchanged) must be restored to --source head"
    );
    assert!(index.resolve_page("Pub").expect("resolve").is_some());
}

// ---------------------------------------------------------------------------
// Pass 2 (Index / --source index) exclusion tests
// ---------------------------------------------------------------------------

/// Regression: an UNSTAGED `.wikiignore` edit (git index checksum
/// unchanged) must still cause `pass_index` to exclude the now-wikiignored
/// file. This is the fail-open leak: previously `pass_index` saw an unchanged
/// git index checksum and returned early, leaving stale `Source::Index` rows
/// for files whose wikiignore status changed without a `git add`.
///
/// Runtime repro: commit pub.md + doc.md; then write `.wikiignore` =
/// `wiki/doc.md` WITHOUT staging. `wiki --source index list` must NOT list
/// doc.md.
#[test]
fn test_pass_index_excludes_wikiignored_file_when_wikiignore_unstaged() {
    let repo = common::FixtureRepo::new();

    // Commit both pages so they appear in the git index.
    repo.write_wiki_md("wiki/pub.md", "Pub", "A public page.", "body");
    repo.write_wiki_md("wiki/doc.md", "Doc", "A secret doc.", "topsecret_token");
    repo.git_add("wiki/pub.md");
    repo.git_add("wiki/doc.md");
    repo.git_commit("add pub and doc");

    // Seed the Index DB — both files must be visible.
    let index = WikiIndex::prepare_for_source(repo.root.as_path(), DocSource::Index)
        .expect("prepare index initial");
    assert!(
        index.resolve_page("Doc").expect("resolve").is_some(),
        "doc.md visible before wikiignore"
    );
    assert!(
        index.resolve_page("Pub").expect("resolve").is_some(),
        "pub.md visible before wikiignore"
    );
    drop(index);

    // Write `.wikiignore` WITHOUT staging — git index checksum unchanged.
    repo.write_file(".wikiignore", "wiki/doc.md\n");

    // Pass 2 must still exclude doc.md even though the git index didn't change.
    let index = WikiIndex::prepare_for_source(repo.root.as_path(), DocSource::Index)
        .expect("prepare index after unstaged wikiignore");
    assert!(
        index.resolve_page("Doc").expect("resolve").is_none(),
        "wikiignored file must be excluded from --source index even when .wikiignore is unstaged"
    );
    assert!(
        index.resolve_page("Pub").expect("resolve").is_some(),
        "non-ignored sibling must remain visible"
    );
    drop(index);

    // Un-ignore: remove the pattern (still unstaged) — doc.md must come back.
    repo.write_file(".wikiignore", "# nothing ignored\n");
    let index = WikiIndex::prepare_for_source(repo.root.as_path(), DocSource::Index)
        .expect("prepare index after un-ignore");
    assert!(
        index.resolve_page("Doc").expect("resolve").is_some(),
        "un-ignored file must be restored to --source index (un-ignore direction)"
    );
}

/// A staged wikiignored file must be absent from the Index-source index
/// (`--source index`): `pass_index` must not insert it into the DB.
#[test]
fn test_pass_index_excludes_wikiignored_staged_file() {
    let repo = common::FixtureRepo::new();

    repo.write_file(".wikiignore", "drafts/\n");
    repo.write_wiki_md("drafts/secret.md", "Secret", "Hidden draft.", "body");
    // Stage both — but do NOT commit, keeping them Index-only.
    repo.git_add(".wikiignore");
    repo.git_add("drafts/secret.md");

    let index = WikiIndex::prepare_for_source(repo.root.as_path(), DocSource::Index)
        .expect("prepare index");
    assert!(
        index.resolve_page("Secret").expect("resolve").is_none(),
        "wikiignored staged file must be absent from --source index"
    );
}
