//! CLI: add, rm, message, commit, status, config.

mod support;

use anyhow::Result;
use support::TestRepo;

#[test]
#[ignore]
fn cli_add_stages_range() -> Result<()> {
    let repo = TestRepo::seeded()?;
    repo.mesh_stdout(["add", "m", "file1.txt#L1-L5"])?;
    let status = repo.mesh_stdout(["status", "m"])?;
    assert!(status.contains("file1.txt"));
    assert!(status.contains("L1-L5"));
    Ok(())
}

#[test]
#[ignore]
fn cli_add_accepts_at_anchor() -> Result<()> {
    let repo = TestRepo::seeded()?;
    let head = repo.head_sha()?;
    repo.mesh_stdout(["add", "m", "file1.txt#L1-L5", "--at", &head])?;
    Ok(())
}

#[test]
#[ignore]
fn cli_add_rejects_bad_address() -> Result<()> {
    let repo = TestRepo::seeded()?;
    let out = repo.run_mesh(["add", "m", "oops-no-fragment"])?;
    assert!(!out.status.success());
    Ok(())
}

#[test]
#[ignore]
fn cli_rm_stages_remove() -> Result<()> {
    let repo = TestRepo::seeded()?;
    repo.mesh_stdout(["add", "m", "file1.txt#L1-L5"])?;
    repo.mesh_stdout(["message", "m", "-m", "seed"])?;
    repo.mesh_stdout(["commit", "m"])?;
    repo.mesh_stdout(["rm", "m", "file1.txt#L1-L5"])?;
    let status = repo.mesh_stdout(["status", "m"])?;
    assert!(status.contains("remove") || status.contains("rm"));
    Ok(())
}

#[test]
#[ignore]
fn cli_message_inline() -> Result<()> {
    let repo = TestRepo::seeded()?;
    repo.mesh_stdout(["message", "m", "-m", "Hello"])?;
    let status = repo.mesh_stdout(["status", "m"])?;
    assert!(status.contains("Hello"));
    Ok(())
}

#[test]
#[ignore]
fn cli_message_from_file() -> Result<()> {
    let repo = TestRepo::seeded()?;
    repo.write_file("msg.txt", "Subject\n\nBody\n")?;
    repo.mesh_stdout(["message", "m", "-F", "msg.txt"])?;
    Ok(())
}

#[test]
#[ignore]
fn cli_commit_writes_ref() -> Result<()> {
    let repo = TestRepo::seeded()?;
    repo.mesh_stdout(["add", "m", "file1.txt#L1-L5"])?;
    repo.mesh_stdout(["message", "m", "-m", "Initial"])?;
    repo.mesh_stdout(["commit", "m"])?;
    assert!(repo.ref_exists("refs/meshes/v1/m"));
    Ok(())
}

#[test]
#[ignore]
fn cli_commit_empty_is_error() -> Result<()> {
    let repo = TestRepo::seeded()?;
    let out = repo.run_mesh(["commit", "empty"])?;
    assert!(!out.status.success());
    Ok(())
}

#[test]
#[ignore]
fn cli_status_check_exit_code_clean() -> Result<()> {
    let repo = TestRepo::seeded()?;
    let out = repo.run_mesh(["status", "--check"])?;
    assert_eq!(out.status.code(), Some(0));
    Ok(())
}

#[test]
#[ignore]
fn cli_status_check_exit_code_drifty() -> Result<()> {
    let repo = TestRepo::seeded()?;
    repo.mesh_stdout(["add", "m", "file1.txt#L1-L5"])?;
    // drift the worktree
    repo.write_file(
        "file1.txt",
        "DRIFT\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n",
    )?;
    let out = repo.run_mesh(["status", "--check"])?;
    assert_ne!(out.status.code(), Some(0));
    Ok(())
}

#[test]
#[ignore]
fn cli_config_read_lists_defaults() -> Result<()> {
    let repo = TestRepo::seeded()?;
    repo.mesh_stdout(["add", "m", "file1.txt#L1-L5"])?;
    repo.mesh_stdout(["message", "m", "-m", "seed"])?;
    repo.mesh_stdout(["commit", "m"])?;
    let out = repo.mesh_stdout(["config", "m"])?;
    assert!(out.contains("copy-detection"));
    assert!(out.contains("ignore-whitespace"));
    Ok(())
}

#[test]
#[ignore]
fn cli_config_stage_override_shows_starred_line() -> Result<()> {
    let repo = TestRepo::seeded()?;
    repo.mesh_stdout(["add", "m", "file1.txt#L1-L5"])?;
    repo.mesh_stdout(["message", "m", "-m", "seed"])?;
    repo.mesh_stdout(["commit", "m"])?;
    repo.mesh_stdout(["config", "m", "copy-detection", "off"])?;
    let out = repo.mesh_stdout(["config", "m"])?;
    assert!(out.contains("* copy-detection"));
    assert!(out.contains("(staged)"));
    Ok(())
}

#[test]
#[ignore]
fn cli_config_unknown_key_errors() -> Result<()> {
    let repo = TestRepo::seeded()?;
    repo.mesh_stdout(["add", "m", "file1.txt#L1-L5"])?;
    repo.mesh_stdout(["message", "m", "-m", "seed"])?;
    repo.mesh_stdout(["commit", "m"])?;
    let out = repo.run_mesh(["config", "m", "no-such-key"])?;
    assert!(!out.status.success());
    Ok(())
}

#[test]
#[ignore]
fn cli_commit_reserved_name_rejected() -> Result<()> {
    let repo = TestRepo::seeded()?;
    // `stale` is on the reserved list — clap may treat it as a subcommand.
    let out = repo.run_mesh(["add", "stale", "file1.txt#L1-L5"])?;
    assert!(!out.status.success());
    Ok(())
}
