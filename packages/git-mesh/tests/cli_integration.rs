mod support;

use anyhow::Result;
use serde_json::Value;
use std::os::unix::fs::PermissionsExt;
use support::TestRepo;

#[test]
fn cli_lists_and_shows_meshes() -> Result<()> {
    let test_repo = TestRepo::new()?;
    test_repo.mesh_stdout([
        "commit",
        "alpha",
        "--link",
        "file1.txt#L1-L5:file2.txt#L10-L15",
        "-m",
        "Alpha subject",
    ])?;
    test_repo.mesh_stdout([
        "commit",
        "beta",
        "--link",
        "file3.txt#L1-L5:file4.txt#L10-L15",
        "-m",
        "Beta subject",
    ])?;

    let list = test_repo.mesh_stdout(std::iter::empty::<&str>())?;
    assert!(list.contains("alpha\t1 links\tAlpha subject"));
    assert!(list.contains("beta\t1 links\tBeta subject"));

    let show = test_repo.mesh_stdout(["alpha"])?;
    assert!(show.contains("mesh alpha"));
    assert!(show.contains("Alpha subject"));
    assert!(show.contains("Links (1):"));
    assert!(show.contains("  file1.txt#L1-L5:file2.txt#L10-L15"));
    assert!(!show.contains("full-link"));

    let show_oneline = test_repo.mesh_stdout(["alpha", "--oneline"])?;
    assert!(!show_oneline.contains("mesh alpha"));
    assert!(!show_oneline.contains("Author:"));
    assert!(!show_oneline.contains("Alpha subject"));
    assert!(show_oneline.contains("  file1.txt#L1-L5:file2.txt#L10-L15"));

    let formatted = test_repo.mesh_stdout(["alpha", "--format=%m %s%n%L%n%l links"])?;
    assert!(formatted.contains("alpha Alpha subject"));
    assert!(formatted.contains("  file1.txt#L1-L5:file2.txt#L10-L15"));
    assert!(formatted.contains("1 links"));

    Ok(())
}

#[test]
fn cli_show_matches_spec_read_output_format() -> Result<()> {
    let test_repo = TestRepo::new()?;
    test_repo.mesh_stdout([
        "commit",
        "alpha",
        "--link",
        "file1.txt#L1-L5:file2.txt#L10-L15",
        "-m",
        "Alpha subject",
    ])?;

    let show = test_repo.mesh_stdout(["alpha"])?;
    let mut lines = show.lines();
    assert_eq!(lines.next(), Some("mesh alpha"));
    let commit_line = lines.next().expect("commit line");
    // Per §10.4, commit is full sha by default (40 hex chars).
    let commit_sha = commit_line
        .strip_prefix("commit ")
        .expect("commit line prefix");
    assert_eq!(
        commit_sha.len(),
        40,
        "commit sha should be full length by default, got `{commit_sha}`"
    );
    assert!(commit_sha.chars().all(|c| c.is_ascii_hexdigit()));
    let author_line = lines.next().expect("author line");
    assert!(author_line.starts_with("Author: "));
    let date_line = lines.next().expect("date line");
    assert!(date_line.starts_with("Date:   "));
    assert_eq!(lines.next(), Some(""));
    assert_eq!(lines.next(), Some("    Alpha subject"));
    assert_eq!(lines.next(), Some(""));
    assert_eq!(lines.next(), Some("Links (1):"));
    let link_line = lines.next().expect("link line");
    // Format: "    <short-sha>  <rangeA>:<rangeB>" (4-space indent, 2-space gap).
    let rest = link_line
        .strip_prefix("    ")
        .expect("4-space indent on link line");
    let (short, pair) = rest
        .split_once("  ")
        .expect("two-space gap between short sha and pair");
    assert_eq!(short.len(), 8, "short sha is 8 chars by default");
    assert!(short.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(pair, "file1.txt#L1-L5:file2.txt#L10-L15");

    // --no-abbrev expands the link sha to 40 chars; commit already full.
    let show_full = test_repo.mesh_stdout(["alpha", "--no-abbrev"])?;
    let link_line_full = show_full
        .lines()
        .find(|l| l.contains("file1.txt#L1-L5:file2.txt#L10-L15"))
        .expect("link line in --no-abbrev output");
    let rest_full = link_line_full.strip_prefix("    ").expect("indent");
    let (short_full, _) = rest_full.split_once("  ").expect("gap");
    assert_eq!(short_full.len(), 40);

    Ok(())
}

#[test]
fn cli_stale_reports_drift() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    test_repo.mesh_stdout([
        "commit",
        "my-mesh",
        "--link",
        "file1.txt#L1-L5:file2.txt#L10-L15",
        "-m",
        "Track ranges",
    ])?;

    test_repo.write_file(
        "file1.txt",
        "prefix\nline1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n",
    )?;
    test_repo.commit_all("shift file1 lines")?;

    let stale = test_repo.mesh_stdout(["stale", "my-mesh"])?;
    assert!(stale.contains("1 stale of 1 links"));
    assert!(stale.contains("MOVED"));
    assert!(stale.contains("file1.txt#L1-L5"));

    Ok(())
}

