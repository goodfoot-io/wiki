mod support;

use anyhow::Result;
use git_mesh_legacy::{LinkStatus, RangeSpec, stale_mesh};

use support::TestRepo;

fn range_spec(path: &str, start: u32, end: u32) -> RangeSpec {
    RangeSpec {
        path: path.to_string(),
        start,
        end,
    }
}

#[test]
fn test_stale_mesh_fresh() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    let (link_id, _, _) = test_repo.create_link_fixture(
        "fresh-link",
        [
            range_spec("file1.txt", 1, 5),
            range_spec("file2.txt", 10, 15),
        ],
    )?;
    test_repo.create_mesh_fixture("my_mesh", "Fresh mesh fixture", &[&link_id])?;

    let resolved = stale_mesh(&test_repo.repo, "my_mesh")?;
    assert_eq!(resolved.links[0].status, LinkStatus::Fresh);
    Ok(())
}

#[test]
fn test_stale_mesh_tracks_line_movement_through_history() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    let (link_id, _, _) = test_repo.create_link_fixture(
        "moved-link",
        [
            range_spec("file1.txt", 1, 5),
            range_spec("file2.txt", 10, 15),
        ],
    )?;
    test_repo.create_mesh_fixture("my_mesh", "Moved mesh fixture", &[&link_id])?;
    test_repo.write_file(
        "file1.txt",
        "prefix\nline1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n",
    )?;
    test_repo.commit_all("shift file1 lines")?;

    let resolved = stale_mesh(&test_repo.repo, "my_mesh")?;
    assert_eq!(resolved.links[0].status, LinkStatus::Moved);
    let current = resolved.links[0].sides[0].current.as_ref().unwrap();
    assert_eq!(current.path, "file1.txt");
    assert_eq!((current.start, current.end), (2, 6));
    Ok(())
}

#[test]
fn test_stale_mesh_tracks_rename_history() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    let (link_id, _, _) = test_repo.create_link_fixture(
        "rename-link",
        [
            range_spec("file1.txt", 1, 5),
            range_spec("file2.txt", 10, 15),
        ],
    )?;
    test_repo.create_mesh_fixture("my_mesh", "Rename mesh fixture", &[&link_id])?;
    test_repo.rename_file("file1.txt", "renamed.txt")?;
    test_repo.commit_all("rename file1")?;

    let resolved = stale_mesh(&test_repo.repo, "my_mesh")?;
    assert_eq!(resolved.links[0].status, LinkStatus::Moved);
    let current = resolved.links[0].sides[0].current.as_ref().unwrap();
    assert_eq!(current.path, "renamed.txt");
    assert_eq!((current.start, current.end), (1, 5));
    Ok(())
}

#[test]
fn test_stale_mesh_reports_modified_and_rewritten() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    let (modified_id, _, _) = test_repo.create_link_fixture(
        "modified-link",
        [
            range_spec("file1.txt", 1, 5),
            range_spec("file2.txt", 10, 15),
        ],
    )?;
    test_repo.create_mesh_fixture("modified_mesh", "Modified mesh fixture", &[&modified_id])?;
    test_repo.write_file(
        "file1.txt",
        "line1\nline2\nupdated\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n",
    )?;
    test_repo.commit_all("modify file1")?;

    let modified = stale_mesh(&test_repo.repo, "modified_mesh")?;
    assert_eq!(modified.links[0].status, LinkStatus::Modified);
    assert_eq!(
        modified.links[0].sides[0]
            .culprit
            .as_ref()
            .map(|c| c.summary.as_str()),
        Some("modify file1")
    );
    assert!(
        modified.links[0]
            .reconcile_command
            .contains("--link file1.txt#L1-L5:file2.txt#L10-L15")
    );

    let (rewritten_id, _, _) = test_repo.create_link_fixture(
        "rewritten-link",
        [
            range_spec("file3.txt", 1, 5),
            range_spec("file4.txt", 10, 15),
        ],
    )?;
    test_repo.create_mesh_fixture("rewritten_mesh", "Rewritten mesh fixture", &[&rewritten_id])?;
    test_repo.write_file(
        "file3.txt",
        "MOD\nMOD\nMOD\nMOD\nMOD\nline6\nline7\nline8\nline9\nline10\n",
    )?;
    test_repo.commit_all("rewrite file3")?;

    let rewritten = stale_mesh(&test_repo.repo, "rewritten_mesh")?;
    assert_eq!(rewritten.links[0].status, LinkStatus::Rewritten);
    assert_eq!(
        rewritten.links[0].sides[0]
            .culprit
            .as_ref()
            .map(|c| c.summary.as_str()),
        Some("rewrite file3")
    );
    Ok(())
}

