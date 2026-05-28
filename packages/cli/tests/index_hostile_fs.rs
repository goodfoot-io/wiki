//! Hostile-FS test: injecting `HostileFs::Yes` must force a full Pass 3 rescan.
//!
//! Contract: `WikiIndex::stats().pass3_full_rescans > 0` after a refresh
//! performed under `HostileFs::Yes`.

mod common;

use wiki::index::{DocSource, HostileFs, WikiIndex};

#[test]
fn hostile_fs_forces_full_pass3_rescan() {
    let repo = common::make_parity_fixture();

    // Open the index with hostile-FS injection.
    let index = WikiIndex::prepare_with_fs_class(
        repo.root.as_path(),
        DocSource::WorkingTree,
        HostileFs::Yes,
    )
    .expect("prepare_with_fs_class");

    let stats = index.stats();
    assert!(
        stats.pass3_full_rescans > 0,
        "expected at least one Pass 3 full rescan under HostileFs::Yes, got {}",
        stats.pass3_full_rescans
    );
}
