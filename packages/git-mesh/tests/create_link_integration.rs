mod support;

use anyhow::Result;
use git_mesh::{CopyDetection, CreateLinkInput, SideSpec, create_link};

use support::TestRepo;

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
                start: 100,
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
        sides: [side2, side1],
        anchor_sha: None,
        id: None,
    };

    let (_, link1) = create_link(&test_repo.repo, input1)?;
    let (_, link2) = create_link(&test_repo.repo, input2)?;

    assert_eq!(link1.sides[0].path, link2.sides[0].path);
    assert_eq!(link1.sides[1].path, link2.sides[1].path);
    Ok(())
}

#[test]
fn test_create_link_uses_anchor_commit_blob_and_range() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    test_repo.write_file("file1.txt", "a1\na2\na3\n")?;
    test_repo.write_file("file2.txt", "b1\nb2\nb3\n")?;
    test_repo.commit_all("anchor")?;
    let anchor_sha = test_repo.head_sha()?;
    let anchor_file1_blob =
        test_repo.git_output(["rev-parse", &format!("{anchor_sha}:file1.txt")])?;
    let anchor_file2_blob =
        test_repo.git_output(["rev-parse", &format!("{anchor_sha}:file2.txt")])?;

    test_repo.write_file("file1.txt", "new1\nnew2\n")?;
    test_repo.write_file("file2.txt", "new1\nnew2\nnew3\nnew4\n")?;
    test_repo.commit_all("head diverged")?;

    let input = CreateLinkInput {
        sides: [
            SideSpec {
                path: "file1.txt".to_string(),
                start: 3,
                end: 3,
                copy_detection: None,
                ignore_whitespace: None,
            },
            SideSpec {
                path: "file2.txt".to_string(),
                start: 1,
                end: 3,
                copy_detection: None,
                ignore_whitespace: None,
            },
        ],
        anchor_sha: Some(anchor_sha.clone()),
        id: None,
    };

    let (id, link) = create_link(&test_repo.repo, input)?;

    assert_eq!(link.anchor_sha, anchor_sha);
    assert_eq!(link.sides[0].path, "file1.txt");
    assert_eq!(link.sides[0].start, 3);
    assert_eq!(link.sides[0].end, 3);
    assert_eq!(link.sides[0].blob, anchor_file1_blob);
    assert_eq!(link.sides[1].path, "file2.txt");
    assert_eq!(link.sides[1].start, 1);
    assert_eq!(link.sides[1].end, 3);
    assert_eq!(link.sides[1].blob, anchor_file2_blob);

    let ref_oid = test_repo.read_ref(&format!("refs/links/v1/{id}"))?;
    let link_blob = test_repo.git_output(["cat-file", "-p", &ref_oid])?;
    assert!(link_blob.contains(&format!("anchor {anchor_sha}")));
    assert!(link_blob.contains(&format!(
        "side 3 3 {} same-commit true\tfile1.txt",
        anchor_file1_blob
    )));
    assert!(link_blob.contains(&format!(
        "side 1 3 {} same-commit true\tfile2.txt",
        anchor_file2_blob
    )));

    Ok(())
}
