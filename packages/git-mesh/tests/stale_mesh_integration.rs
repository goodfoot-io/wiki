mod support;

use anyhow::Result;
use git_mesh::{LinkStatus, RangeSpec, stale_mesh};

use support::TestRepo;

fn range_spec(path: &str, start: u32, end: u32) -> RangeSpec {
    RangeSpec {
        path: path.to_string(),
        start,
        end,
    }
}

#[test]
fn test_stale_mesh_fresh() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    let (link_id, _, _) = test_repo.create_link_fixture(
        "fresh-link",
        [
            range_spec("file1.txt", 1, 5),
            range_spec("file2.txt", 10, 15),
        ],
    )?;
    test_repo.create_mesh_fixture("my_mesh", "Fresh mesh fixture", &[&link_id])?;

    let resolved = stale_mesh(&test_repo.repo, "my_mesh")?;
    assert!(!resolved.links.is_empty());
    assert_eq!(resolved.links[0].status, LinkStatus::Fresh);
    Ok(())
}

#[test]
fn test_stale_mesh_moved() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    let (link_id, _, _) = test_repo.create_link_fixture(
        "moved-link",
        [
            range_spec("file1.txt", 1, 5),
            range_spec("file2.txt", 10, 15),
        ],
    )?;
    test_repo.create_mesh_fixture("my_mesh", "Moved mesh fixture", &[&link_id])?;
    test_repo.write_file("file1.txt", "new_line_here\n1\n2\n3\n4\n5\n")?;

    let resolved = stale_mesh(&test_repo.repo, "my_mesh")?;
    assert!(!resolved.links.is_empty());
    assert_eq!(resolved.links[0].status, LinkStatus::Moved);
    Ok(())
}

#[test]
fn test_stale_mesh_modified() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    let (link_id, _, _) = test_repo.create_link_fixture(
        "modified-link",
        [
            range_spec("file1.txt", 1, 5),
            range_spec("file2.txt", 10, 15),
        ],
    )?;
    test_repo.create_mesh_fixture("my_mesh", "Modified mesh fixture", &[&link_id])?;
    test_repo.write_file("file1.txt", "1\n2\nMODIFIED\n4\n5\n")?;

    let resolved = stale_mesh(&test_repo.repo, "my_mesh")?;
    assert!(!resolved.links.is_empty());
    assert_eq!(resolved.links[0].status, LinkStatus::Modified);
    Ok(())
}

#[test]
fn test_stale_mesh_rewritten() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    let (link_id, _, _) = test_repo.create_link_fixture(
        "rewritten-link",
        [
            range_spec("file1.txt", 1, 5),
            range_spec("file2.txt", 10, 15),
        ],
    )?;
    test_repo.create_mesh_fixture("my_mesh", "Rewritten mesh fixture", &[&link_id])?;
    test_repo.write_file("file1.txt", "MOD\nMOD\nMOD\nMOD\nMOD\n")?;

    let resolved = stale_mesh(&test_repo.repo, "my_mesh")?;
    assert!(!resolved.links.is_empty());
    assert_eq!(resolved.links[0].status, LinkStatus::Rewritten);
    Ok(())
}

#[test]
fn test_stale_mesh_missing() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    let (link_id, _, _) = test_repo.create_link_fixture(
        "missing-link",
        [
            range_spec("file1.txt", 1, 5),
            range_spec("file2.txt", 10, 15),
        ],
    )?;
    test_repo.create_mesh_fixture("my_mesh", "Missing mesh fixture", &[&link_id])?;
    test_repo.remove_file("file1.txt")?;

    let resolved = stale_mesh(&test_repo.repo, "my_mesh")?;
    assert!(!resolved.links.is_empty());
    assert_eq!(resolved.links[0].status, LinkStatus::Missing);
    Ok(())
}
