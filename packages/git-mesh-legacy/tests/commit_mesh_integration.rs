mod support;

use anyhow::Result;
use git_mesh_legacy::{CommitInput, RangeSpec, SideSpec, commit_mesh, show_mesh};

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
    let link_refs_before = test_repo.list_refs("refs/links/v1")?;
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
    assert_eq!(test_repo.list_refs("refs/links/v1")?, link_refs_before);
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

#[test]
fn test_commit_mesh_retries_implicit_tip_race_without_partial_link_refs() -> Result<()> {
    let test_repo = TestRepo::new()?;
    commit_mesh(
        &test_repo.repo,
        commit_input(
            "my_mesh",
            vec![[side_spec("file1.txt", 1, 5), side_spec("file2.txt", 10, 15)]],
            vec![],
            "Initial state",
        ),
    )?;

    let link_refs_before = test_repo.list_refs("refs/links/v1")?;
    let hook_command = "git hash-object -w --stdin <<'EOF' >/dev/null\nfile3-link\nEOF\nblob=$(git rev-parse --verify HEAD:file3.txt)\nblob2=$(git rev-parse --verify HEAD:file4.txt)\nlink_blob=$(printf 'anchor %s\\ncreated 2026-01-01T00:00:00Z\\nside 1 5 %s same-commit true\\tfile3.txt\\nside 10 15 %s same-commit true\\tfile4.txt\\n' \"$(git rev-parse HEAD)\" \"$blob\" \"$blob2\" | git hash-object -w --stdin)\ngit update-ref refs/links/v1/raced-link \"$link_blob\"\nlinks_blob=$(printf 'raced-link\\n' | git hash-object -w --stdin)\ntree=$(printf '100644 blob %s\\tlinks\\n' \"$links_blob\" | git mktree)\ncommit=$(GIT_AUTHOR_NAME='Test User' GIT_AUTHOR_EMAIL='test@example.com' GIT_COMMITTER_NAME='Test User' GIT_COMMITTER_EMAIL='test@example.com' git commit-tree \"$tree\" -p \"$(git rev-parse refs/meshes/v1/my_mesh)\" -m 'Raced state')\ngit update-ref refs/meshes/v1/my_mesh \"$commit\"";
    unsafe {
        std::env::set_var(
            "GIT_MESH_TEST_HOOK",
            format!("commit_mesh_before_transaction:once:{hook_command}"),
        );
    }

    let result = commit_mesh(
        &test_repo.repo,
        commit_input(
            "my_mesh",
            vec![[side_spec("file1.txt", 2, 6), side_spec("file2.txt", 10, 15)]],
            vec![],
            "Retried state",
        ),
    );

    unsafe {
        std::env::remove_var("GIT_MESH_TEST_HOOK");
    }
    result?;

    let mesh = show_mesh(&test_repo.repo, "my_mesh")?;
    assert_eq!(mesh.message, "Retried state");
    assert_eq!(mesh.links.len(), 2);
    let link_refs_after = test_repo.list_refs("refs/links/v1")?;
    assert_eq!(link_refs_after.len(), link_refs_before.len() + 2);
    assert!(test_repo.ref_exists("refs/links/v1/raced-link"));
    Ok(())
}

#[test]
fn test_commit_mesh_empty_invocation_message_mentions_flags() -> Result<()> {
    let test_repo = TestRepo::new()?;
    let err = commit_mesh(
        &test_repo.repo,
        commit_input("my_mesh", vec![], vec![], "nothing"),
    )
    .unwrap_err()
    .to_string();
    assert!(
        err.contains("--link") && err.contains("--unlink") && err.contains("--amend"),
        "expected empty-invocation error to cite --link/--unlink/--amend, got: {err}"
    );
    Ok(())
}

#[test]
fn test_commit_mesh_amend_with_link_error_mentions_reword_only() -> Result<()> {
    let test_repo = TestRepo::new()?;
    let result = commit_mesh(
        &test_repo.repo,
        CommitInput {
            name: "my_mesh".to_string(),
            adds: vec![[side_spec("file1.txt", 1, 5), side_spec("file2.txt", 10, 15)]],
            removes: vec![],
            message: "x".to_string(),
            anchor_sha: None,
            expected_tip: None,
            amend: true,
        },
    );
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("--amend") && err.contains("reword"),
        "expected --amend/reword error, got: {err}"
    );
    Ok(())
}

