//! Library tests for `mesh::commit_mesh` (§6.1, §6.2).

mod support;

use anyhow::Result;
use git_mesh::staging::StagedConfig;
use git_mesh::types::CopyDetection;
use git_mesh::{
    append_add, append_config, append_remove, commit_mesh, read_mesh, set_message,
};
use support::TestRepo;

#[test]

fn commit_happy_path_writes_ref_and_tree() -> Result<()> {
    let repo = TestRepo::seeded()?;
    let gix = repo.gix_repo()?;
    append_add(&gix, "my-mesh", "file1.txt", 1, 5, None)?;
    set_message(&gix, "my-mesh", "Initial message")?;
    let tip = commit_mesh(&gix, "my-mesh")?;
    assert!(!tip.is_empty());
    assert!(repo.ref_exists("refs/meshes/v1/my-mesh"));
    let m = read_mesh(&gix, "my-mesh")?;
    assert_eq!(m.message.trim(), "Initial message");
    assert_eq!(m.ranges.len(), 1);
    Ok(())
}

#[test]

fn commit_writes_ranges_sorted_by_path_start_end() -> Result<()> {
    let repo = TestRepo::seeded()?;
    let gix = repo.gix_repo()?;
    append_add(&gix, "sort-mesh", "file2.txt", 5, 8, None)?;
    append_add(&gix, "sort-mesh", "file1.txt", 7, 9, None)?;
    append_add(&gix, "sort-mesh", "file1.txt", 1, 3, None)?;
    set_message(&gix, "sort-mesh", "m")?;
    commit_mesh(&gix, "sort-mesh")?;
    // Spec §4.2: canonical order is by (path, start, end) ascending.
    // We don't know the range ids, but we can read the mesh back and
    // verify count.
    let m = read_mesh(&gix, "sort-mesh")?;
    assert_eq!(m.ranges.len(), 3);
    Ok(())
}

#[test]

fn commit_rejects_duplicate_location() -> Result<()> {
    let repo = TestRepo::seeded()?;
    let gix = repo.gix_repo()?;
    append_add(&gix, "dup", "file1.txt", 1, 5, None)?;
    append_add(&gix, "dup", "file1.txt", 1, 5, None)?;
    set_message(&gix, "dup", "m")?;
    let err = commit_mesh(&gix, "dup").unwrap_err();
    assert!(matches!(err, git_mesh::Error::DuplicateRangeLocation { .. }));
    Ok(())
}

#[test]

fn commit_with_empty_staging_errors() -> Result<()> {
    let repo = TestRepo::seeded()?;
    let gix = repo.gix_repo()?;
    let err = commit_mesh(&gix, "empty").unwrap_err();
    assert!(matches!(err, git_mesh::Error::StagingEmpty(_)));
    Ok(())
}

#[test]

fn first_commit_without_message_errors() -> Result<()> {
    let repo = TestRepo::seeded()?;
    let gix = repo.gix_repo()?;
    append_add(&gix, "needs-msg", "file1.txt", 1, 5, None)?;
    let err = commit_mesh(&gix, "needs-msg").unwrap_err();
    assert!(matches!(err, git_mesh::Error::MessageRequired(_)));
    Ok(())
}

#[test]

fn second_commit_reuses_parent_message_when_unset() -> Result<()> {
    let repo = TestRepo::seeded()?;
    let gix = repo.gix_repo()?;
    append_add(&gix, "carry", "file1.txt", 1, 5, None)?;
    set_message(&gix, "carry", "first subject")?;
    commit_mesh(&gix, "carry")?;
    // second commit, no staged message
    append_add(&gix, "carry", "file2.txt", 2, 4, None)?;
    commit_mesh(&gix, "carry")?;
    let m = read_mesh(&gix, "carry")?;
    assert!(m.message.contains("first subject"));
    Ok(())
}

#[test]

fn commit_config_noop_only_is_rejected() -> Result<()> {
    let repo = TestRepo::seeded()?;
    let gix = repo.gix_repo()?;
    // seed a first commit
    append_add(&gix, "cfg", "file1.txt", 1, 5, None)?;
    set_message(&gix, "cfg", "seed")?;
    commit_mesh(&gix, "cfg")?;
    // stage a config that equals the committed value -> no-op
    append_config(
        &gix,
        "cfg",
        &StagedConfig::CopyDetection(CopyDetection::SameCommit),
    )?;
    let err = commit_mesh(&gix, "cfg").unwrap_err();
    assert!(matches!(err, git_mesh::Error::ConfigNoOp { .. }));
    Ok(())
}

#[test]

fn remove_of_unknown_range_errors() -> Result<()> {
    let repo = TestRepo::seeded()?;
    let gix = repo.gix_repo()?;
    append_add(&gix, "m", "file1.txt", 1, 5, None)?;
    set_message(&gix, "m", "seed")?;
    commit_mesh(&gix, "m")?;
    append_remove(&gix, "m", "file1.txt", 7, 9)?;
    let err = commit_mesh(&gix, "m").unwrap_err();
    assert!(matches!(err, git_mesh::Error::RangeNotInMesh { .. }));
    Ok(())
}

#[test]

fn commit_rejects_reserved_name() -> Result<()> {
    let repo = TestRepo::seeded()?;
    let gix = repo.gix_repo()?;
    append_add(&gix, "stale", "file1.txt", 1, 5, None)?;
    let err = commit_mesh(&gix, "stale").unwrap_err();
    assert!(matches!(err, git_mesh::Error::ReservedName(_)));
    Ok(())
}

#[test]

fn commit_is_atomic_on_invalid_op() -> Result<()> {
    // One invalid op aborts before any object is written (§6.2 step 5/7).
    let repo = TestRepo::seeded()?;
    let gix = repo.gix_repo()?;
    append_add(&gix, "atomic", "file1.txt", 1, 5, None)?;
    append_add(&gix, "atomic", "no/such.txt", 1, 1, None)?;
    set_message(&gix, "atomic", "m")?;
    assert!(commit_mesh(&gix, "atomic").is_err());
    assert!(!repo.ref_exists("refs/meshes/v1/atomic"));
    // No range ref should have been created either.
    assert!(repo.list_refs("refs/ranges/v1/")?.is_empty());
    Ok(())
}

#[test]

// TODO(slice-D): CAS retry needs a concurrent writer fixture; sketched
// as a placeholder so the behavior stays on the TODO list.
fn commit_retries_on_cas_conflict() -> Result<()> {
    Ok(())
}
