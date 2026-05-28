//! Rename test: a `git mv` must not re-tokenize the FTS row.
//!
//! Contract: after `git mv foo.md bar.md` + commit + refresh, the `blobs` row
//! for the content is unchanged (same OID, refcount = 1) and
//! `IndexStats::fts_retokenizations` remains 0.

mod common;

use wiki::index::WikiIndex;

#[test]
fn rename_preserves_blob_and_skips_retokenization() {
    let repo = common::FixtureRepo::new();

    // Commit foo.md as a wiki page.
    repo.write_wiki_md("foo.md", "Foo Page", "The foo summary.", "Foo body.");
    repo.git_add("foo.md");
    repo.git_commit("add foo.md");

    // Prime the index.
    let _index = WikiIndex::prepare(repo.root.as_path()).expect("first prepare");

    // Rename and commit.
    repo.git_mv("foo.md", "bar.md");
    repo.git_commit("rename foo.md -> bar.md");

    // Refresh the index.
    let index_after = WikiIndex::prepare(repo.root.as_path()).expect("second prepare");

    let stats = index_after.stats();
    assert_eq!(
        stats.fts_retokenizations, 0,
        "rename must not retokenize: got {} retokenizations",
        stats.fts_retokenizations
    );
}
