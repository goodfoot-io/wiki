//! CLI: `git mesh stale` — default human output (§10.4).

mod support;

use anyhow::Result;
use support::TestRepo;

fn seed(repo: &TestRepo, name: &str) -> Result<()> {
    repo.mesh_stdout(["add", name, "file1.txt#L1-L5"])?;
    repo.mesh_stdout(["message", name, "-m", "seed"])?;
    repo.mesh_stdout(["commit", name])?;
    Ok(())
}

fn drift(repo: &TestRepo, msg: &str) -> Result<String> {
    repo.write_file(
        "file1.txt",
        "lineONE\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n",
    )?;
    repo.commit_all(msg)
}

#[test]
#[ignore]
fn clean_exit_zero() -> Result<()> {
    let repo = TestRepo::seeded()?;
    seed(&repo, "m")?;
    let out = repo.run_mesh(["stale", "m"])?;
    assert_eq!(out.status.code(), Some(0));
    Ok(())
}

#[test]
#[ignore]
fn drifty_exit_one() -> Result<()> {
    let repo = TestRepo::seeded()?;
    seed(&repo, "m")?;
    drift(&repo, "mutate")?;
    let out = repo.run_mesh(["stale", "m"])?;
    assert_eq!(out.status.code(), Some(1));
    Ok(())
}

#[test]
#[ignore]
fn no_exit_code_forces_zero() -> Result<()> {
    let repo = TestRepo::seeded()?;
    seed(&repo, "m")?;
    drift(&repo, "mutate")?;
    let out = repo.run_mesh(["stale", "m", "--no-exit-code"])?;
    assert_eq!(out.status.code(), Some(0));
    Ok(())
}

#[test]
#[ignore]
fn human_output_has_summary_line() -> Result<()> {
    let repo = TestRepo::seeded()?;
    seed(&repo, "m")?;
    drift(&repo, "mutate")?;
    let out = repo.run_mesh(["stale", "m"])?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("stale of"), "summary 'N stale of M ranges'");
    Ok(())
}

#[test]
#[ignore]
fn human_output_groups_changed_ranges() -> Result<()> {
    let repo = TestRepo::seeded()?;
    seed(&repo, "m")?;
    drift(&repo, "mutate")?;
    let out = repo.run_mesh(["stale", "m"])?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Changed ranges"));
    Ok(())
}

#[test]
#[ignore]
fn oneline_suppresses_diffs() -> Result<()> {
    let repo = TestRepo::seeded()?;
    seed(&repo, "m")?;
    drift(&repo, "mutate")?;
    let out = repo.run_mesh(["stale", "m", "--oneline"])?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.contains("@@ "));
    Ok(())
}

#[test]
#[ignore]
fn stat_shows_counts() -> Result<()> {
    let repo = TestRepo::seeded()?;
    seed(&repo, "m")?;
    drift(&repo, "mutate")?;
    let out = repo.run_mesh(["stale", "m", "--stat"])?;
    assert_eq!(out.status.code(), Some(1));
    Ok(())
}

#[test]
#[ignore]
fn patch_includes_unified_diff() -> Result<()> {
    let repo = TestRepo::seeded()?;
    seed(&repo, "m")?;
    drift(&repo, "mutate")?;
    let out = repo.run_mesh(["stale", "m", "--patch"])?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("@@ "));
    Ok(())
}

#[test]
#[ignore]
fn workspace_scan_without_name() -> Result<()> {
    let repo = TestRepo::seeded()?;
    seed(&repo, "a")?;
    seed(&repo, "b")?;
    drift(&repo, "mutate")?;
    let out = repo.run_mesh(["stale"])?;
    assert_eq!(out.status.code(), Some(1));
    Ok(())
}
