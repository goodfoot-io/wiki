//! dir_mtimes rows for deleted directories are never pruned — only
//! INSERT/UPDATE, no DELETE. A restored/moved-in directory with a preserved
//! old mtime matching a stale row would be wrongly treated as clean.
//!
//! The test creates a subdirectory, does a cold prepare (which records
//! dir_mtimes for root + ".wiki" + subdir — 3 rows), then deletes the
//! subdirectory and does a warm prepare. The stale subdir row should be
//! removed but the current code only upserts — it never deletes — so
//! `debug_dir_mtimes_count()` still returns 3 instead of 2.

mod common;

use common::FixtureRepo;
use wiki::index::{DocSource, HostileFs, WikiIndex};

#[test]
fn deleted_directory_dir_mtimes_row_is_not_pruned() {
    let repo = FixtureRepo::new();

    // Create a subdirectory with wiki files.
    repo.write_wiki_md("subdir/page1.md", "Page 1", "Summary 1", "Body 1");
    repo.write_wiki_md("subdir/page2.md", "Page 2", "Summary 2", "Body 2");
    repo.git_add(".");
    repo.git_commit("initial");

    // Cold prepare under HostileFs::No so dir_mtimes are recorded.
    // .wiki/ is created by prepare itself, so we see 3 directories:
    // root (""), ".wiki", and "subdir".
    let cold = WikiIndex::prepare_with_fs_class(
        repo.root.as_path(),
        DocSource::WorkingTree,
        HostileFs::No,
    )
    .expect("cold prepare");
    assert_eq!(
        cold.debug_dir_mtimes_count().expect("dir_mtimes count"),
        3,
        "expected 3 dir_mtimes rows: root + .wiki + subdir"
    );

    // Delete the subdirectory entirely from the worktree, stage the
    // deletion, and commit so the fast-triple gate misses on the next
    // prepare.
    std::fs::remove_dir_all(repo.root.join("subdir"))
        .expect("remove_dir_all subdir");
    repo.git_add(".");
    repo.git_commit("delete subdir");

    // Warm prepare. The stale dir_mtimes row for subdir should be pruned
    // but the current code never deletes from dir_mtimes — it only
    // upserts directories that were walked during this pass.
    //
    // With the bug, `debug_dir_mtimes_count()` returns 3 (root + .wiki
    // + stale subdir). A correct implementation would return 2 (root
    // + .wiki only).
    let warm = WikiIndex::prepare_with_fs_class(
        repo.root.as_path(),
        DocSource::WorkingTree,
        HostileFs::No,
    )
    .expect("warm prepare");
    assert_eq!(
        warm.debug_dir_mtimes_count().expect("dir_mtimes count"),
        2,
        "expected 2 dir_mtimes rows after deleting subdir (stale row was \
         not pruned by current code)"
    );
}