#[test]
fn cli_stale_supports_exit_code_and_machine_formats() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    test_repo.mesh_stdout([
        "commit",
        "my-mesh",
        "--link",
        "file1.txt#L1-L5:file2.txt#L10-L15",
        "-m",
        "Track ranges",
    ])?;

    let fresh = test_repo.mesh_output(["stale", "my-mesh", "--exit-code"])?;
    assert_eq!(fresh.status.code(), Some(0));

    test_repo.write_file(
        "file1.txt",
        "prefix\nline1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n",
    )?;
    test_repo.commit_all("shift file1 lines")?;

    let stale = test_repo.mesh_output(["stale", "my-mesh", "--exit-code"])?;
    assert_eq!(stale.status.code(), Some(1));

    let porcelain = test_repo.mesh_stdout(["stale", "my-mesh", "--format=porcelain"])?;
    assert!(porcelain.contains("mesh=my-mesh"));
    assert!(porcelain.contains("status=MOVED"));
    assert!(porcelain.contains("pair=file1.txt#L1-L5:file2.txt#L10-L15"));
    assert!(porcelain.contains("currentPair=file1.txt#L2-L6:file2.txt#L10-L15"));

    let json = test_repo.mesh_stdout(["stale", "my-mesh", "--format=json"])?;
    let payload: Value = serde_json::from_str(&json)?;
    assert_eq!(payload["version"], 1);
    assert_eq!(payload["meshes"][0]["name"], "my-mesh");
    assert_eq!(payload["meshes"][0]["stale_count"], 1);
    assert_eq!(
        payload["meshes"][0]["links"][0]["pair"],
        "file1.txt#L1-L5:file2.txt#L10-L15"
    );
    assert_eq!(
        payload["meshes"][0]["links"][0]["current_pair"],
        "file1.txt#L2-L6:file2.txt#L10-L15"
    );
    assert_eq!(payload["meshes"][0]["links"][0]["status"], "MOVED");

    let junit = test_repo.mesh_stdout(["stale", "my-mesh", "--format=junit"])?;
    assert!(junit.contains("<testsuite name=\"git-mesh stale\""));
    assert!(junit.contains("failures=\"1\""));
    assert!(junit.contains(
        "<failure message=\"MOVED file1.txt#L1-L5:file2.txt#L10-L15 -&gt; file1.txt#L2-L6:file2.txt#L10-L15\">"
    ));

    let github_actions = test_repo.mesh_stdout(["stale", "my-mesh", "--format=github-actions"])?;
    assert!(github_actions.contains("::warning file=file1.txt,line=1::"));
    assert!(github_actions.contains("mesh my-mesh%3A MOVED"));

    Ok(())
}

#[test]
fn cli_stale_includes_culprit_and_reconcile_data() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    test_repo.mesh_stdout([
        "commit",
        "my-mesh",
        "--link",
        "file1.txt#L1-L5:file2.txt#L10-L15",
        "-m",
        "Track ranges",
    ])?;

    test_repo.write_file(
        "file1.txt",
        "line1\nline2\nupdated\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n",
    )?;
    test_repo.commit_all("modify file1")?;

    let human = test_repo.mesh_stdout(["stale", "my-mesh"])?;
    assert!(human.contains("caused by"));
    assert!(human.contains("modify file1"));
    assert!(human.contains("reconcile with:"));
    assert!(human.contains("git mesh commit my-mesh --unlink"));

    let porcelain = test_repo.mesh_stdout(["stale", "my-mesh", "--format=porcelain"])?;
    assert!(porcelain.contains("reconcile=git mesh commit my-mesh --unlink"));
    assert!(porcelain.contains("leftCulprit="));
    assert!(porcelain.contains("modify file1"));

    let json = test_repo.mesh_stdout(["stale", "my-mesh", "--format=json"])?;
    let payload: Value = serde_json::from_str(&json)?;
    assert_eq!(
        payload["meshes"][0]["links"][0]["reconcile_command"],
        "git mesh commit my-mesh --unlink file1.txt#L1-L5:file2.txt#L10-L15 --link file1.txt#L1-L5:file2.txt#L10-L15 -m \"...\""
    );
    assert_eq!(
        payload["meshes"][0]["links"][0]["sides"][0]["culprit"]["summary"],
        "modify file1"
    );

    Ok(())
}

