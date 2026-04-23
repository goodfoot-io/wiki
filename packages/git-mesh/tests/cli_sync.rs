mod support;

use anyhow::Result;
use support::TestRepo;

#[test]
fn cli_doctor_reports_ok_and_broken_meshes() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    test_repo.mesh_stdout([
        "commit",
        "healthy",
        "--link",
        "file1.txt#L1-L5:file2.txt#L10-L15",
        "-m",
        "Healthy mesh",
    ])?;

    // Clean tree: exit 0, summary mentions the mesh count and per-mesh `ok`
    // line carries the link count.
    let ok = test_repo.mesh_output(["doctor"])?;
    assert_eq!(ok.status.code(), Some(0));
    let ok_stdout = String::from_utf8(ok.stdout)?;
    assert!(
        ok_stdout.contains("mesh doctor: checking refs/meshes/v1/*"),
        "missing header: {ok_stdout}"
    );
    assert!(
        ok_stdout.contains("ok      healthy  (1 link)"),
        "missing per-mesh ok line: {ok_stdout}"
    );
    assert!(
        ok_stdout.contains("mesh doctor: ok (1 mesh checked)"),
        "missing summary: {ok_stdout}"
    );

    // Broken tree: exit 2 (tool error), not 1 (which is reserved for
    // `stale --exit-code`). Per-mesh ISSUE line plus a bulleted detail.
    test_repo.create_mesh_fixture("broken", "Broken mesh", &["missing-link"])?;
    let broken = test_repo.mesh_output(["doctor"])?;
    assert_eq!(
        broken.status.code(),
        Some(2),
        "doctor must exit 2 on integrity failures"
    );
    let stdout = String::from_utf8(broken.stdout)?;
    assert!(stdout.contains("ISSUE   broken"), "missing ISSUE line: {stdout}");
    assert!(
        stdout.contains("- mesh is unreadable")
            || stdout.contains("- link `missing-link` is unreadable"),
        "missing integrity detail: {stdout}"
    );
    // Healthy mesh still listed as ok.
    assert!(
        stdout.contains("ok      healthy"),
        "healthy mesh missing from output: {stdout}"
    );
    assert!(
        stdout.contains("mesh doctor: found 1 issue across 1 mesh (1/2 ok)"),
        "missing summary footer: {stdout}"
    );

    Ok(())
}

#[test]
fn cli_doctor_empty_repo_reports_no_meshes() -> Result<()> {
    let test_repo = TestRepo::new()?;
    let output = test_repo.mesh_output(["doctor"])?;
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout)?;
    assert!(
        stdout.contains("mesh doctor: ok (no meshes)"),
        "empty repo should report no meshes: {stdout}"
    );
    Ok(())
}