#[test]
fn test_commit_mesh_duplicate_link_error_points_at_reanchor_idiom() -> Result<()> {
    let test_repo = TestRepo::new()?;
    let sides = [side_spec("file1.txt", 1, 5), side_spec("file2.txt", 10, 15)];
    commit_mesh(
        &test_repo.repo,
        commit_input("my_mesh", vec![sides.clone()], vec![], "first"),
    )?;
    let err = commit_mesh(
        &test_repo.repo,
        commit_input("my_mesh", vec![sides], vec![], "dup"),
    )
    .unwrap_err()
    .to_string();
    assert!(
        err.contains("file1.txt#L1-L5:file2.txt#L10-L15"),
        "expected pair in error, got: {err}"
    );
    assert!(
        err.contains("--unlink") && err.contains("--link") && err.contains("re-anchor"),
        "expected re-anchor idiom in error, got: {err}"
    );
    Ok(())
}

#[test]
fn test_commit_mesh_missing_unlink_pair_error_names_pair() -> Result<()> {
    let test_repo = TestRepo::new()?;
    // Prime mesh with an unrelated link so --unlink runs against a real tip.
    commit_mesh(
        &test_repo.repo,
        commit_input(
            "my_mesh",
            vec![[side_spec("file3.txt", 1, 5), side_spec("file4.txt", 10, 15)]],
            vec![],
            "seed",
        ),
    )?;
    let err = commit_mesh(
        &test_repo.repo,
        commit_input(
            "my_mesh",
            vec![],
            vec![[range_spec("file1.txt", 1, 5), range_spec("file2.txt", 10, 15)]],
            "rm",
        ),
    )
    .unwrap_err()
    .to_string();
    assert!(
        err.contains("file1.txt#L1-L5:file2.txt#L10-L15"),
        "expected pair in error, got: {err}"
    );
    assert!(err.contains("--unlink"), "expected --unlink label, got: {err}");
    Ok(())
}

#[test]
fn test_commit_mesh_validation_does_not_write_link_refs_when_later_add_fails() -> Result<()> {
    let test_repo = TestRepo::new()?;
    // Seed the mesh with pair P1.
    let p1 = [side_spec("file1.txt", 1, 5), side_spec("file2.txt", 10, 15)];
    commit_mesh(
        &test_repo.repo,
        commit_input("my_mesh", vec![p1.clone()], vec![], "seed"),
    )?;
    let link_refs_before = test_repo.list_refs("refs/links/v1")?;

    // Attempt to add a new pair P2 and a duplicate-of-existing pair P1 in
    // one invocation. P2 is valid in isolation; P1 must fail. The whole
    // invocation must be rejected with no new link refs written.
    let p2 = [side_spec("a.txt", 1, 2), side_spec("b.txt", 3, 4)];
    let result = commit_mesh(
        &test_repo.repo,
        commit_input("my_mesh", vec![p2, p1], vec![], "two adds"),
    );
    assert!(result.is_err());
    let link_refs_after = test_repo.list_refs("refs/links/v1")?;
    assert_eq!(
        link_refs_after, link_refs_before,
        "no link refs should be created when validation fails; got {link_refs_after:?}"
    );
    Ok(())
}

#[test]
fn test_commit_mesh_intra_invocation_duplicate_link_rejected() -> Result<()> {
    let test_repo = TestRepo::new()?;
    let sides = [side_spec("file1.txt", 1, 5), side_spec("file2.txt", 10, 15)];
    let err = commit_mesh(
        &test_repo.repo,
        commit_input("my_mesh", vec![sides.clone(), sides], vec![], "dup"),
    )
    .unwrap_err()
    .to_string();
    assert!(
        err.contains("more than once") || err.contains("appears"),
        "expected intra-invocation duplicate error, got: {err}"
    );
    Ok(())
}
