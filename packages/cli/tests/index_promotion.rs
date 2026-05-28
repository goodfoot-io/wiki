//! Promotion test: an untracked file promoted to the index via `git add` must
//! share the same `blobs` row (same OID) rather than creating a duplicate.
//!
//! Contract after `git add foo.md` + refresh:
//!   - exactly one `blobs` row for the content OID
//!   - two `paths` rows pointing at that OID: `Source::Index` and `Source::Worktree`

mod common;

use wiki::index::{DocSource, WikiIndex};

#[test]
#[ignore = "tdd-bootstrap"]
fn untracked_promoted_to_index_shares_blob() {
    let repo = common::FixtureRepo::new();

    // Need at least one commit so HEAD exists.
    repo.write_wiki_md("seed.md", "Seed", "Seed summary.", "Seed body.");
    repo.git_add("seed.md");
    repo.git_commit("initial commit");

    // Write an untracked wiki page.
    repo.write_wiki_md("foo.md", "Foo Page", "The foo summary.", "Foo body.");

    // First refresh: foo.md appears as Source::Worktree.
    let _before = WikiIndex::prepare(repo.root.as_path()).expect("prepare before add");

    // Stage foo.md.
    repo.git_add("foo.md");

    // Second refresh: foo.md now has both Source::Index and Source::Worktree rows.
    let _after = WikiIndex::prepare_for_source(repo.root.as_path(), DocSource::Index)
        .expect("prepare after add");

    // Phase 3 exposes a method to count blobs and paths rows for an OID.
    // Assert: one blobs row, two paths rows.
    unimplemented!("Phase 3: assert one blobs row + two paths rows for foo.md OID")
}
