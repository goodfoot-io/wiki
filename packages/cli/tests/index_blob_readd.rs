//! A blob whose refcount hits zero mid-refresh (its `blobs` row deleted)
//! must be re-inserted when a later delta in the same refresh re-adds the
//! same OID. This pins the per-refresh blob-cache eviction in
//! `decrement_blob`: without it the cache would still claim the blob
//! exists, the insert would be skipped, and the new path row would
//! reference a missing blob (the page silently vanishes from search).

mod common;

use wiki::index::WikiIndex;
use wiki::index::blob::compute_blob_oid;

#[test]
fn refcount_zero_then_same_oid_readd_in_one_refresh() {
    let repo = common::FixtureRepo::new();

    let bytes = "---\ntitle: Phoenix\nsummary: Dies and returns.\n---\n\nSame bytes, new path.\n";
    let oid = compute_blob_oid(bytes.as_bytes());

    repo.write_file("z.md", bytes);
    repo.git_add("z.md");
    repo.git_commit("add z.md");
    drop(WikiIndex::prepare(repo.root.as_path()).expect("prepare full"));

    // `git rm` drops the index and worktree rows; only (z.md, Tree) remains,
    // so the blob's refcount is exactly 1 going into the final refresh.
    repo.git_rm("z.md");
    drop(WikiIndex::prepare(repo.root.as_path()).expect("prepare tree-only"));

    // One refresh now sees: Tree removal of z.md (refcount 1 -> 0, blob row
    // deleted), then Index + Worktree additions of b.md with the same OID.
    repo.git_commit("remove z.md");
    repo.write_file("b.md", bytes);
    repo.git_add("b.md");
    let index = WikiIndex::prepare(repo.root.as_path()).expect("prepare re-add");

    let (blobs, paths) = index
        .debug_blob_path_counts(&oid.0)
        .expect("debug_blob_path_counts");
    assert_eq!(blobs, 1, "blob row re-inserted after mid-refresh deletion");
    assert_eq!(paths, 2, "Index + Worktree paths rows for b.md");

    let page = index
        .resolve_page("Phoenix")
        .expect("resolve_page")
        .expect("page must survive the path move");
    assert!(
        page.file.ends_with("b.md"),
        "resolves to b.md: {}",
        page.file
    );
}
