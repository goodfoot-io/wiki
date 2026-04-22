use anyhow::Result;
use git_mesh::{
    commit_mesh, create_link, remove_mesh, rename_mesh, restore_mesh, show_mesh, stale_mesh,
    CommitInput, CopyDetection, CreateLinkInput, LinkStatus, RangeSpec, SideSpec,
};
use std::fs;

struct TestRepo {
    pub repo: gix::Repository,
    pub dir: tempfile::TempDir,
}

impl TestRepo {
    fn new() -> Result<Self> {
        let dir = tempfile::tempdir()?;
        let repo = gix::init(dir.path())?;
        Ok(Self { repo, dir })
    }

    fn write_file(&self, path: &str, content: &str) -> Result<()> {
        let p = self.dir.path().join(path);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(p, content)?;
        Ok(())
    }
}

// 1. Link Creation Tests

#[test]
#[ignore]
fn test_create_link_success() -> Result<()> {
    let test_repo = TestRepo::new()?;
    test_repo.write_file("file1.txt", "1\n2\n3\n4\n5\n")?;
    test_repo.write_file("file2.txt", "10\n11\n12\n13\n14\n15\n")?;

    let input = CreateLinkInput {
        sides: [
            SideSpec {
                path: "file1.txt".to_string(),
                start: 1,
                end: 5,
                copy_detection: Some(CopyDetection::SameCommit),
                ignore_whitespace: Some(true),
            },
            SideSpec {
                path: "file2.txt".to_string(),
                start: 1,
                end: 5,
                copy_detection: Some(CopyDetection::SameCommit),
                ignore_whitespace: Some(true),
            },
        ],
        anchor_sha: None,
        id: None,
    };

    let (id, link) = create_link(&test_repo.repo, input)?;
    assert!(!id.is_empty());
    assert_eq!(link.sides.len(), 2);
    Ok(())
}

#[test]
#[ignore]
fn test_create_link_out_of_bounds() -> Result<()> {
    let test_repo = TestRepo::new()?;
    test_repo.write_file("file1.txt", "1\n2\n")?;
    test_repo.write_file("file2.txt", "1\n2\n")?;

    let input = CreateLinkInput {
        sides: [
            SideSpec {
                path: "file1.txt".to_string(),
                start: 100, // Out of bounds
                end: 200,
                copy_detection: None,
                ignore_whitespace: None,
            },
            SideSpec {
                path: "file2.txt".to_string(),
                start: 1,
                end: 2,
                copy_detection: None,
                ignore_whitespace: None,
            },
        ],
        anchor_sha: None,
        id: None,
    };

    let result = create_link(&test_repo.repo, input);
    assert!(result.is_err());
    Ok(())
}

#[test]
#[ignore]
fn test_create_link_canonicalization() -> Result<()> {
    let test_repo = TestRepo::new()?;
    test_repo.write_file("a.txt", "1\n2\n3\n4\n5\n")?;
    test_repo.write_file("b.txt", "1\n2\n3\n4\n5\n")?;
    
    let side1 = SideSpec {
        path: "a.txt".to_string(),
        start: 1,
        end: 5,
        copy_detection: None,
        ignore_whitespace: None,
    };
    let side2 = SideSpec {
        path: "b.txt".to_string(),
        start: 1,
        end: 5,
        copy_detection: None,
        ignore_whitespace: None,
    };

    let input1 = CreateLinkInput {
        sides: [side1.clone(), side2.clone()],
        anchor_sha: None,
        id: None,
    };
    
    let input2 = CreateLinkInput {
        sides: [side2.clone(), side1.clone()], // Reversed
        anchor_sha: None,
        id: None,
    };

    let (_, link1) = create_link(&test_repo.repo, input1)?;
    let (_, link2) = create_link(&test_repo.repo, input2)?;

    // Assert that the sides are canonicalized into the same deterministic order
    assert_eq!(link1.sides[0].path, link2.sides[0].path);
    assert_eq!(link1.sides[1].path, link2.sides[1].path);
    Ok(())
}

// 2. Mesh Commit Tests

