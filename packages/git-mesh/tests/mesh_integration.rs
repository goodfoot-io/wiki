use anyhow::Result;
use git_mesh::{
    commit_mesh, create_link, remove_mesh, rename_mesh, restore_mesh, show_mesh, stale_mesh,
    CommitInput, CopyDetection, CreateLinkInput, LinkStatus, RangeSpec, SideSpec,
};
use std::fs;
use std::process::{Command, Output, Stdio};

struct TestRepo {
    pub repo: gix::Repository,
    pub dir: tempfile::TempDir,
}

impl TestRepo {
    fn new() -> Result<Self> {
        let dir = tempfile::tempdir()?;
        let repo = gix::init(dir.path())?;
        let mut test_repo = Self { repo, dir };
        
        test_repo.write_file("initial.txt", "initial content")?;
        test_repo.write_file("file1.txt", "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n")?;
        test_repo.write_file("file2.txt", "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\nline11\nline12\nline13\nline14\nline15\nline16\n")?;
        test_repo.write_file("file3.txt", "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n")?;
        test_repo.write_file("file4.txt", "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\nline11\nline12\nline13\nline14\nline15\nline16\n")?;
        test_repo.commit_all("initial commit")?;
        
        Ok(test_repo)
    }

    fn write_file(&self, path: &str, content: &str) -> Result<()> {
        let p = self.dir.path().join(path);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(p, content)?;
        Ok(())
    }

    fn commit_all(&mut self, message: &str) -> Result<()> {
        self.run_git(["add", "."])?;
        self.run_git_with_identity(["commit", "-m", message])?;

        self.repo = gix::open(self.dir.path())?;
        Ok(())
    }

    fn git<I, S>(&self, args: I) -> Command
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut command = Command::new("git");
        command.current_dir(self.dir.path());
        for arg in args {
            command.arg(arg.as_ref());
        }
        command
    }

    fn with_identity(command: &mut Command) -> &mut Command {
        command
            .env("GIT_AUTHOR_NAME", "Test User")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test User")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
    }

    fn ensure_success(output: Output, context: &str) -> Result<Output> {
        anyhow::ensure!(
            output.status.success(),
            "{context}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(output)
    }

    fn run_git<I, S>(&self, args: I) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self::ensure_success(self.git(args).output()?, "git command failed")
    }

    fn run_git_with_identity<I, S>(&self, args: I) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut command = self.git(args);
        Self::with_identity(&mut command);
        Self::ensure_success(command.output()?, "git command failed")
    }

    fn run_git_with_input<I, S>(&self, args: I, input: &str) -> Result<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        use std::io::Write;

        let mut child = self
            .git(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        {
            let mut stdin = child.stdin.take().ok_or_else(|| anyhow::anyhow!("missing stdin"))?;
            stdin.write_all(input.as_bytes())?;
        }
        let output = Self::ensure_success(child.wait_with_output()?, "git command failed")?;
        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    }

    fn git_output<I, S>(&self, args: I) -> Result<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let output = self.run_git(args)?;
        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    }

    fn head_sha(&self) -> Result<String> {
        self.git_output(["rev-parse", "HEAD"])
    }

    fn write_blob(&self, content: &str) -> Result<String> {
        self.run_git_with_input(["hash-object", "-w", "--stdin"], content)
    }

    fn read_ref(&self, name: &str) -> Result<String> {
        self.git_output(["rev-parse", name])
    }

    fn set_ref(&mut self, name: &str, oid: &str) -> Result<()> {
        self.git_output(["update-ref", name, oid])?;
        self.repo = gix::open(self.dir.path())?;
        Ok(())
    }

    fn file_blob_at_head(&self, path: &str) -> Result<String> {
        self.git_output(["rev-parse", &format!("HEAD:{path}")])
    }

    fn create_link_fixture(
        &mut self,
        id: &str,
        sides: [RangeSpec; 2],
    ) -> Result<(String, String, String)> {
        let anchor_sha = self.head_sha()?;
        let side_a_blob = self.file_blob_at_head(&sides[0].path)?;
        let side_b_blob = self.file_blob_at_head(&sides[1].path)?;
        let link_text = format!(
            "anchor {anchor_sha}\ncreated 2026-01-01T00:00:00Z\nside {} {} {} same-commit true\t{}\nside {} {} {} same-commit true\t{}\n",
            sides[0].start,
            sides[0].end,
            side_a_blob,
            sides[0].path,
            sides[1].start,
            sides[1].end,
            side_b_blob,
            sides[1].path
        );
        let blob_oid = self.write_blob(&link_text)?;
        self.set_ref(&format!("refs/links/v1/{id}"), &blob_oid)?;
        Ok((id.to_string(), blob_oid, link_text))
    }

    fn create_mesh_fixture(&mut self, name: &str, message: &str, link_ids: &[&str]) -> Result<String> {
        let mut links_text = String::new();
        for id in link_ids {
            links_text.push_str(id);
            links_text.push('\n');
        }
        let links_blob = self.write_blob(&links_text)?;
        let tree_oid = self.run_git_with_input(
            ["mktree"],
            &format!("100644 blob {links_blob}\tlinks\n"),
        )?;

        let parent = self.read_ref(&format!("refs/meshes/v1/{name}")).ok();
        let mut commit_args = vec!["commit-tree".to_string(), tree_oid.clone(), "-m".to_string(), message.to_string()];
        if let Some(parent) = parent {
            commit_args.push("-p".to_string());
            commit_args.push(parent);
        }

        let commit_oid = self
            .run_git_with_identity(commit_args.iter().map(String::as_str))?;
        let commit_oid = String::from_utf8(commit_oid.stdout)?.trim().to_string();
        self.set_ref(&format!("refs/meshes/v1/{name}"), &commit_oid)?;
        Ok(commit_oid)
    }

    fn remove_file(&mut self, path: &str) -> Result<()> {
        fs::remove_file(self.dir.path().join(path))?;
        self.repo = gix::open(self.dir.path())?;
        Ok(())
    }
}

