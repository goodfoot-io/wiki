//! A committed rename-with-edit (gix `Rewrite` at 50-99% similarity) must
//! perform full blob bookkeeping, not just a path swap. The Tree pass maps
//! the rewrite to `DeltaAction::Rename` carrying only the NEW blob OID, and
//! `apply_rename` swaps the `paths` row without touching refcounts. After a
//! commit that renames AND edits a page:
//!
//! - the OLD blob's refcount is never decremented for the Tree source, so
//!   the old `blobs` row leaks (refcount 1, zero `paths` rows) and its FTS
//!   entry remains searchable as ghost content;
//! - the NEW blob's refcount undercounts (2 from the Index/Worktree Add
//!   deltas) while THREE `paths` rows reference it (Tree, Index, Worktree).
//!
//! Expected: the old blob row is gone (`(0, 0)`) and the new blob has one
//! `blobs` row referenced by three `paths` rows (`(1, 3)`).
//!
//! Under the current code this test fails at `prepare`: deltas apply in
//! strict Tree -> Index -> Worktree order, so the Tree `Rename` inserts a
//! `paths` row at the NEW OID before any delta has created its `blobs` row,
//! and `paths.oid REFERENCES blobs(oid)` aborts the refresh with
//! "FOREIGN KEY constraint failed" — the earliest symptom of the same
//! missing bookkeeping. A pure rename (unchanged content, same OID) does
//! not hit this because the old blob row already exists at that OID.

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

    // Commit the original page and index it: old OID has refcount 3
    // (Tree + Index + Worktree paths rows).
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

    // The old blob must be fully released: the Tree rename plus the
    // Index/Worktree removals take its refcount 3 -> 0, deleting the row.
    let (old_blobs, old_paths) = index
        .debug_blob_path_counts(&old_oid.0)
        .expect("debug_blob_path_counts old oid");
    assert_eq!(
        (old_blobs, old_paths),
        (0, 0),
        "old blob row must be deleted after rename+edit; a leftover blobs \
         row with zero paths rows is the leaked ghost blob"
    );

    // The new blob is referenced by exactly three paths rows (Tree from the
    // rename swap, plus Index and Worktree adds) backed by one blobs row.
    let (new_blobs, new_paths) = index
        .debug_blob_path_counts(&new_oid.0)
        .expect("debug_blob_path_counts new oid");
    assert_eq!(new_blobs, 1, "exactly one blobs row for the edited content");
    assert_eq!(new_paths, 3, "Tree + Index + Worktree paths rows for b.md");

    // The page must resolve at its new location.
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
