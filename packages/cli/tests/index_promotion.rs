//! Promotion test: an untracked file promoted to the index via `git add` must
//! share the same `blobs` row (same OID) rather than creating a duplicate.

mod common;

use wiki::index::WikiIndex;
use wiki::index::blob::compute_blob_oid;

#[test]
fn untracked_promoted_to_index_shares_blob() {
    let repo = common::FixtureRepo::new();

    repo.write_wiki_md("seed.md", "Seed", "Seed summary.", "Seed body.");
    repo.git_add("seed.md");
    repo.git_commit("initial commit");

    let bytes = "---\ntitle: Foo Page\nsummary: The foo summary.\n---\n\nFoo body.\n";
    repo.write_file("foo.md", bytes);
    let oid = compute_blob_oid(bytes.as_bytes());

    let _before = WikiIndex::prepare(repo.root.as_path()).expect("prepare before add");
    repo.git_add("foo.md");
    let after = WikiIndex::prepare(repo.root.as_path()).expect("prepare after add");

    let (blobs, paths) = after
        .debug_blob_path_counts(&oid.0)
        .expect("debug_blob_path_counts");
    assert_eq!(blobs, 1, "exactly one blobs row for foo.md OID");
    assert_eq!(paths, 2, "Index + Worktree paths rows for foo.md OID");
}
