mod support;

use anyhow::Result;
use support::TestRepo;

#[test]
fn cli_rm_mv_and_restore_work() -> Result<()> {
    let test_repo = TestRepo::new()?;
    test_repo.mesh_stdout([
        "commit",
        "mesh-a",
        "--link",
        "file1.txt#L1-L5:file2.txt#L10-L15",
        "-m",
        "Original state",
    ])?;
    let first_tip = test_repo.read_ref("refs/meshes/v1/mesh-a")?;

    test_repo.mesh_stdout([
        "commit",
        "mesh-a",
        "--link",
        "file3.txt#L1-L5:file4.txt#L10-L15",
        "-m",
        "Expanded state",
    ])?;

    test_repo.mesh_stdout(["restore", "mesh-a", "HEAD~1"])?;
    let restored_show = test_repo.mesh_stdout(["mesh-a"])?;
    assert!(restored_show.contains("Original state"));
    assert_ne!(test_repo.read_ref("refs/meshes/v1/mesh-a")?, first_tip);

    test_repo.mesh_stdout(["mv", "mesh-a", "mesh-b"])?;
    let renamed_show = test_repo.mesh_stdout(["mesh-b"])?;
    assert!(renamed_show.contains("mesh mesh-b"));
    let old_err = test_repo.mesh_stderr(["mesh-a"])?;
    assert!(old_err.contains("error:"));

    test_repo.mesh_stdout(["rm", "mesh-b"])?;
    let deleted_err = test_repo.mesh_stderr(["mesh-b"])?;
    assert!(deleted_err.contains("error:"));

    Ok(())
}

#[test]
fn cli_rejects_reserved_names() -> Result<()> {
    let test_repo = TestRepo::new()?;
    let stderr = test_repo.mesh_stderr([
        "commit",
        "stale",
        "--link",
        "file1.txt#L1-L5:file2.txt#L10-L15",
        "-m",
        "bad",
    ])?;
    assert!(stderr.contains("mesh name `stale` is reserved"));

    let mv_stderr = test_repo.mesh_stderr(["mv", "missing", "doctor"])?;
    assert!(mv_stderr.contains("mesh name `doctor` is reserved"));

    Ok(())
}
