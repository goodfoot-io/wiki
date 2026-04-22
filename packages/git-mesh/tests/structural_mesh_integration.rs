mod support;

use anyhow::Result;
use git_mesh::{RangeSpec, remove_mesh, rename_mesh, restore_mesh, show_mesh};

use support::TestRepo;

fn range_spec(path: &str, start: u32, end: u32) -> RangeSpec {
    RangeSpec {
        path: path.to_string(),
        start,
        end,
    }
}

#[test]
fn test_structural_rm() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    let (link_id, _, _) = test_repo.create_link_fixture(
        "rm-link",
        [
            range_spec("file1.txt", 1, 5),
            range_spec("file2.txt", 10, 15),
        ],
    )?;
    test_repo.create_mesh_fixture("my_mesh", "Mesh to remove", &[&link_id])?;
    remove_mesh(&test_repo.repo, "my_mesh")?;
    let result = show_mesh(&test_repo.repo, "my_mesh");
    assert!(result.is_err());
    Ok(())
}

#[test]
fn test_structural_mv() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    let (link_id, _, _) = test_repo.create_link_fixture(
        "mv-link",
        [
            range_spec("file1.txt", 1, 5),
            range_spec("file2.txt", 10, 15),
        ],
    )?;
    test_repo.create_mesh_fixture("old_mesh", "Mesh to rename", &[&link_id])?;
    rename_mesh(&test_repo.repo, "old_mesh", "new_mesh", false)?;

    let result = show_mesh(&test_repo.repo, "old_mesh");
    assert!(result.is_err());

    let mesh = show_mesh(&test_repo.repo, "new_mesh")?;
    assert_eq!(mesh.name, "new_mesh");
    Ok(())
}

#[test]
fn test_structural_restore() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    let (first_link_id, _, _) = test_repo.create_link_fixture(
        "restore-link-a",
        [
            range_spec("file1.txt", 1, 5),
            range_spec("file2.txt", 10, 15),
        ],
    )?;
    let first_commit =
        test_repo.create_mesh_fixture("my_mesh", "Original mesh state", &[&first_link_id])?;
    let (second_link_id, _, _) = test_repo.create_link_fixture(
        "restore-link-b",
        [
            range_spec("file3.txt", 1, 5),
            range_spec("file4.txt", 10, 15),
        ],
    )?;
    let _second_commit = test_repo.create_mesh_fixture(
        "my_mesh",
        "Updated mesh state",
        &[&first_link_id, &second_link_id],
    )?;
    assert_ne!(test_repo.read_ref("refs/meshes/v1/my_mesh")?, first_commit);
    restore_mesh(&test_repo.repo, "my_mesh", "HEAD~1")?;
    let mesh = show_mesh(&test_repo.repo, "my_mesh")?;
    assert_eq!(mesh.name, "my_mesh");
    Ok(())
}
