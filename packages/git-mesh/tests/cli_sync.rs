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

    let ok = test_repo.mesh_stdout(["doctor"])?;
    assert!(ok.contains("mesh doctor: ok"));

    test_repo.create_mesh_fixture("broken", "Broken mesh", &["missing-link"])?;
    let broken = test_repo.mesh_output(["doctor"])?;
    assert_eq!(broken.status.code(), Some(1));
    let stdout = String::from_utf8(broken.stdout)?;
    assert!(stdout.contains("mesh doctor: found 1 issue(s)"));
    assert!(stdout.contains("mesh `broken` is unreadable"));

    Ok(())
}