// 1. Link Creation Tests

#[test]
fn test_create_link_success() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    test_repo.write_file("file1.txt", "1\n2\n3\n4\n5\n")?;
    test_repo.write_file("file2.txt", "10\n11\n12\n13\n14\n15\n")?;
    test_repo.commit_all("init")?;

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
    let ref_oid = test_repo.read_ref(&format!("refs/links/v1/{id}"))?;
    let link_blob = test_repo.git_output(["cat-file", "-p", &ref_oid])?;
    assert!(link_blob.contains(&format!("anchor {}", link.anchor_sha)));
    assert!(link_blob.contains(&format!(
        "side {} {} {} same-commit true\t{}",
        link.sides[0].start, link.sides[0].end, link.sides[0].blob, link.sides[0].path
    )));
    assert!(link_blob.contains(&format!(
        "side {} {} {} same-commit true\t{}",
        link.sides[1].start, link.sides[1].end, link.sides[1].blob, link.sides[1].path
    )));
    Ok(())
}

#[test]
fn test_create_link_out_of_bounds() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    test_repo.write_file("file1.txt", "1\n2\n")?;
    test_repo.write_file("file2.txt", "1\n2\n")?;
    test_repo.commit_all("init")?;

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
fn test_create_link_canonicalization() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    test_repo.write_file("a.txt", "1\n2\n3\n4\n5\n")?;
    test_repo.write_file("b.txt", "1\n2\n3\n4\n5\n")?;
    test_repo.commit_all("init")?;
    
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
    let mut test_repo = TestRepo::new()?;
    let (link_id, _, _) = test_repo.create_link_fixture(
        "fresh-link",
        [
            RangeSpec { path: "file1.txt".to_string(), start: 1, end: 5 },
            RangeSpec { path: "file2.txt".to_string(), start: 10, end: 15 },
        ],
    )?;
    test_repo.create_mesh_fixture("my_mesh", "Fresh mesh fixture", &[&link_id])?;

    let resolved = stale_mesh(&test_repo.repo, "my_mesh")?;
    assert!(!resolved.links.is_empty());
    assert_eq!(resolved.links[0].status, LinkStatus::Fresh);
    Ok(())
}

