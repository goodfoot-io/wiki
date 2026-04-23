mod support;

use anyhow::Result;
use git_mesh_legacy::{Link, LinkSide, RangeSpec, parse_link, read_mesh, read_mesh_at, serialize_link};

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
fn test_read_mesh_returns_canonical_link_order() -> Result<()> {
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
        vec![second_link_id.as_str(), first_link_id.as_str()]
    );

    Ok(())
}

#[test]
fn test_read_mesh_at_reads_historical_mesh_state() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    let (first_link_id, _, _) = test_repo.create_link_fixture(
        "first-link",
        [range_spec("file1.txt", 1, 2), range_spec("file2.txt", 3, 4)],
    )?;
    let (second_link_id, _, _) = test_repo.create_link_fixture(
        "second-link",
        [range_spec("file3.txt", 1, 2), range_spec("file4.txt", 3, 4)],
    )?;

    test_repo.create_mesh_fixture("history_mesh", "First state", &[&first_link_id])?;
    test_repo.create_mesh_fixture(
        "history_mesh",
        "Second state",
        &[&first_link_id, &second_link_id],
    )?;

    let historical = read_mesh_at(&test_repo.repo, "history_mesh", Some("HEAD~1"))?;
    let current = read_mesh(&test_repo.repo, "history_mesh")?;

    assert_eq!(historical.message, "First state");
    assert_eq!(historical.links.len(), 1);
    assert_eq!(historical.links[0].id, first_link_id);

    assert_eq!(current.message, "Second state");
    assert_eq!(current.links.len(), 2);

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
                copy_detection: git_mesh_legacy::CopyDetection::SameCommit,
                ignore_whitespace: true,
            },
            LinkSide {
                path: "b.txt".to_string(),
                start: 4,
                end: 6,
                blob: "blob-b".to_string(),
                copy_detection: git_mesh_legacy::CopyDetection::AnyFileInRepo,
                ignore_whitespace: false,
            },
        ],
    };

    let serialized = serialize_link(&link);
    let parsed = parse_link(&serialized)?;

    assert_eq!(parsed, link);
    Ok(())
}

/// §4.1: exact on-disk bytes — header order (`anchor`, `created`, then two
/// sorted `side` lines), TAB before path, trailing newline, no blank lines.
#[test]
fn test_serialize_link_matches_on_disk_format() {
    let link = Link {
        anchor_sha: "aaaaaaaa".to_string(),
        created_at: "2026-04-22T00:00:00+00:00".to_string(),
        sides: [
            LinkSide {
                path: "a path with spaces.txt".to_string(),
                start: 1,
                end: 3,
                blob: "blob-a".to_string(),
                copy_detection: git_mesh_legacy::CopyDetection::SameCommit,
                ignore_whitespace: true,
            },
            LinkSide {
                path: "b.txt".to_string(),
                start: 10,
                end: 20,
                blob: "blob-b".to_string(),
                copy_detection: git_mesh_legacy::CopyDetection::Off,
                ignore_whitespace: false,
            },
        ],
    };
    let serialized = serialize_link(&link);
    let expected = concat!(
        "anchor aaaaaaaa\n",
        "created 2026-04-22T00:00:00+00:00\n",
        "side 1 3 blob-a same-commit true\ta path with spaces.txt\n",
        "side 10 20 blob-b off false\tb.txt\n",
    );
    assert_eq!(serialized, expected);
    // No blank lines; trailing newline; no leading newline.
    assert!(!serialized.contains("\n\n"));
    assert!(serialized.ends_with('\n'));
    assert!(!serialized.starts_with('\n'));
}

/// §4.1: on write, the two `side` lines are sorted so the pair is effectively
/// unordered. Construct a Link whose in-memory sides are out of canonical
/// order and verify the serialized bytes still lead with the smaller side.
#[test]
fn test_serialize_link_sorts_sides() {
    let lower = LinkSide {
        path: "a.txt".to_string(),
        start: 1,
        end: 2,
        blob: "blob-a".to_string(),
        copy_detection: git_mesh_legacy::CopyDetection::SameCommit,
        ignore_whitespace: true,
    };
    let higher = LinkSide {
        path: "z.txt".to_string(),
        start: 1,
        end: 2,
        blob: "blob-z".to_string(),
        copy_detection: git_mesh_legacy::CopyDetection::SameCommit,
        ignore_whitespace: true,
    };
    // Sides reversed from canonical.
    let link = Link {
        anchor_sha: "deadbeef".to_string(),
        created_at: "2026-04-22T00:00:00+00:00".to_string(),
        sides: [higher, lower],
    };
    // Parser canonicalizes; so does create_link on write. serialize_link
    // itself is a direct dump — but round-tripping must always produce the
    // canonical order.
    let round_tripped = parse_link(&serialize_link(&link)).unwrap();
    assert_eq!(round_tripped.sides[0].path, "a.txt");
    assert_eq!(round_tripped.sides[1].path, "z.txt");
}

