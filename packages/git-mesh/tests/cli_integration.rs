mod support;

use anyhow::Result;
use support::TestRepo;

#[test]
fn cli_lists_and_shows_meshes() -> Result<()> {
    let test_repo = TestRepo::new()?;
    test_repo.mesh_stdout([
        "commit",
        "alpha",
        "--link",
        "file1.txt#L1-L5:file2.txt#L10-L15",
        "-m",
        "Alpha subject",
    ])?;
    test_repo.mesh_stdout([
        "commit",
        "beta",
        "--link",
        "file3.txt#L1-L5:file4.txt#L10-L15",
        "-m",
        "Beta subject",
    ])?;

    let list = test_repo.mesh_stdout(std::iter::empty::<&str>())?;
    assert!(list.contains("alpha\t1 links\tAlpha subject"));
    assert!(list.contains("beta\t1 links\tBeta subject"));

    let show = test_repo.mesh_stdout(["alpha"])?;
    assert!(show.contains("mesh alpha"));
    assert!(show.contains("Alpha subject"));
    assert!(show.contains("Links (1):"));

    Ok(())
}

#[test]
fn cli_stale_reports_drift() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    test_repo.mesh_stdout([
        "commit",
        "my-mesh",
        "--link",
        "file1.txt#L1-L5:file2.txt#L10-L15",
        "-m",
        "Track ranges",
    ])?;

    test_repo.write_file(
        "file1.txt",
        "prefix\nline1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n",
    )?;
    test_repo.commit_all("shift file1 lines")?;

    let stale = test_repo.mesh_stdout(["stale", "my-mesh"])?;
    assert!(stale.contains("1 stale of 1 links"));
    assert!(stale.contains("MOVED"));
    assert!(stale.contains("file1.txt#L1-L5"));

    Ok(())
}

#[test]
fn cli_commit_supports_unlink_amend_and_anchor() -> Result<()> {
    let test_repo = TestRepo::new()?;
    let anchor = test_repo.head_sha()?;

    test_repo.mesh_stdout([
        "commit",
        "my-mesh",
        "--anchor",
        &anchor,
        "--link",
        "file1.txt#L1-L5:file2.txt#L10-L15",
        "-m",
        "Initial message",
    ])?;

    let initial_show = test_repo.mesh_stdout(["my-mesh"])?;
    assert!(initial_show.contains("Initial message"));

    test_repo.mesh_stdout([
        "commit",
        "my-mesh",
        "--unlink",
        "file1.txt#L1-L5:file2.txt#L10-L15",
        "--link",
        "file1.txt#L2-L6:file2.txt#L10-L15",
        "-m",
        "Reconcile drift",
    ])?;

    let reconciled_show = test_repo.mesh_stdout(["my-mesh"])?;
    assert!(reconciled_show.contains("Reconcile drift"));

    test_repo.mesh_stdout(["commit", "my-mesh", "--amend", "-m", "Reworded message"])?;

    let amended_show = test_repo.mesh_stdout(["my-mesh"])?;
    assert!(amended_show.contains("Reworded message"));

    Ok(())
}

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
    Ok(())
}
