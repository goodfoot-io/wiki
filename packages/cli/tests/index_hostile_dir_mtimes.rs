//! Hostile-FS dir_mtimes test: `dir_mtimes` must NOT be populated under
//! `HostileFs::Yes` because they are never consulted — `is_clean` is always
//! `false` when `is_hostile` is true (see `pass_worktree` at lines 146-147).
//!
//! Contract: after a `HostileFs::Yes` refresh, `debug_dir_mtimes_count()` == 0.
//! The current unfixed code unconditionally collects and upserts mtimes in the
//! `else` branch (lines 160-162, 242-250), so this test MUST FAIL until the
//! collection is gated behind `!is_hostile`.

mod common;

use common::FixtureRepo;
use wiki::index::{DocSource, HostileFs, WikiIndex};

#[test]
fn hostile_fs_does_not_write_dir_mtimes() {
    let repo = FixtureRepo::new();

    // Create some wiki pages in subdirectories so we get multiple dir_mtimes rows.
    for d in 0..3 {
        for p in 0..3 {
            repo.write_wiki_md(
                &format!("dir{d}/page{p}.md"),
                &format!("Page {d}-{p}"),
                &format!("Summary {d}-{p}"),
                &format!("Body {d}-{p}"),
            );
        }
    }
    repo.git_add(".");
    repo.git_commit("initial");

    // Refresh under HostileFs::Yes.
    let index = WikiIndex::prepare_with_fs_class(
        repo.root.as_path(),
        DocSource::WorkingTree,
        HostileFs::Yes,
    )
    .expect("prepare_with_fs_class");

    let count = index
        .debug_dir_mtimes_count()
        .expect("debug_dir_mtimes_count");

    assert_eq!(
        count, 0,
        "dir_mtimes should be empty after a HostileFs::Yes refresh, \
         got {count} rows. The collection in pass_worktree is \
         unconditional in the 'else' branch (lines 160-162) and must be \
         gated behind !is_hostile."
    );
}
