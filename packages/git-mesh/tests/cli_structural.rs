//! CLI: restore, revert, delete, mv, doctor (§6.7, §6.8).

mod support;

use anyhow::Result;
use git_mesh::validation::RESERVED_MESH_NAMES;
use support::{BareRepo, TestRepo};

fn seed(repo: &TestRepo, name: &str) -> Result<()> {
    repo.mesh_stdout(["add", name, "file1.txt#L1-L5"])?;
    repo.mesh_stdout(["message", name, "-m", "seed"])?;
    repo.mesh_stdout(["commit", name])?;
    Ok(())
}

#[test]

fn restore_clears_staging() -> Result<()> {
    let repo = TestRepo::seeded()?;
    repo.mesh_stdout(["add", "m", "file1.txt#L1-L5"])?;
    repo.mesh_stdout(["restore", "m"])?;
    let out = repo.run_mesh(["commit", "m"])?;
    assert!(!out.status.success(), "no-op commit should fail");
    Ok(())
}

#[test]

fn revert_creates_new_tip() -> Result<()> {
    let repo = TestRepo::seeded()?;
    seed(&repo, "rev")?;
    let first_oid = repo.git_stdout(["rev-parse", "refs/meshes/v1/rev"])?;
    repo.mesh_stdout(["add", "rev", "file2.txt#L1-L3"])?;
    repo.mesh_stdout(["message", "rev", "-m", "v2"])?;
    repo.mesh_stdout(["commit", "rev"])?;
    repo.mesh_stdout(["revert", "rev", &first_oid])?;
    let new_tip = repo.git_stdout(["rev-parse", "refs/meshes/v1/rev"])?;
    assert_ne!(new_tip, first_oid, "revert is fast-forward, not rewind");
    Ok(())
}

#[test]

fn delete_removes_ref() -> Result<()> {
    let repo = TestRepo::seeded()?;
    seed(&repo, "gone")?;
    repo.mesh_stdout(["delete", "gone"])?;
    assert!(!repo.ref_exists("refs/meshes/v1/gone"));
    Ok(())
}

#[test]

fn mv_renames_ref() -> Result<()> {
    let repo = TestRepo::seeded()?;
    seed(&repo, "oldn")?;
    repo.mesh_stdout(["mv", "oldn", "newn"])?;
    assert!(repo.ref_exists("refs/meshes/v1/newn"));
    assert!(!repo.ref_exists("refs/meshes/v1/oldn"));
    Ok(())
}

#[test]

fn mv_rejects_reserved_name() -> Result<()> {
    let repo = TestRepo::seeded()?;
    seed(&repo, "oldn")?;
    let out = repo.run_mesh(["mv", "oldn", "delete"])?;
    assert!(!out.status.success());
    Ok(())
}

#[test]

fn every_reserved_name_rejected_on_create() -> Result<()> {
    // §10.2 reserved list.
    let repo = TestRepo::seeded()?;
    for &name in RESERVED_MESH_NAMES {
        let out = repo.run_mesh(["add", name, "file1.txt#L1-L5"])?;
        assert!(!out.status.success(), "reserved name `{name}` was accepted");
    }
    Ok(())
}

#[test]

fn doctor_runs_clean_on_fresh_repo() -> Result<()> {
    let repo = TestRepo::seeded()?;
    let out = repo.run_mesh(["doctor"])?;
    assert_eq!(out.status.code(), Some(0));
    Ok(())
}

#[test]

fn doctor_flags_missing_refspec() -> Result<()> {
    let repo = TestRepo::seeded()?;
    let bare = BareRepo::new()?;
    repo.add_remote("origin", bare.path())?;
    // origin has no mesh refspec — doctor should report a finding.
    let out = repo.run_mesh(["doctor"])?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let combined = format!("{stdout}{stderr}");
    assert!(combined.contains("refspec") || combined.to_lowercase().contains("remote"));
    Ok(())
}
