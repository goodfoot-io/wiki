//! The index cache DB is derived data: a schema-version mismatch (older or
//! newer CLI wrote it) or a corrupt file must be discarded and rebuilt
//! transparently, never surfaced as a command failure.

mod common;

use wiki::index::WikiIndex;

fn seeded_repo() -> common::FixtureRepo {
    let repo = common::FixtureRepo::new();
    repo.write_wiki_md("seed.md", "Seed", "Seed summary.", "Seed body.");
    repo.git_add("seed.md");
    repo.git_commit("initial commit");
    repo
}

// ── Target-layout port (plan merged-store-generations, Phase 1) ──────────
//
// The merged store lives at `<git-common-dir>/wiki/store.sqlite`; once the
// freshness rewrite serves generations from it, the rebuild contract moves
// with it — and no `.wiki/` directory may ever appear on any checkout.

#[test]
fn merged_store_corruption_rebuilds_without_creating_wiki_dir() {
    let repo = seeded_repo();

    drop(WikiIndex::prepare(repo.root.as_path()).expect("initial prepare"));

    let db = common::target_db_path(&repo.root);
    assert!(db.exists(), "merged store must exist after prepare");
    // Clobber the DB (sidecars first, so no WAL replays over the garbage).
    for suffix in ["-shm", "-wal"] {
        let mut p = db.clone().into_os_string();
        p.push(suffix);
        let _ = std::fs::remove_file(std::path::PathBuf::from(p));
    }
    std::fs::write(&db, b"this is not a sqlite database").expect("corrupt db file");

    let index = WikiIndex::prepare(repo.root.as_path()).expect("prepare must rebuild, not fail");
    let page = index
        .resolve_page("Seed")
        .expect("resolve_page")
        .expect("Seed present after rebuild");
    assert_eq!(page.title, "Seed");
    // Retirement gate (plan D14): no `.wiki` directory may appear anywhere
    // after any command — every derived byte lives in the merged store and
    // the only wiki-related file in the worktree is the tracked root
    // `.wikiignore`.
    assert!(
        !repo.root.join(".wiki").exists(),
        "no .wiki directory may exist after any command"
    );
}