#[test]
fn cli_stale_supports_stat_and_patch() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    test_repo.mesh_stdout([
        "commit",
        "my-mesh",
        "--link",
        "file1.txt#L1-L5:file2.txt#L10-L15",
        "-m",
        "Track ranges",
    ])?;

    test_repo.write_file(
        "file1.txt",
        "line1\nline2\nupdated\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n",
    )?;
    test_repo.commit_all("modify file1")?;

    let stat = test_repo.mesh_stdout(["stale", "my-mesh", "--stat"])?;
    assert!(stat.contains("MODIFIED"));
    assert!(
        stat.contains("file1.txt#L1-L5:file2.txt#L10-L15 -> file1.txt#L1-L5:file2.txt#L10-L15")
    );
    assert!(!stat.contains("reconcile with:"));
    assert!(!stat.contains("├─"));

    let patch = test_repo.mesh_stdout(["stale", "my-mesh", "--patch"])?;
    assert!(patch.contains("--- file1.txt#L1-L5"));
    assert!(patch.contains("+++ file1.txt#L1-L5"));
    assert!(patch.contains("@@"));
    assert!(patch.contains("-line3"));
    assert!(patch.contains("+updated"));

    Ok(())
}

#[test]
fn cli_stale_without_name_scans_all_meshes_and_since_filters_links() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    let before = test_repo.head_sha()?;

    test_repo.mesh_stdout([
        "commit",
        "old-mesh",
        "--link",
        "file1.txt#L1-L5:file2.txt#L10-L15",
        "-m",
        "Older mesh",
    ])?;

    test_repo.write_file("new-anchor.txt", "a\nb\nc\nd\ne\nf\n")?;
    test_repo.commit_all("add new anchor file")?;
    let since = test_repo.head_sha()?;

    test_repo.mesh_stdout([
        "commit",
        "new-mesh",
        "--link",
        "new-anchor.txt#L1-L5:file3.txt#L1-L5",
        "-m",
        "Newer mesh",
    ])?;

    let scan_all = test_repo.mesh_stdout(["stale", "--format=porcelain"])?;
    assert!(scan_all.contains("mesh=old-mesh"));
    assert!(scan_all.contains("mesh=new-mesh"));

    let filtered = test_repo.mesh_stdout(["stale", "--format=porcelain", "--since", &since])?;
    assert!(!filtered.contains("mesh=old-mesh"));
    assert!(filtered.contains("mesh=new-mesh"));

    let filtered_old =
        test_repo.mesh_stdout(["stale", "old-mesh", "--format=json", "--since", &since])?;
    let payload: Value = serde_json::from_str(&filtered_old)?;
    assert_eq!(payload["meshes"][0]["name"], "old-mesh");
    assert_eq!(payload["meshes"][0]["link_count"], 0);

    let unfiltered_old =
        test_repo.mesh_stdout(["stale", "old-mesh", "--format=json", "--since", &before])?;
    let payload: Value = serde_json::from_str(&unfiltered_old)?;
    assert_eq!(payload["meshes"][0]["link_count"], 1);

    Ok(())
}

#[test]
fn cli_commit_supports_unlink_amend_and_anchor() -> Result<()> {
    let test_repo = TestRepo::new()?;
    let anchor = test_repo.head_sha()?;

    test_repo.mesh_stdout([
        "commit",
        "my-mesh",
        "--anchor",
        &anchor,
        "--link",
        "file1.txt#L1-L5:file2.txt#L10-L15",
        "-m",
        "Initial message",
    ])?;

    let initial_show = test_repo.mesh_stdout(["my-mesh"])?;
    assert!(initial_show.contains("Initial message"));

    test_repo.mesh_stdout([
        "commit",
        "my-mesh",
        "--unlink",
        "file1.txt#L1-L5:file2.txt#L10-L15",
        "--link",
        "file1.txt#L2-L6:file2.txt#L10-L15",
        "-m",
        "Reconcile drift",
    ])?;

    let reconciled_show = test_repo.mesh_stdout(["my-mesh"])?;
    assert!(reconciled_show.contains("Reconcile drift"));

    test_repo.mesh_stdout(["commit", "my-mesh", "--amend", "-m", "Reworded message"])?;

    let amended_show = test_repo.mesh_stdout(["my-mesh"])?;
    assert!(amended_show.contains("Reworded message"));

    Ok(())
}