#[test]
#[ignore]
fn test_stale_mesh_moved() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    let (link_id, _, _) = test_repo.create_link_fixture(
        "moved-link",
        [
            RangeSpec { path: "file1.txt".to_string(), start: 1, end: 5 },
            RangeSpec { path: "file2.txt".to_string(), start: 10, end: 15 },
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
#[ignore]
fn test_stale_mesh_modified() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    let (link_id, _, _) = test_repo.create_link_fixture(
        "modified-link",
        [
            RangeSpec { path: "file1.txt".to_string(), start: 1, end: 5 },
            RangeSpec { path: "file2.txt".to_string(), start: 10, end: 15 },
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
#[ignore]
fn test_stale_mesh_rewritten() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    let (link_id, _, _) = test_repo.create_link_fixture(
        "rewritten-link",
        [
            RangeSpec { path: "file1.txt".to_string(), start: 1, end: 5 },
            RangeSpec { path: "file2.txt".to_string(), start: 10, end: 15 },
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
#[ignore]
fn test_stale_mesh_missing() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    let (link_id, _, _) = test_repo.create_link_fixture(
        "missing-link",
        [
            RangeSpec { path: "file1.txt".to_string(), start: 1, end: 5 },
            RangeSpec { path: "file2.txt".to_string(), start: 10, end: 15 },
        ],
    )?;
    test_repo.create_mesh_fixture("my_mesh", "Missing mesh fixture", &[&link_id])?;
    test_repo.remove_file("file1.txt")?;

    let resolved = stale_mesh(&test_repo.repo, "my_mesh")?;
    assert!(!resolved.links.is_empty());
    assert_eq!(resolved.links[0].status, LinkStatus::Missing);
    Ok(())
}

// 4. Structural Operation Tests

#[test]
#[ignore]
fn test_structural_rm() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    let (link_id, _, _) = test_repo.create_link_fixture(
        "rm-link",
        [
            RangeSpec { path: "file1.txt".to_string(), start: 1, end: 5 },
            RangeSpec { path: "file2.txt".to_string(), start: 10, end: 15 },
        ],
    )?;
    test_repo.create_mesh_fixture("my_mesh", "Mesh to remove", &[&link_id])?;
    remove_mesh(&test_repo.repo, "my_mesh")?;
    let result = show_mesh(&test_repo.repo, "my_mesh");
    assert!(result.is_err());
    Ok(())
}

#[test]
#[ignore]
fn test_structural_mv() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    let (link_id, _, _) = test_repo.create_link_fixture(
        "mv-link",
        [
            RangeSpec { path: "file1.txt".to_string(), start: 1, end: 5 },
            RangeSpec { path: "file2.txt".to_string(), start: 10, end: 15 },
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
#[ignore]
fn test_structural_restore() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    let (first_link_id, _, _) = test_repo.create_link_fixture(
        "restore-link-a",
        [
            RangeSpec { path: "file1.txt".to_string(), start: 1, end: 5 },
            RangeSpec { path: "file2.txt".to_string(), start: 10, end: 15 },
        ],
    )?;
    let first_commit = test_repo.create_mesh_fixture("my_mesh", "Original mesh state", &[&first_link_id])?;
    let (second_link_id, _, _) = test_repo.create_link_fixture(
        "restore-link-b",
        [
            RangeSpec { path: "file3.txt".to_string(), start: 1, end: 5 },
            RangeSpec { path: "file4.txt".to_string(), start: 10, end: 15 },
        ],
    )?;
    let _second_commit = test_repo.create_mesh_fixture("my_mesh", "Updated mesh state", &[&first_link_id, &second_link_id])?;
    assert_ne!(test_repo.read_ref("refs/meshes/v1/my_mesh")?, first_commit);
    restore_mesh(&test_repo.repo, "my_mesh", "HEAD~1")?;
    let mesh = show_mesh(&test_repo.repo, "my_mesh")?;
    assert_eq!(mesh.name, "my_mesh");
    Ok(())
}