#[test]
fn test_stale_mesh_reports_missing_after_delete() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    let (link_id, _, _) = test_repo.create_link_fixture(
        "missing-link",
        [
            range_spec("file1.txt", 1, 5),
            range_spec("file2.txt", 10, 15),
        ],
    )?;
    test_repo.create_mesh_fixture("my_mesh", "Missing mesh fixture", &[&link_id])?;
    test_repo.remove_file("file1.txt")?;
    test_repo.commit_all("delete file1")?;

    let resolved = stale_mesh(&test_repo.repo, "my_mesh")?;
    assert_eq!(resolved.links[0].status, LinkStatus::Missing);
    assert!(resolved.links[0].sides[0].current.is_none());
    assert!(resolved.links[0].reconcile_command.contains("--unlink"));
    assert!(!resolved.links[0].reconcile_command.contains(" --link "));
    Ok(())
}

#[test]
fn test_stale_mesh_uses_copy_detection_to_follow_copied_file() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    let (link_id, _, _) = test_repo.create_link_fixture(
        "copy-link",
        [
            range_spec("file1.txt", 1, 5),
            range_spec("file2.txt", 10, 15),
        ],
    )?;
    test_repo.create_mesh_fixture("my_mesh", "Copy mesh fixture", &[&link_id])?;
    test_repo.copy_file("file1.txt", "copied.txt")?;
    test_repo.remove_file("file1.txt")?;
    test_repo.commit_all("copy file1 to copied and delete source")?;

    let resolved = stale_mesh(&test_repo.repo, "my_mesh")?;
    assert_eq!(resolved.links[0].status, LinkStatus::Moved);
    let current = resolved.links[0].sides[0].current.as_ref().unwrap();
    assert_eq!(current.path, "copied.txt");
    Ok(())
}

#[test]
fn test_stale_mesh_reports_orphaned_when_anchor_unreachable() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    let (link_id, _, _) = test_repo.create_link_fixture(
        "orphan-link",
        [
            range_spec("file1.txt", 1, 5),
            range_spec("file2.txt", 10, 15),
        ],
    )?;
    let anchor = test_repo.head_sha()?;
    test_repo.create_mesh_fixture("my_mesh", "Orphan mesh fixture", &[&link_id])?;
    test_repo.write_file(
        "file1.txt",
        "changed\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n",
    )?;
    test_repo.commit_all("advance head")?;
    test_repo.run_git(["checkout", "--orphan", "orphaned-head"])?;
    test_repo.run_git(["reset", "--hard"])?;
    test_repo.write_file("new-root.txt", "root\n")?;
    test_repo.commit_all("new root")?;
    test_repo.delete_ref("refs/heads/master").ok();
    test_repo.delete_ref("refs/heads/main").ok();

    let resolved = stale_mesh(&test_repo.repo, "my_mesh")?;
    assert_eq!(resolved.links[0].anchor_sha, anchor);
    assert_eq!(resolved.links[0].status, LinkStatus::Orphaned);
    assert!(
        resolved.links[0]
            .sides
            .iter()
            .all(|side| side.current.is_none())
    );
    Ok(())
}
