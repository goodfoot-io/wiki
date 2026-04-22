mod support;

use anyhow::Result;
use git_mesh::{Link, LinkSide, RangeSpec, parse_link, read_mesh, serialize_link};

use support::TestRepo;

fn range_spec(path: &str, start: u32, end: u32) -> RangeSpec {
    RangeSpec {
        path: path.to_string(),
        start,
        end,
    }
}

#[test]
fn test_read_mesh_returns_full_stored_link_data() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    let (link_id, _, _) = test_repo.create_link_fixture(
        "full-link",
        [
            range_spec("file1.txt", 1, 5),
            range_spec("file2.txt", 10, 15),
        ],
    )?;
    let anchor_sha = test_repo.head_sha()?;
    let file1_blob = test_repo.file_blob_at_head("file1.txt")?;
    let file2_blob = test_repo.file_blob_at_head("file2.txt")?;
    test_repo.create_mesh_fixture("my_mesh", "Stored mesh fixture", &[&link_id])?;

    let mesh = read_mesh(&test_repo.repo, "my_mesh")?;

    assert_eq!(mesh.name, "my_mesh");
    assert_eq!(mesh.message, "Stored mesh fixture");
    assert_eq!(mesh.links.len(), 1);
    assert_eq!(mesh.links[0].id, link_id);
    assert_eq!(mesh.links[0].anchor_sha, anchor_sha);
    assert_eq!(mesh.links[0].sides[0].path, "file1.txt");
    assert_eq!(mesh.links[0].sides[0].start, 1);
    assert_eq!(mesh.links[0].sides[0].end, 5);
    assert_eq!(mesh.links[0].sides[0].blob, file1_blob);
    assert_eq!(mesh.links[0].sides[1].path, "file2.txt");
    assert_eq!(mesh.links[0].sides[1].start, 10);
    assert_eq!(mesh.links[0].sides[1].end, 15);
    assert_eq!(mesh.links[0].sides[1].blob, file2_blob);

    Ok(())
}

#[test]
fn test_read_mesh_preserves_stored_link_order() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    let (first_link_id, _, _) = test_repo.create_link_fixture(
        "z-link",
        [range_spec("file1.txt", 1, 2), range_spec("file2.txt", 3, 4)],
    )?;
    let (second_link_id, _, _) = test_repo.create_link_fixture(
        "a-link",
        [range_spec("file3.txt", 1, 2), range_spec("file4.txt", 3, 4)],
    )?;
    test_repo.create_mesh_fixture(
        "ordered_mesh",
        "Order fixture",
        &[&first_link_id, &second_link_id],
    )?;

    let mesh = read_mesh(&test_repo.repo, "ordered_mesh")?;

    assert_eq!(
        mesh.links
            .iter()
            .map(|link| link.id.as_str())
            .collect::<Vec<_>>(),
        vec![first_link_id.as_str(), second_link_id.as_str()]
    );

    Ok(())
}

#[test]
fn test_read_mesh_fails_cleanly_when_link_ref_is_missing() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    let (link_id, _, _) = test_repo.create_link_fixture(
        "missing-ref-link",
        [range_spec("file1.txt", 1, 2), range_spec("file2.txt", 3, 4)],
    )?;
    test_repo.create_mesh_fixture("broken_mesh", "Broken mesh fixture", &[&link_id])?;
    test_repo.run_git(["update-ref", "-d", &format!("refs/links/v1/{link_id}")])?;

    let error =
        read_mesh(&test_repo.repo, "broken_mesh").expect_err("missing link ref should fail");

    assert!(error.to_string().contains("git command failed"));
    Ok(())
}

#[test]
fn test_parse_serialize_link_round_trip() -> Result<()> {
    let link = Link {
        anchor_sha: "abc123".to_string(),
        created_at: "2026-01-01T00:00:00Z".to_string(),
        sides: [
            LinkSide {
                path: "a.txt".to_string(),
                start: 1,
                end: 3,
                blob: "blob-a".to_string(),
                copy_detection: git_mesh::CopyDetection::SameCommit,
                ignore_whitespace: true,
            },
            LinkSide {
                path: "b.txt".to_string(),
                start: 4,
                end: 6,
                blob: "blob-b".to_string(),
                copy_detection: git_mesh::CopyDetection::AnyFileInRepo,
                ignore_whitespace: false,
            },
        ],
    };

    let serialized = serialize_link(&link);
    let parsed = parse_link(&serialized)?;

    assert_eq!(parsed, link);
    Ok(())
}
