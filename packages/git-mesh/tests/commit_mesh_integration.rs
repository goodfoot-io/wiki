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

#[test]
fn test_commit_mesh_create_fresh() -> Result<()> {
    let test_repo = TestRepo::new()?;
    let input = CommitInput {
        name: "my_mesh".to_string(),
        adds: vec![[side_spec("file1.txt", 1, 5), side_spec("file2.txt", 10, 15)]],
        removes: vec![],
        message: "Initial mesh commit".to_string(),
        anchor_sha: None,
        amend: false,
    };

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
    let input1 = CommitInput {
        name: "my_mesh".to_string(),
        adds: vec![[side_spec("file1.txt", 1, 5), side_spec("file2.txt", 10, 15)]],
        removes: vec![],
        message: "First link".to_string(),
        anchor_sha: None,
        amend: false,
    };
    commit_mesh(&test_repo.repo, input1)?;

    let input2 = CommitInput {
        name: "my_mesh".to_string(),
        adds: vec![[side_spec("file3.txt", 1, 5), side_spec("file4.txt", 10, 15)]],
        removes: vec![],
        message: "Second link".to_string(),
        anchor_sha: None,
        amend: false,
    };
    commit_mesh(&test_repo.repo, input2)?;

    let mesh = show_mesh(&test_repo.repo, "my_mesh")?;
    assert_eq!(mesh.links.len(), 2);
    Ok(())
}

#[test]
fn test_commit_mesh_remove_link() -> Result<()> {
    let test_repo = TestRepo::new()?;
    let input1 = CommitInput {
        name: "my_mesh".to_string(),
        adds: vec![[side_spec("file1.txt", 1, 5), side_spec("file2.txt", 10, 15)]],
        removes: vec![],
        message: "First link".to_string(),
        anchor_sha: None,
        amend: false,
    };
    commit_mesh(&test_repo.repo, input1)?;

    let input2 = CommitInput {
        name: "my_mesh".to_string(),
        adds: vec![],
        removes: vec![[
            range_spec("file1.txt", 1, 5),
            range_spec("file2.txt", 10, 15),
        ]],
        message: "Remove link".to_string(),
        anchor_sha: None,
        amend: false,
    };
    commit_mesh(&test_repo.repo, input2)?;

    let mesh = show_mesh(&test_repo.repo, "my_mesh")?;
    assert_eq!(mesh.links.len(), 0);
    Ok(())
}

#[test]
fn test_commit_mesh_reconcile() -> Result<()> {
    let test_repo = TestRepo::new()?;
    let input1 = CommitInput {
        name: "my_mesh".to_string(),
        adds: vec![[side_spec("file1.txt", 1, 5), side_spec("file2.txt", 10, 15)]],
        removes: vec![],
        message: "First link".to_string(),
        anchor_sha: None,
        amend: false,
    };
    commit_mesh(&test_repo.repo, input1)?;

    let input2 = CommitInput {
        name: "my_mesh".to_string(),
        adds: vec![[side_spec("file1.txt", 2, 6), side_spec("file2.txt", 10, 15)]],
        removes: vec![[
            range_spec("file1.txt", 1, 5),
            range_spec("file2.txt", 10, 15),
        ]],
        message: "Reconcile drift".to_string(),
        anchor_sha: None,
        amend: false,
    };
    commit_mesh(&test_repo.repo, input2)?;

    let mesh = show_mesh(&test_repo.repo, "my_mesh")?;
    assert_eq!(mesh.links.len(), 1);
    Ok(())
}

#[test]
fn test_commit_mesh_amend_message() -> Result<()> {
    let test_repo = TestRepo::new()?;
    let input1 = CommitInput {
        name: "my_mesh".to_string(),
        adds: vec![[side_spec("file1.txt", 1, 5), side_spec("file2.txt", 10, 15)]],
        removes: vec![],
        message: "Initial message".to_string(),
        anchor_sha: None,
        amend: false,
    };
    commit_mesh(&test_repo.repo, input1)?;

    let input2 = CommitInput {
        name: "my_mesh".to_string(),
        adds: vec![],
        removes: vec![],
        message: "Amended message".to_string(),
        anchor_sha: None,
        amend: true,
    };
    commit_mesh(&test_repo.repo, input2)?;

    let mesh = show_mesh(&test_repo.repo, "my_mesh")?;
    assert_eq!(mesh.message, "Amended message");
    Ok(())
}

#[test]
fn test_commit_mesh_amend_with_links_fails() -> Result<()> {
    let test_repo = TestRepo::new()?;
    let input = CommitInput {
        name: "my_mesh".to_string(),
        adds: vec![[side_spec("file1.txt", 1, 5), side_spec("file2.txt", 10, 15)]],
        removes: vec![],
        message: "Amended message".to_string(),
        anchor_sha: None,
        amend: true,
    };

    let result = commit_mesh(&test_repo.repo, input);
    assert!(result.is_err());
    Ok(())
}

#[test]
fn test_commit_mesh_add_existing_pair_fails() -> Result<()> {
    let test_repo = TestRepo::new()?;
    let sides = [side_spec("file1.txt", 1, 5), side_spec("file2.txt", 10, 15)];

    let input1 = CommitInput {
        name: "my_mesh".to_string(),
        adds: vec![sides.clone()],
        removes: vec![],
        message: "First link".to_string(),
        anchor_sha: None,
        amend: false,
    };
    commit_mesh(&test_repo.repo, input1)?;

    let input2 = CommitInput {
        name: "my_mesh".to_string(),
        adds: vec![sides],
        removes: vec![],
        message: "Duplicate link".to_string(),
        anchor_sha: None,
        amend: false,
    };
    let result = commit_mesh(&test_repo.repo, input2);
    assert!(result.is_err());
    Ok(())
}

#[test]
fn test_commit_mesh_remove_nonexistent_pair_fails() -> Result<()> {
    let test_repo = TestRepo::new()?;
    let input = CommitInput {
        name: "my_mesh".to_string(),
        adds: vec![],
        removes: vec![[
            range_spec("file1.txt", 1, 5),
            range_spec("file2.txt", 10, 15),
        ]],
        message: "Remove nonexistent link".to_string(),
        anchor_sha: None,
        amend: false,
    };

    let result = commit_mesh(&test_repo.repo, input);
    assert!(result.is_err());
    Ok(())
}

#[test]
fn test_commit_mesh_empty_fails() -> Result<()> {
    let test_repo = TestRepo::new()?;
    let input = CommitInput {
        name: "my_mesh".to_string(),
        adds: vec![],
        removes: vec![],
        message: "Empty commit".to_string(),
        anchor_sha: None,
        amend: false,
    };

    let result = commit_mesh(&test_repo.repo, input);
    assert!(result.is_err());
    Ok(())
}
