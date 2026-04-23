mod support;

use anyhow::Result;
use serde_json::Value;
use support::TestRepo;

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

    let junit = test_repo.mesh_stdout(["stale", "my-mesh", "--format=junit"])?;
    assert!(junit.contains("<testsuite name=\"git-mesh stale\""));
    assert!(junit.contains("failures=\"1\""));
    assert!(junit.contains(
        "<failure message=\"MOVED file1.txt#L1-L5:file2.txt#L10-L15 -&gt; file1.txt#L2-L6:file2.txt#L10-L15\">"
    ));

    let github_actions = test_repo.mesh_stdout(["stale", "my-mesh", "--format=github-actions"])?;
    assert!(github_actions.contains("::warning file=file1.txt,line=1::"));
    assert!(github_actions.contains("mesh my-mesh%3A MOVED"));

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
