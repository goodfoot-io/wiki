mod support;

use anyhow::Result;
use git_mesh::{CommitInput, RangeSpec, SideSpec, commit_mesh, show_mesh};

use support::TestRepo;

fn side_spec(path: &str, start: u32, end: u32) -> SideSpec {
    SideSpec {
        path: path.to_string(),
        start,
        end,
        copy_detection: None,
        ignore_whitespace: None,
    }
}

fn range_spec(path: &str, start: u32, end: u32) -> RangeSpec {
    RangeSpec {
        path: path.to_string(),
        start,
        end,
    }
}

fn commit_input(
    name: &str,
    adds: Vec<[SideSpec; 2]>,
    removes: Vec<[RangeSpec; 2]>,
    message: &str,
) -> CommitInput {
    CommitInput {
        name: name.to_string(),
        adds,
        removes,
        message: message.to_string(),
        anchor_sha: None,
        expected_tip: None,
        amend: false,
    }
}

#[test]
fn test_commit_mesh_create_fresh() -> Result<()> {
    let test_repo = TestRepo::new()?;
    let input = commit_input(
        "my_mesh",
        vec![[side_spec("file1.txt", 1, 5), side_spec("file2.txt", 10, 15)]],
        vec![],
        "Initial mesh commit",
    );

    commit_mesh(&test_repo.repo, input)?;

    let mesh = show_mesh(&test_repo.repo, "my_mesh")?;
    assert_eq!(mesh.name, "my_mesh");
    assert_eq!(mesh.message, "Initial mesh commit");
    assert_eq!(mesh.links.len(), 1);
    Ok(())
}

#[test]
fn test_commit_mesh_add_link_to_existing() -> Result<()> {
    let test_repo = TestRepo::new()?;
    commit_mesh(
        &test_repo.repo,
        commit_input(
            "my_mesh",
            vec![[side_spec("file1.txt", 1, 5), side_spec("file2.txt", 10, 15)]],
            vec![],
            "First link",
        ),
    )?;

    commit_mesh(
        &test_repo.repo,
        commit_input(
            "my_mesh",
            vec![[side_spec("file3.txt", 1, 5), side_spec("file4.txt", 10, 15)]],
            vec![],
            "Second link",
        ),
    )?;

    let mesh = show_mesh(&test_repo.repo, "my_mesh")?;
    assert_eq!(mesh.links.len(), 2);
    Ok(())
}

#[test]
fn test_commit_mesh_remove_link() -> Result<()> {
    let test_repo = TestRepo::new()?;
    commit_mesh(
        &test_repo.repo,
        commit_input(
            "my_mesh",
            vec![[side_spec("file1.txt", 1, 5), side_spec("file2.txt", 10, 15)]],
            vec![],
            "First link",
        ),
    )?;

    commit_mesh(
        &test_repo.repo,
        commit_input(
            "my_mesh",
            vec![],
            vec![[
                range_spec("file1.txt", 1, 5),
                range_spec("file2.txt", 10, 15),
            ]],
            "Remove link",
        ),
    )?;

    let mesh = show_mesh(&test_repo.repo, "my_mesh")?;
    assert_eq!(mesh.links.len(), 0);
    Ok(())
}

#[test]
fn test_commit_mesh_reconcile() -> Result<()> {
    let test_repo = TestRepo::new()?;
    commit_mesh(
        &test_repo.repo,
        commit_input(
            "my_mesh",
            vec![[side_spec("file1.txt", 1, 5), side_spec("file2.txt", 10, 15)]],
            vec![],
            "First link",
        ),
    )?;

    commit_mesh(
        &test_repo.repo,
        commit_input(
            "my_mesh",
            vec![[side_spec("file1.txt", 2, 6), side_spec("file2.txt", 10, 15)]],
            vec![[
                range_spec("file1.txt", 1, 5),
                range_spec("file2.txt", 10, 15),
            ]],
            "Reconcile drift",
        ),
    )?;

    let mesh = show_mesh(&test_repo.repo, "my_mesh")?;
    assert_eq!(mesh.links.len(), 1);
    Ok(())
}

#[test]
fn test_commit_mesh_amend_message_reuses_tip_parents() -> Result<()> {
    let test_repo = TestRepo::new()?;
    commit_mesh(
        &test_repo.repo,
        commit_input(
            "my_mesh",
            vec![[side_spec("file1.txt", 1, 5), side_spec("file2.txt", 10, 15)]],
            vec![],
            "Initial message",
        ),
    )?;

    let initial_tip = test_repo.read_ref("refs/meshes/v1/my_mesh")?;
    let initial_parents = test_repo.commit_parents(&initial_tip)?;

    commit_mesh(
        &test_repo.repo,
        CommitInput {
            name: "my_mesh".to_string(),
            adds: vec![],
            removes: vec![],
            message: "Amended message".to_string(),
            anchor_sha: None,
            expected_tip: Some(initial_tip),
            amend: true,
        },
    )?;

    let mesh = show_mesh(&test_repo.repo, "my_mesh")?;
    assert_eq!(mesh.message, "Amended message");

    let amended_tip = test_repo.read_ref("refs/meshes/v1/my_mesh")?;
    assert_eq!(test_repo.commit_parents(&amended_tip)?, initial_parents);
    Ok(())
}