#[test]
fn cli_commit_supports_message_file_edit_and_no_ignore_whitespace() -> Result<()> {
    let mut test_repo = TestRepo::new()?;

    test_repo.write_file("message.txt", "Message from file\n\nBody line\n")?;
    test_repo.mesh_stdout([
        "commit",
        "file-message-mesh",
        "--link",
        "file1.txt#L1-L5:file2.txt#L10-L15",
        "-F",
        "message.txt",
    ])?;

    let show = test_repo.mesh_stdout(["file-message-mesh"])?;
    assert!(show.contains("Message from file"));
    assert!(show.contains("Body line"));

    let editor = test_repo.dir.path().join("write-editor-message.sh");
    test_repo.write_file(
        "write-editor-message.sh",
        "#!/bin/sh\ncat <<'EOF' > \"$1\"\nEdited subject\n\nEdited body\nEOF\n",
    )?;
    std::fs::set_permissions(&editor, std::fs::Permissions::from_mode(0o755))?;

    test_repo.mesh_stdout_with_env(
        ["commit", "file-message-mesh", "--amend", "--edit"],
        [("GIT_EDITOR", editor.to_str().unwrap())],
    )?;

    let amended = test_repo.mesh_stdout(["file-message-mesh"])?;
    assert!(amended.contains("Edited subject"));
    assert!(amended.contains("Edited body"));

    test_repo.mesh_stdout([
        "commit",
        "strict-whitespace",
        "--link",
        "file3.txt#L1-L5:file4.txt#L10-L15",
        "--no-ignore-whitespace",
        "-m",
        "Track exact whitespace",
    ])?;

    test_repo.write_file(
        "file3.txt",
        "line1\nline2\nline 3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n",
    )?;
    test_repo.commit_all("whitespace only change")?;

    let stale = test_repo.mesh_stdout(["stale", "strict-whitespace", "--format=json"])?;
    let payload: Value = serde_json::from_str(&stale)?;
    assert_eq!(payload["meshes"][0]["links"][0]["status"], "MODIFIED");

    Ok(())
}

#[test]
fn cli_rm_mv_and_restore_work() -> Result<()> {
    let test_repo = TestRepo::new()?;
    test_repo.mesh_stdout([
        "commit",
        "mesh-a",
        "--link",
        "file1.txt#L1-L5:file2.txt#L10-L15",
        "-m",
        "Original state",
    ])?;
    let first_tip = test_repo.read_ref("refs/meshes/v1/mesh-a")?;

    test_repo.mesh_stdout([
        "commit",
        "mesh-a",
        "--link",
        "file3.txt#L1-L5:file4.txt#L10-L15",
        "-m",
        "Expanded state",
    ])?;

    test_repo.mesh_stdout(["restore", "mesh-a", "HEAD~1"])?;
    let restored_show = test_repo.mesh_stdout(["mesh-a"])?;
    assert!(restored_show.contains("Original state"));
    assert_ne!(test_repo.read_ref("refs/meshes/v1/mesh-a")?, first_tip);

    test_repo.mesh_stdout(["mv", "mesh-a", "mesh-b"])?;
    let renamed_show = test_repo.mesh_stdout(["mesh-b"])?;
    assert!(renamed_show.contains("mesh mesh-b"));
    let old_err = test_repo.mesh_stderr(["mesh-a"])?;
    assert!(old_err.contains("error:"));

    test_repo.mesh_stdout(["rm", "mesh-b"])?;
    let deleted_err = test_repo.mesh_stderr(["mesh-b"])?;
    assert!(deleted_err.contains("error:"));

    Ok(())
}

