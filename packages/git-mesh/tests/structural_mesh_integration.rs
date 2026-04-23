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
    let previous_tip = test_repo.read_ref("refs/meshes/v1/my_mesh")?;
    assert_ne!(previous_tip, first_commit);
    restore_mesh(&test_repo.repo, "my_mesh", "HEAD~1")?;
    let restored_tip = test_repo.read_ref("refs/meshes/v1/my_mesh")?;
    let mesh = show_mesh(&test_repo.repo, "my_mesh")?;
    assert_eq!(mesh.name, "my_mesh");
    assert_eq!(mesh.links, vec![first_link_id.clone()]);
    assert_eq!(mesh.message, "Original mesh state");
    assert_ne!(restored_tip, first_commit);
    assert_ne!(restored_tip, previous_tip);
    assert_eq!(test_repo.commit_parents(&restored_tip)?, vec![previous_tip]);
    assert_eq!(
        test_repo.git_output(["show", "-s", "--format=%T", &restored_tip])?,
        test_repo.git_output(["show", "-s", "--format=%T", &first_commit])?
    );
    Ok(())
}

#[test]
fn test_structural_remove_mesh_fails_cleanly_on_stale_tip() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    let (link_id, _, _) = test_repo.create_link_fixture(
        "rm-race-link",
        [
            range_spec("file1.txt", 1, 5),
            range_spec("file2.txt", 10, 15),
        ],
    )?;
    let original_tip = test_repo.create_mesh_fixture("my_mesh", "Mesh to remove", &[&link_id])?;
    let hook_command = "links_blob=$(printf 'rm-race-link\\n' | git hash-object -w --stdin)\ntree=$(printf '100644 blob %s\\tlinks\\n' \"$links_blob\" | git mktree)\ncommit=$(GIT_AUTHOR_NAME='Test User' GIT_AUTHOR_EMAIL='test@example.com' GIT_COMMITTER_NAME='Test User' GIT_COMMITTER_EMAIL='test@example.com' git commit-tree \"$tree\" -p \"$(git rev-parse refs/meshes/v1/my_mesh)\" -m 'Raced update')\ngit update-ref refs/meshes/v1/my_mesh \"$commit\"";
    unsafe {
        std::env::set_var(
            "GIT_MESH_TEST_HOOK",
            format!("remove_mesh_before_transaction:once:{hook_command}"),
        );
    }
    let result = remove_mesh(&test_repo.repo, "my_mesh");
    unsafe {
        std::env::remove_var("GIT_MESH_TEST_HOOK");
    }
    assert!(result.is_err());
    let replacement_tip = test_repo.read_ref("refs/meshes/v1/my_mesh")?;
    assert_eq!(test_repo.read_ref("refs/meshes/v1/my_mesh")?, replacement_tip);
    assert_ne!(replacement_tip, original_tip);
    Ok(())
}

#[test]
fn test_structural_restore_mesh_fails_cleanly_on_race() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    let (first_link_id, _, _) = test_repo.create_link_fixture(
        "restore-race-a",
        [
            range_spec("file1.txt", 1, 5),
            range_spec("file2.txt", 10, 15),
        ],
    )?;
    test_repo.create_mesh_fixture("my_mesh", "Original mesh state", &[&first_link_id])?;
    let (second_link_id, _, _) = test_repo.create_link_fixture(
        "restore-race-b",
        [
            range_spec("file3.txt", 1, 5),
            range_spec("file4.txt", 10, 15),
        ],
    )?;
    test_repo.create_mesh_fixture("my_mesh", "Updated mesh state", &[&first_link_id, &second_link_id])?;

    let hook_command = "links_blob=$(printf 'restore-race-a\\nrestore-race-b\\n' | git hash-object -w --stdin)\ntree=$(printf '100644 blob %s\\tlinks\\n' \"$links_blob\" | git mktree)\ncommit=$(GIT_AUTHOR_NAME='Test User' GIT_AUTHOR_EMAIL='test@example.com' GIT_COMMITTER_NAME='Test User' GIT_COMMITTER_EMAIL='test@example.com' git commit-tree \"$tree\" -p \"$(git rev-parse refs/meshes/v1/my_mesh)\" -m 'Concurrent restore race')\ngit update-ref refs/meshes/v1/my_mesh \"$commit\"";
    unsafe {
        std::env::set_var(
            "GIT_MESH_TEST_HOOK",
            format!("restore_mesh_before_transaction:once:{hook_command}"),
        );
    }
    let result = restore_mesh(&test_repo.repo, "my_mesh", "HEAD~1");
    unsafe {
        std::env::remove_var("GIT_MESH_TEST_HOOK");
    }

    assert!(result.is_err());
    let mesh = show_mesh(&test_repo.repo, "my_mesh")?;
    assert_eq!(mesh.message, "Concurrent restore race");
    assert_eq!(mesh.links.len(), 2);
    Ok(())
}