#[test]
fn test_commit_mesh_amend_with_links_fails() -> Result<()> {
    let test_repo = TestRepo::new()?;
    let result = commit_mesh(
        &test_repo.repo,
        CommitInput {
            name: "my_mesh".to_string(),
            adds: vec![[side_spec("file1.txt", 1, 5), side_spec("file2.txt", 10, 15)]],
            removes: vec![],
            message: "Amended message".to_string(),
            anchor_sha: None,
            expected_tip: None,
            amend: true,
        },
    );
    assert!(result.is_err());
    Ok(())
}

#[test]
fn test_commit_mesh_add_existing_pair_fails() -> Result<()> {
    let test_repo = TestRepo::new()?;
    let sides = [side_spec("file1.txt", 1, 5), side_spec("file2.txt", 10, 15)];

    commit_mesh(
        &test_repo.repo,
        commit_input("my_mesh", vec![sides.clone()], vec![], "First link"),
    )?;

    let result = commit_mesh(
        &test_repo.repo,
        commit_input("my_mesh", vec![sides], vec![], "Duplicate link"),
    );
    assert!(result.is_err());
    Ok(())
}

#[test]
fn test_commit_mesh_remove_nonexistent_pair_fails() -> Result<()> {
    let test_repo = TestRepo::new()?;
    let result = commit_mesh(
        &test_repo.repo,
        commit_input(
            "my_mesh",
            vec![],
            vec![[
                range_spec("file1.txt", 1, 5),
                range_spec("file2.txt", 10, 15),
            ]],
            "Remove nonexistent link",
        ),
    );
    assert!(result.is_err());
    Ok(())
}

#[test]
fn test_commit_mesh_empty_fails() -> Result<()> {
    let test_repo = TestRepo::new()?;
    let result = commit_mesh(
        &test_repo.repo,
        commit_input("my_mesh", vec![], vec![], "Empty commit"),
    );
    assert!(result.is_err());
    Ok(())
}

#[test]
fn test_commit_mesh_canonicalizes_links_file_on_write() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    test_repo.create_link_fixture(
        "z-link",
        [
            range_spec("file1.txt", 1, 5),
            range_spec("file2.txt", 10, 15),
        ],
    )?;
    test_repo.create_link_fixture(
        "a-link",
        [
            range_spec("file3.txt", 1, 5),
            range_spec("file4.txt", 10, 15),
        ],
    )?;
    test_repo.create_mesh_fixture("my_mesh", "fixture", &["z-link", "a-link", "z-link"])?;

    let mesh_tip = test_repo.read_ref("refs/meshes/v1/my_mesh")?;
    commit_mesh(
        &test_repo.repo,
        CommitInput {
            name: "my_mesh".to_string(),
            adds: vec![],
            removes: vec![],
            message: "Canonicalized".to_string(),
            anchor_sha: None,
            expected_tip: Some(mesh_tip),
            amend: true,
        },
    )?;

    let updated_tip = test_repo.read_ref("refs/meshes/v1/my_mesh")?;
    assert_eq!(
        test_repo.show_file(&updated_tip, "links")?,
        "a-link\nz-link"
    );
    Ok(())
}

#[test]
fn test_commit_mesh_validation_failure_does_not_update_mesh_ref() -> Result<()> {
    let test_repo = TestRepo::new()?;
    let result = commit_mesh(
        &test_repo.repo,
        commit_input(
            "my_mesh",
            vec![
                [side_spec("file1.txt", 1, 5), side_spec("file2.txt", 10, 15)],
                [
                    side_spec("file1.txt", 50, 60),
                    side_spec("file2.txt", 10, 15),
                ],
            ],
            vec![],
            "invalid",
        ),
    );

    assert!(result.is_err());
    assert!(test_repo.read_ref("refs/meshes/v1/my_mesh").is_err());
    Ok(())
}

#[test]
fn test_commit_mesh_stale_expected_tip_fails_with_no_ref_update() -> Result<()> {
    let test_repo = TestRepo::new()?;
    commit_mesh(
        &test_repo.repo,
        commit_input(
            "my_mesh",
            vec![[side_spec("file1.txt", 1, 5), side_spec("file2.txt", 10, 15)]],
            vec![],
            "First link",
        ),
    )?;

    let stale_tip = test_repo.read_ref("refs/meshes/v1/my_mesh")?;
    commit_mesh(
        &test_repo.repo,
        commit_input(
            "my_mesh",
            vec![[side_spec("file3.txt", 1, 5), side_spec("file4.txt", 10, 15)]],
            vec![],
            "Second link",
        ),
    )?;
    let current_tip = test_repo.read_ref("refs/meshes/v1/my_mesh")?;

    let result = commit_mesh(
        &test_repo.repo,
        CommitInput {
            name: "my_mesh".to_string(),
            adds: vec![],
            removes: vec![],
            message: "stale amend".to_string(),
            anchor_sha: None,
            expected_tip: Some(stale_tip),
            amend: true,
        },
    );

    assert!(result.is_err());
    assert_eq!(test_repo.read_ref("refs/meshes/v1/my_mesh")?, current_tip);
    Ok(())
}
