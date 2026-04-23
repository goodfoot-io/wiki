mod support;

use anyhow::Result;
use serde_json::Value;
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
fn cli_stale_supports_exit_code_and_machine_formats() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    test_repo.mesh_stdout([
        "commit",
        "my-mesh",
        "--link",
        "file1.txt#L1-L5:file2.txt#L10-L15",
        "-m",
        "Track ranges",
    ])?;

    let fresh = test_repo.mesh_output(["stale", "my-mesh", "--exit-code"])?;
    assert_eq!(fresh.status.code(), Some(0));

    test_repo.write_file(
        "file1.txt",
        "prefix\nline1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n",
    )?;
    test_repo.commit_all("shift file1 lines")?;

    let stale = test_repo.mesh_output(["stale", "my-mesh", "--exit-code"])?;
    assert_eq!(stale.status.code(), Some(1));

    let porcelain = test_repo.mesh_stdout(["stale", "my-mesh", "--format=porcelain"])?;
    assert!(porcelain.contains("mesh=my-mesh"));
    assert!(porcelain.contains("status=MOVED"));
    assert!(porcelain.contains("pair=file1.txt#L1-L5:file2.txt#L10-L15"));
    assert!(porcelain.contains("currentPair=file1.txt#L2-L6:file2.txt#L10-L15"));

    let json = test_repo.mesh_stdout(["stale", "my-mesh", "--format=json"])?;
    let payload: Value = serde_json::from_str(&json)?;
    assert_eq!(payload["version"], 1);
    assert_eq!(payload["meshes"][0]["name"], "my-mesh");
    assert_eq!(payload["meshes"][0]["stale_count"], 1);
    assert_eq!(
        payload["meshes"][0]["links"][0]["pair"],
        "file1.txt#L1-L5:file2.txt#L10-L15"
    );
    assert_eq!(
        payload["meshes"][0]["links"][0]["current_pair"],
        "file1.txt#L2-L6:file2.txt#L10-L15"
    );
    assert_eq!(payload["meshes"][0]["links"][0]["status"], "MOVED");

    Ok(())
}

#[test]
fn cli_stale_includes_culprit_and_reconcile_data() -> Result<()> {
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
        "line1\nline2\nupdated\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n",
    )?;
    test_repo.commit_all("modify file1")?;

    let human = test_repo.mesh_stdout(["stale", "my-mesh"])?;
    assert!(human.contains("caused by"));
    assert!(human.contains("modify file1"));
    assert!(human.contains("reconcile with:"));
    assert!(human.contains("git mesh commit my-mesh --unlink"));

    let porcelain = test_repo.mesh_stdout(["stale", "my-mesh", "--format=porcelain"])?;
    assert!(porcelain.contains("reconcile=git mesh commit my-mesh --unlink"));
    assert!(porcelain.contains("leftCulprit="));
    assert!(porcelain.contains("modify file1"));

    let json = test_repo.mesh_stdout(["stale", "my-mesh", "--format=json"])?;
    let payload: Value = serde_json::from_str(&json)?;
    assert_eq!(
        payload["meshes"][0]["links"][0]["reconcile_command"],
        "git mesh commit my-mesh --unlink file1.txt#L1-L5:file2.txt#L10-L15 --link file1.txt#L1-L5:file2.txt#L10-L15 -m \"...\""
    );
    assert_eq!(
        payload["meshes"][0]["links"][0]["sides"][0]["culprit"]["summary"],
        "modify file1"
    );

    Ok(())
}

#[test]
fn cli_stale_without_name_scans_all_meshes_and_since_filters_links() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    let before = test_repo.head_sha()?;

    test_repo.mesh_stdout([
        "commit",
        "old-mesh",
        "--link",
        "file1.txt#L1-L5:file2.txt#L10-L15",
        "-m",
        "Older mesh",
    ])?;

    test_repo.write_file("new-anchor.txt", "a\nb\nc\nd\ne\nf\n")?;
    test_repo.commit_all("add new anchor file")?;
    let since = test_repo.head_sha()?;

    test_repo.mesh_stdout([
        "commit",
        "new-mesh",
        "--link",
        "new-anchor.txt#L1-L5:file3.txt#L1-L5",
        "-m",
        "Newer mesh",
    ])?;

    let scan_all = test_repo.mesh_stdout(["stale", "--format=porcelain"])?;
    assert!(scan_all.contains("mesh=old-mesh"));
    assert!(scan_all.contains("mesh=new-mesh"));

    let filtered = test_repo.mesh_stdout(["stale", "--format=porcelain", "--since", &since])?;
    assert!(!filtered.contains("mesh=old-mesh"));
    assert!(filtered.contains("mesh=new-mesh"));

    let filtered_old =
        test_repo.mesh_stdout(["stale", "old-mesh", "--format=json", "--since", &since])?;
    let payload: Value = serde_json::from_str(&filtered_old)?;
    assert_eq!(payload["meshes"][0]["name"], "old-mesh");
    assert_eq!(payload["meshes"][0]["link_count"], 0);

    let unfiltered_old =
        test_repo.mesh_stdout(["stale", "old-mesh", "--format=json", "--since", &before])?;
    let payload: Value = serde_json::from_str(&unfiltered_old)?;
    assert_eq!(payload["meshes"][0]["link_count"], 1);

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