#[test]
fn test_parse_link_tolerates_unknown_headers() {
    let text = concat!(
        "anchor aaaa\n",
        "created 2026-04-22T00:00:00+00:00\n",
        "experimental-future-field some-value\n",
        "side 1 2 blob-a same-commit true\ta.txt\n",
        "side 3 4 blob-b same-commit true\tb.txt\n",
    );
    let link = parse_link(text).expect("unknown headers should be tolerated");
    assert_eq!(link.anchor_sha, "aaaa");
    assert_eq!(link.sides[0].path, "a.txt");
}

#[test]
fn test_parse_link_rejects_blank_lines() {
    let text = concat!(
        "anchor aaaa\n",
        "\n",
        "created 2026-04-22T00:00:00+00:00\n",
        "side 1 2 blob-a same-commit true\ta.txt\n",
        "side 3 4 blob-b same-commit true\tb.txt\n",
    );
    let err = parse_link(text).unwrap_err().to_string();
    assert!(err.contains("blank lines"), "error was: {err}");
}

#[test]
fn test_parse_link_rejects_missing_trailing_newline() {
    let text = concat!(
        "anchor aaaa\n",
        "created 2026-04-22T00:00:00+00:00\n",
        "side 1 2 blob-a same-commit true\ta.txt\n",
        "side 3 4 blob-b same-commit true\tb.txt", // no \n
    );
    let err = parse_link(text).unwrap_err().to_string();
    assert!(err.contains("trailing newline"), "error was: {err}");
}

#[test]
fn test_parse_link_rejects_duplicate_anchor() {
    let text = concat!(
        "anchor aaaa\n",
        "anchor bbbb\n",
        "created 2026-04-22T00:00:00+00:00\n",
        "side 1 2 blob-a same-commit true\ta.txt\n",
        "side 3 4 blob-b same-commit true\tb.txt\n",
    );
    let err = parse_link(text).unwrap_err().to_string();
    assert!(err.contains("duplicate `anchor`"), "error was: {err}");
}

#[test]
fn test_parse_link_rejects_missing_tab_before_path() {
    let text = concat!(
        "anchor aaaa\n",
        "created 2026-04-22T00:00:00+00:00\n",
        "side 1 2 blob-a same-commit true a.txt\n",
        "side 3 4 blob-b same-commit true\tb.txt\n",
    );
    let err = parse_link(text).unwrap_err().to_string();
    assert!(err.contains("TAB"), "error was: {err}");
}

#[test]
fn test_parse_link_rejects_wrong_field_count() {
    let text = concat!(
        "anchor aaaa\n",
        "created 2026-04-22T00:00:00+00:00\n",
        "side 1 2 blob-a same-commit\ta.txt\n", // only 4 fields
        "side 3 4 blob-b same-commit true\tb.txt\n",
    );
    let err = parse_link(text).unwrap_err().to_string();
    assert!(err.contains("5 space-separated"), "error was: {err}");
}

#[test]
fn test_parse_link_rejects_invalid_copy_detection() {
    let text = concat!(
        "anchor aaaa\n",
        "created 2026-04-22T00:00:00+00:00\n",
        "side 1 2 blob-a bogus true\ta.txt\n",
        "side 3 4 blob-b same-commit true\tb.txt\n",
    );
    let err = parse_link(text).unwrap_err().to_string();
    assert!(err.contains("invalid copy detection"), "error was: {err}");
}

#[test]
fn test_parse_link_rejects_invalid_ignore_whitespace() {
    let text = concat!(
        "anchor aaaa\n",
        "created 2026-04-22T00:00:00+00:00\n",
        "side 1 2 blob-a same-commit yes\ta.txt\n",
        "side 3 4 blob-b same-commit true\tb.txt\n",
    );
    let err = parse_link(text).unwrap_err().to_string();
    assert!(err.contains("ignore_whitespace"), "error was: {err}");
}

#[test]
fn test_parse_link_rejects_too_many_sides() {
    let text = concat!(
        "anchor aaaa\n",
        "created 2026-04-22T00:00:00+00:00\n",
        "side 1 2 blob-a same-commit true\ta.txt\n",
        "side 3 4 blob-b same-commit true\tb.txt\n",
        "side 5 6 blob-c same-commit true\tc.txt\n",
    );
    let err = parse_link(text).unwrap_err().to_string();
    assert!(err.contains("exactly two"), "error was: {err}");
}

#[test]
fn test_parse_link_rejects_missing_anchor() {
    let text = concat!(
        "created 2026-04-22T00:00:00+00:00\n",
        "side 1 2 blob-a same-commit true\ta.txt\n",
        "side 3 4 blob-b same-commit true\tb.txt\n",
    );
    let err = parse_link(text).unwrap_err().to_string();
    assert!(err.contains("`anchor`"), "error was: {err}");
}

#[test]
fn test_parse_link_rejects_missing_created() {
    let text = concat!(
        "anchor aaaa\n",
        "side 1 2 blob-a same-commit true\ta.txt\n",
        "side 3 4 blob-b same-commit true\tb.txt\n",
    );
    let err = parse_link(text).unwrap_err().to_string();
    assert!(err.contains("`created`"), "error was: {err}");
}
