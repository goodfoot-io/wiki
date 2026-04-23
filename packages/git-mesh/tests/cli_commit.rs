mod support;

use anyhow::Result;
use serde_json::Value;
use std::os::unix::fs::PermissionsExt;
use support::TestRepo;

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