#[test]
#[ignore]
fn test_commit_mesh_create_fresh() -> Result<()> {
    let test_repo = TestRepo::new()?;
    let input = CommitInput {
        name: "my_mesh".to_string(),
        adds: vec![[
            SideSpec {
                path: "file1.txt".to_string(),
                start: 1,
                end: 5,
                copy_detection: None,
                ignore_whitespace: None,
            },
            SideSpec {
                path: "file2.txt".to_string(),
                start: 10,
                end: 15,
                copy_detection: None,
                ignore_whitespace: None,
            },
        ]],
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
#[ignore]
fn test_commit_mesh_add_link_to_existing() -> Result<()> {
    let test_repo = TestRepo::new()?;
    let input1 = CommitInput {
        name: "my_mesh".to_string(),
        adds: vec![[
            SideSpec {
                path: "file1.txt".to_string(),
                start: 1,
                end: 5,
                copy_detection: None,
                ignore_whitespace: None,
            },
            SideSpec {
                path: "file2.txt".to_string(),
                start: 10,
                end: 15,
                copy_detection: None,
                ignore_whitespace: None,
            },
        ]],
        removes: vec![],
        message: "First link".to_string(),
        anchor_sha: None,
        amend: false,
    };
    commit_mesh(&test_repo.repo, input1)?;

    let input2 = CommitInput {
        name: "my_mesh".to_string(),
        adds: vec![[
            SideSpec {
                path: "file3.txt".to_string(),
                start: 1,
                end: 5,
                copy_detection: None,
                ignore_whitespace: None,
            },
            SideSpec {
                path: "file4.txt".to_string(),
                start: 10,
                end: 15,
                copy_detection: None,
                ignore_whitespace: None,
            },
        ]],
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
#[ignore]
fn test_commit_mesh_remove_link() -> Result<()> {
    let test_repo = TestRepo::new()?;
    let input1 = CommitInput {
        name: "my_mesh".to_string(),
        adds: vec![[
            SideSpec {
                path: "file1.txt".to_string(),
                start: 1,
                end: 5,
                copy_detection: None,
                ignore_whitespace: None,
            },
            SideSpec {
                path: "file2.txt".to_string(),
                start: 10,
                end: 15,
                copy_detection: None,
                ignore_whitespace: None,
            },
        ]],
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
            RangeSpec {
                path: "file1.txt".to_string(),
                start: 1,
                end: 5,
            },
            RangeSpec {
                path: "file2.txt".to_string(),
                start: 10,
                end: 15,
            },
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
#[ignore]
fn test_commit_mesh_reconcile() -> Result<()> {
    let test_repo = TestRepo::new()?;
    let input1 = CommitInput {
        name: "my_mesh".to_string(),
        adds: vec![[
            SideSpec {
                path: "file1.txt".to_string(),
                start: 1,
                end: 5,
                copy_detection: None,
                ignore_whitespace: None,
            },
            SideSpec {
                path: "file2.txt".to_string(),
                start: 10,
                end: 15,
                copy_detection: None,
                ignore_whitespace: None,
            },
        ]],
        removes: vec![],
        message: "First link".to_string(),
        anchor_sha: None,
        amend: false,
    };
    commit_mesh(&test_repo.repo, input1)?;

    let input2 = CommitInput {
        name: "my_mesh".to_string(),
        adds: vec![[
            SideSpec {
                path: "file1.txt".to_string(),
                start: 2, // Drifted
                end: 6,
                copy_detection: None,
                ignore_whitespace: None,
            },
            SideSpec {
                path: "file2.txt".to_string(),
                start: 10,
                end: 15,
                copy_detection: None,
                ignore_whitespace: None,
            },
        ]],
        removes: vec![[
            RangeSpec {
                path: "file1.txt".to_string(),
                start: 1,
                end: 5,
            },
            RangeSpec {
                path: "file2.txt".to_string(),
                start: 10,
                end: 15,
            },
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
#[ignore]
fn test_commit_mesh_amend_message() -> Result<()> {
    let test_repo = TestRepo::new()?;
    let input1 = CommitInput {
        name: "my_mesh".to_string(),
        adds: vec![[
            SideSpec {
                path: "file1.txt".to_string(),
                start: 1,
                end: 5,
                copy_detection: None,
                ignore_whitespace: None,
            },
            SideSpec {
                path: "file2.txt".to_string(),
                start: 10,
                end: 15,
                copy_detection: None,
                ignore_whitespace: None,
            },
        ]],
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
#[ignore]
fn test_commit_mesh_amend_with_links_fails() -> Result<()> {
    let test_repo = TestRepo::new()?;
    let input = CommitInput {
        name: "my_mesh".to_string(),
        adds: vec![[
            SideSpec {
                path: "file1.txt".to_string(),
                start: 1,
                end: 5,
                copy_detection: None,
                ignore_whitespace: None,
            },
            SideSpec {
                path: "file2.txt".to_string(),
                start: 10,
                end: 15,
                copy_detection: None,
                ignore_whitespace: None,
            },
        ]],
        removes: vec![],
        message: "Amended message".to_string(),
        anchor_sha: None,
        amend: true, // true alongside adds yields error
    };

    let result = commit_mesh(&test_repo.repo, input);
    assert!(result.is_err());
    Ok(())
}

#[test]
#[ignore]
fn test_commit_mesh_add_existing_pair_fails() -> Result<()> {
    let test_repo = TestRepo::new()?;
    let sides = [
        SideSpec {
            path: "file1.txt".to_string(),
            start: 1,
            end: 5,
            copy_detection: None,
            ignore_whitespace: None,
        },
        SideSpec {
            path: "file2.txt".to_string(),
            start: 10,
            end: 15,
            copy_detection: None,
            ignore_whitespace: None,
        },
    ];

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
        adds: vec![sides.clone()],
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
#[ignore]
fn test_commit_mesh_remove_nonexistent_pair_fails() -> Result<()> {
    let test_repo = TestRepo::new()?;
    let input = CommitInput {
        name: "my_mesh".to_string(),
        adds: vec![],
        removes: vec![[
            RangeSpec {
                path: "file1.txt".to_string(),
                start: 1,
                end: 5,
            },
            RangeSpec {
                path: "file2.txt".to_string(),
                start: 10,
                end: 15,
            },
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
#[ignore]
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

// 3. Staleness Computation Tests

#[test]
#[ignore]
fn test_stale_mesh_fresh() -> Result<()> {
    let test_repo = TestRepo::new()?;
    // Simulate initial setup
    test_repo.write_file("file1.txt", "1\n2\n3\n4\n5\n")?;
    test_repo.write_file("file2.txt", "10\n11\n12\n13\n14\n15\n")?;
    
    let resolved = stale_mesh(&test_repo.repo, "my_mesh")?;
    assert!(!resolved.links.is_empty());
    assert_eq!(resolved.links[0].status, LinkStatus::Fresh);
    Ok(())
}

#[test]
#[ignore]
fn test_stale_mesh_moved() -> Result<()> {
    let test_repo = TestRepo::new()?;
    test_repo.write_file("file1.txt", "new_line_here\n1\n2\n3\n4\n5\n")?;
    
    let resolved = stale_mesh(&test_repo.repo, "my_mesh")?;
    assert!(!resolved.links.is_empty());
    assert_eq!(resolved.links[0].status, LinkStatus::Moved);
    Ok(())
}

#[test]
#[ignore]
fn test_stale_mesh_modified() -> Result<()> {
    let test_repo = TestRepo::new()?;
    test_repo.write_file("file1.txt", "1\n2\nMODIFIED\n4\n5\n")?;
    
    let resolved = stale_mesh(&test_repo.repo, "my_mesh")?;
    assert!(!resolved.links.is_empty());
    assert_eq!(resolved.links[0].status, LinkStatus::Modified);
    Ok(())
}

#[test]
#[ignore]
fn test_stale_mesh_rewritten() -> Result<()> {
    let test_repo = TestRepo::new()?;
    test_repo.write_file("file1.txt", "MOD\nMOD\nMOD\nMOD\nMOD\n")?;
    
    let resolved = stale_mesh(&test_repo.repo, "my_mesh")?;
    assert!(!resolved.links.is_empty());
    assert_eq!(resolved.links[0].status, LinkStatus::Rewritten);
    Ok(())
}

#[test]
#[ignore]
fn test_stale_mesh_missing() -> Result<()> {
    let test_repo = TestRepo::new()?;
    // file1.txt doesn't exist anymore
    
    let resolved = stale_mesh(&test_repo.repo, "my_mesh")?;
    assert!(!resolved.links.is_empty());
    assert_eq!(resolved.links[0].status, LinkStatus::Missing);
    Ok(())
}

// 4. Structural Operation Tests

#[test]
#[ignore]
fn test_structural_rm() -> Result<()> {
    let test_repo = TestRepo::new()?;
    remove_mesh(&test_repo.repo, "my_mesh")?;
    let result = show_mesh(&test_repo.repo, "my_mesh");
    assert!(result.is_err());
    Ok(())
}

#[test]
#[ignore]
fn test_structural_mv() -> Result<()> {
    let test_repo = TestRepo::new()?;
    rename_mesh(&test_repo.repo, "old_mesh", "new_mesh", false)?;
    
    let result = show_mesh(&test_repo.repo, "old_mesh");
    assert!(result.is_err());
    
    let mesh = show_mesh(&test_repo.repo, "new_mesh")?;
    assert_eq!(mesh.name, "new_mesh");
    Ok(())
}

#[test]
#[ignore]
fn test_structural_restore() -> Result<()> {
    let test_repo = TestRepo::new()?;
    restore_mesh(&test_repo.repo, "my_mesh", "HEAD~1")?;
    let mesh = show_mesh(&test_repo.repo, "my_mesh")?;
    assert_eq!(mesh.name, "my_mesh");
    Ok(())
}