#[test]
fn cli_commit_retries_implicit_tip_race() -> Result<()> {
    let test_repo = TestRepo::new()?;
    test_repo.mesh_stdout([
        "commit",
        "mesh-a",
        "--link",
        "file1.txt#L1-L5:file2.txt#L10-L15",
        "-m",
        "Original state",
    ])?;

    let hook_command = "blob1=$(git rev-parse --verify HEAD:file3.txt)\nblob2=$(git rev-parse --verify HEAD:file4.txt)\nlink_blob=$(printf 'anchor %s\\ncreated 2026-01-01T00:00:00Z\\nside 1 5 %s same-commit true\\tfile3.txt\\nside 10 15 %s same-commit true\\tfile4.txt\\n' \"$(git rev-parse HEAD)\" \"$blob1\" \"$blob2\" | git hash-object -w --stdin)\ngit update-ref refs/links/v1/cli-raced-link \"$link_blob\"\nlinks_blob=$(printf 'cli-raced-link\\n' | git hash-object -w --stdin)\ntree=$(printf '100644 blob %s\\tlinks\\n' \"$links_blob\" | git mktree)\ncommit=$(GIT_AUTHOR_NAME='Test User' GIT_AUTHOR_EMAIL='test@example.com' GIT_COMMITTER_NAME='Test User' GIT_COMMITTER_EMAIL='test@example.com' git commit-tree \"$tree\" -p \"$(git rev-parse refs/meshes/v1/mesh-a)\" -m 'Concurrent state')\ngit update-ref refs/meshes/v1/mesh-a \"$commit\"";
    test_repo.mesh_stdout_with_env(
        [
            "commit",
            "mesh-a",
            "--link",
            "file1.txt#L2-L6:file2.txt#L10-L15",
            "-m",
            "Retried state",
        ],
        [(
            "GIT_MESH_TEST_HOOK",
            format!("commit_mesh_before_transaction:once:{hook_command}"),
        )],
    )?;

    let show = test_repo.mesh_stdout(["mesh-a"])?;
    assert!(show.contains("Retried state"));
    assert!(show.contains("file3.txt#L1-L5:file4.txt#L10-L15"));
    assert!(show.contains("file1.txt#L2-L6:file2.txt#L10-L15"));
    Ok(())
}

#[test]
fn cli_show_supports_at_log_diff_and_no_abbrev() -> Result<()> {
    let test_repo = TestRepo::new()?;
    test_repo.mesh_stdout([
        "commit",
        "mesh-history",
        "--link",
        "file1.txt#L1-L5:file2.txt#L10-L15",
        "-m",
        "First state",
    ])?;
    test_repo.mesh_stdout([
        "commit",
        "mesh-history",
        "--link",
        "file3.txt#L1-L5:file4.txt#L10-L15",
        "-m",
        "Second state",
    ])?;

    let historical = test_repo.mesh_stdout(["mesh-history", "--at", "HEAD~1"])?;
    assert!(historical.contains("First state"));
    assert!(!historical.contains("file3.txt#L1-L5:file4.txt#L10-L15"));

    let no_abbrev = test_repo.mesh_stdout(["mesh-history", "--no-abbrev"])?;
    let head_commit = test_repo.read_ref("refs/meshes/v1/mesh-history")?;
    assert!(no_abbrev.contains(&format!("commit {head_commit}")));

    let log = test_repo.mesh_stdout(["mesh-history", "--log", "--limit", "1"])?;
    assert!(log.contains(&head_commit));
    assert!(log.contains("Second state"));
    assert!(!log.contains("First state"));

    let log_oneline = test_repo.mesh_stdout(["mesh-history", "--log", "--oneline"])?;
    assert!(log_oneline.contains("Second state"));
    assert!(log_oneline.contains("First state"));

    let diff = test_repo.mesh_stdout(["mesh-history", "--diff", "HEAD~1..HEAD"])?;
    assert!(diff.contains("diff "));
    assert!(diff.contains("+ file3.txt#L1-L5:file4.txt#L10-L15 @"));
    assert!(!diff.contains("- file1.txt#L1-L5:file2.txt#L10-L15 @"));

    Ok(())
}

#[test]
fn cli_rejects_reserved_names() -> Result<()> {
    let test_repo = TestRepo::new()?;
    let stderr = test_repo.mesh_stderr([
        "commit",
        "stale",
        "--link",
        "file1.txt#L1-L5:file2.txt#L10-L15",
        "-m",
        "bad",
    ])?;
    assert!(stderr.contains("mesh name `stale` is reserved"));

    let mv_stderr = test_repo.mesh_stderr(["mv", "missing", "doctor"])?;
    assert!(mv_stderr.contains("mesh name `doctor` is reserved"));

    Ok(())
}

#[test]
fn cli_doctor_reports_ok_and_broken_meshes() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    test_repo.mesh_stdout([
        "commit",
        "healthy",
        "--link",
        "file1.txt#L1-L5:file2.txt#L10-L15",
        "-m",
        "Healthy mesh",
    ])?;

    let ok = test_repo.mesh_stdout(["doctor"])?;
    assert!(ok.contains("mesh doctor: ok"));

    test_repo.create_mesh_fixture("broken", "Broken mesh", &["missing-link"])?;
    let broken = test_repo.mesh_output(["doctor"])?;
    assert_eq!(broken.status.code(), Some(1));
    let stdout = String::from_utf8(broken.stdout)?;
    assert!(stdout.contains("mesh doctor: found 1 issue(s)"));
    assert!(stdout.contains("mesh `broken` is unreadable"));

    Ok(())
}
