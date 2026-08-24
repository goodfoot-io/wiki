//! A committed rename-with-edit (gix `Rewrite` at 50-99% similarity) under
//! the generations freshness model (plan merged-store-generations D5).
//!
//! The rewrite decomposes into Remove(a.md, old oid) + Add(b.md, new oid)
//! per source. Publishing never deletes anything physical: the old blob
//! row SURVIVES because the retained prior generation still serves it —
//! what must never happen is a ghost: a `blobs` row whose membership
//! refcount does not equal the number of retained generations referencing
//! it through `gen_paths`.
//!
//! Expected after the rename+edit prepare:
//!
//! - the served generation holds exactly one path row for b.md per source
//!   (Tree + Index + Worktree = 3), zero for a.md;
//! - both blob rows exist — old serving the retained generation, new
//!   serving the current one — and each refcount equals its
//!   `COUNT(DISTINCT gen_id)` membership count;
//! - resolution serves the edited content at b.md.

mod common;

use wiki::index::WikiIndex;
use wiki::index::blob::compute_blob_oid;

/// Ten-line body so that changing a single line keeps the edited file well
/// above gix's default 50% rename-detection similarity threshold, ensuring
/// the tree diff reports a `Rewrite` (rename + edit) rather than an
/// unrelated delete/add pair.
fn page_bytes(line_five: &str) -> String {
    format!(
        "---\ntitle: Migration Guide\nsummary: How to migrate between versions.\n---\n\n\
         Line one of the migration guide.\n\
         Line two of the migration guide.\n\
         Line three of the migration guide.\n\
         Line four of the migration guide.\n\
         {line_five}\n\
         Line six of the migration guide.\n\
         Line seven of the migration guide.\n\
         Line eight of the migration guide.\n\
         Line nine of the migration guide.\n\
         Line ten of the migration guide.\n"
    )
}

#[test]
fn committed_rename_with_edit_rebalances_blob_refcounts() {
    let repo = common::FixtureRepo::new();

    let old_bytes = page_bytes("Line five of the migration guide.");
    let new_bytes = page_bytes("Line five was rewritten in the rename.");
    let old_oid = compute_blob_oid(old_bytes.as_bytes());
    let new_oid = compute_blob_oid(new_bytes.as_bytes());
    assert_ne!(old_oid.0, new_oid.0, "edit must change the blob OID");

    // Commit the original page and index it.
    repo.write_file("a.md", &old_bytes);
    repo.git_add("a.md");
    repo.git_commit("add a.md");
    drop(WikiIndex::prepare(repo.root.as_path()).expect("prepare initial"));

    // One commit that both renames a.md -> b.md AND edits one body line,
    // so the tree diff sees a Rewrite with a NEW blob OID.
    repo.git_mv("a.md", "b.md");
    repo.write_file("b.md", &new_bytes);
    repo.git_add("b.md");
    repo.git_commit("rename a.md to b.md with an edit");

    let index = WikiIndex::prepare(repo.root.as_path()).expect("prepare after rename+edit");

    // The served generation has no a.md rows left and three b.md rows
    // (Tree from the rewrite decomposition plus Index and Worktree adds).
    let (old_blobs, old_served_paths) = index
        .debug_blob_path_counts(&old_oid.0)
        .expect("debug_blob_path_counts old oid");
    assert_eq!(
        old_served_paths, 0,
        "a.md must be gone from the served generation's corpus"
    );
    let (new_blobs, new_paths) = index
        .debug_blob_path_counts(&new_oid.0)
        .expect("debug_blob_path_counts new oid");
    assert_eq!(new_blobs, 1, "exactly one blobs row for the edited content");
    assert_eq!(new_paths, 3, "Tree + Index + Worktree gen_paths rows for b.md");

    // Immutability, not ghosting: the OLD blob row survives because the
    // retained predecessor generation still serves it — and its membership
    // refcount is exact, not leaked.
    assert_eq!(
        old_blobs, 1,
        "the old blob stays alive serving its retained generation"
    );

    // The page must resolve at its new location with edited content.
    let page = index
        .resolve_page("Migration Guide")
        .expect("resolve_page")
        .expect("page must survive the rename");
    assert!(
        page.file.ends_with("b.md"),
        "resolves to b.md: {}",
        page.file
    );
}
