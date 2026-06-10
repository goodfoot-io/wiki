//! Pass 3 dir-mtime Merkle short-circuit: when one wiki file is edited,
//! only that file's directory should be re-walked on the next refresh.
//!
//! The test forces `HostileFs::No` so the merkle path is exercised even
//! when the underlying filesystem (Docker / CI overlayfs) would otherwise
//! classify as hostile.

mod common;

use common::FixtureRepo;
use wiki::index::{DocSource, HostileFs, WikiIndex};

#[test]
fn warm_refresh_walks_only_changed_dir() {
    let repo = FixtureRepo::new();

    // 4 subdirectories, 5 wiki pages each = 20 docs.
    for d in 0..4 {
        for p in 0..5 {
            repo.write_wiki_md(
                &format!("docs{d}/page{p}.md"),
                &format!("Page {d}-{p}"),
                &format!("Summary {d}-{p}"),
                &format!("Body {d}-{p}"),
            );
        }
    }
    repo.git_add(".");
    repo.git_commit("initial");

    // Cold prepare under HostileFs::No so dir_mtimes are recorded.
    let _cold = WikiIndex::prepare_with_fs_class(
        repo.root.as_path(),
        DocSource::WorkingTree,
        HostileFs::No,
    )
    .expect("cold prepare");

    // Ensure a distinct mtime when we edit (FS mtime granularity safety).
    std::thread::sleep(std::time::Duration::from_millis(20));

    // Edit one page (in docs1/) and stage a sentinel so the fast triple
    // misses on the next prepare. sentinel.txt goes in the root, which
    // dirties the root directory's mtime too — that's expected.
    repo.write_wiki_md(
        "docs1/page2.md",
        "Page 1-2",
        "Summary 1-2 edited",
        "Body 1-2 edited",
    );
    repo.write_file("sentinel.txt", "1");
    repo.git_add("sentinel.txt");

    let warm = WikiIndex::prepare_with_fs_class(
        repo.root.as_path(),
        DocSource::WorkingTree,
        HostileFs::No,
    )
    .expect("warm prepare");
    let stats = warm.stats();

    // 5 populated dirs total: root + docs0..docs3. Two are dirty (root
    // because of sentinel.txt, docs1 because of the page2.md edit). The
    // other three (docs0, docs2, docs3) must short-circuit.
    assert!(
        stats.pass3_dir_walks > 0,
        "expected at least one dir walk, got {}",
        stats.pass3_dir_walks
    );
    assert!(
        stats.pass3_dir_walks < 5,
        "expected fewer than 5 dir walks (clean dirs skip stat/hash), got {}",
        stats.pass3_dir_walks
    );

    // And pass3_full_rescans must stay at 0 — we forced HostileFs::No.
    assert_eq!(stats.pass3_full_rescans, 0);
}

/// In-place content edit inside a directory whose mtime does not change
/// (POSIX dir mtime only changes on entry add/remove/rename, not on
/// content edits) must still be picked up by the next refresh.
///
/// Reproduces the bug where `warm_refresh_walks_only_changed_dir` passes
/// but the edited page's content is stale because its parent directory
/// was wrongly classified as clean.
#[test]
fn in_place_edit_in_clean_dir_is_still_indexed() {
    let repo = FixtureRepo::new();

    repo.write_wiki_md(
        "docs/deep.md",
        "Deep",
        "Original summary.",
        "Original body.",
    );
    repo.git_add(".");
    repo.git_commit("initial");

    // Cold prepare under HostileFs::No so dir_mtimes are recorded.
    let _cold = WikiIndex::prepare_with_fs_class(
        repo.root.as_path(),
        DocSource::WorkingTree,
        HostileFs::No,
    )
    .expect("cold prepare");

    // Ensure a distinct file mtime (FS granularity safety).
    std::thread::sleep(std::time::Duration::from_millis(20));

    // In-place content edit — does NOT change docs/ dir mtime on POSIX.
    repo.write_wiki_md(
        "docs/deep.md",
        "Deep",
        "Edited summary.",
        "Edited body.",
    );

    // Stage a root-level sentinel so the fast triple misses on the next
    // prepare. This dirties root's mtime but docs/ stays unchanged.
    repo.write_file("sentinel.txt", "1");
    repo.git_add("sentinel.txt");

    let warm = WikiIndex::prepare_with_fs_class(
        repo.root.as_path(),
        DocSource::WorkingTree,
        HostileFs::No,
    )
    .expect("warm prepare");

    let resolved = warm
        .resolve_page("Deep")
        .expect("resolve_page")
        .expect("page must exist");

    assert_eq!(resolved.summary, "Edited summary.");
    assert_eq!(resolved.content, "Edited body.\n");
}
