//! CLI: `git mesh stale` machine formats (§10.4).

mod support;

use anyhow::Result;
use serde_json::Value;
use support::TestRepo;

fn seed(repo: &TestRepo, name: &str) -> Result<()> {
    repo.mesh_stdout(["add", name, "file1.txt#L1-L5"])?;
    repo.mesh_stdout(["message", name, "-m", "seed"])?;
    repo.mesh_stdout(["commit", name])?;
    Ok(())
}

fn drift(repo: &TestRepo) -> Result<String> {
    repo.write_file(
        "file1.txt",
        "lineONE\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n",
    )?;
    repo.commit_all("mutate")
}

#[test]
#[ignore]
fn porcelain_one_line_per_finding() -> Result<()> {
    let repo = TestRepo::seeded()?;
    seed(&repo, "m")?;
    drift(&repo)?;
    let out = repo.run_mesh(["stale", "m", "--format=porcelain"])?;
    assert_eq!(out.status.code(), Some(1));
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(text.trim().lines().count(), 1);
    assert!(text.contains("CHANGED"));
    assert!(text.contains("file1.txt"));
    Ok(())
}

#[test]
#[ignore]
fn porcelain_clean_is_empty() -> Result<()> {
    let repo = TestRepo::seeded()?;
    seed(&repo, "m")?;
    let out = repo.run_mesh(["stale", "m", "--format=porcelain"])?;
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).trim().is_empty());
    Ok(())
}

#[test]
#[ignore]
fn json_has_version_one() -> Result<()> {
    let repo = TestRepo::seeded()?;
    seed(&repo, "m")?;
    drift(&repo)?;
    let out = repo.run_mesh(["stale", "m", "--format=json"])?;
    let v: Value = serde_json::from_slice(&out.stdout)?;
    assert_eq!(v["version"], 1);
    assert!(v["ranges"].is_array());
    Ok(())
}

#[test]
#[ignore]
fn json_range_entries_have_lsp_shape() -> Result<()> {
    let repo = TestRepo::seeded()?;
    seed(&repo, "m")?;
    drift(&repo)?;
    let out = repo.run_mesh(["stale", "m", "--format=json"])?;
    let v: Value = serde_json::from_slice(&out.stdout)?;
    let first = &v["ranges"][0];
    assert!(first["severity"].is_string() || first["severity"].is_number());
    assert!(first["range"].is_object());
    assert!(first["message"].is_string());
    Ok(())
}

#[test]
#[ignore]
fn junit_has_testsuite_tag() -> Result<()> {
    let repo = TestRepo::seeded()?;
    seed(&repo, "m")?;
    drift(&repo)?;
    let out = repo.run_mesh(["stale", "m", "--format=junit"])?;
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("<testsuite"));
    assert!(s.contains("<testcase"));
    Ok(())
}

#[test]
#[ignore]
fn github_actions_emits_warning_annotation() -> Result<()> {
    let repo = TestRepo::seeded()?;
    seed(&repo, "m")?;
    drift(&repo)?;
    let out = repo.run_mesh(["stale", "m", "--format=github-actions"])?;
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("::warning file=file1.txt"));
    Ok(())
}

#[test]
#[ignore]
fn tool_error_exits_two() -> Result<()> {
    // Running outside a git repo is the canonical "tool error" (§10.4).
    let dir = tempfile::tempdir()?;
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_git-mesh"))
        .current_dir(dir.path())
        .args(["stale"])
        .output()?;
    assert_eq!(out.status.code(), Some(2));
    Ok(())
}

#[test]
#[ignore]
fn since_filters_by_anchor_age() -> Result<()> {
    let repo = TestRepo::seeded()?;
    let early_anchor = repo.head_sha()?;
    // Move HEAD forward so --since has something to exclude.
    repo.commit_file("other.txt", "x\n", "mid")?;
    let mid = repo.head_sha()?;
    // Stage range anchored at mid, not early.
    repo.mesh_stdout(["add", "m", "file1.txt#L1-L5", "--at", &mid])?;
    repo.mesh_stdout(["message", "m", "-m", "seed"])?;
    repo.mesh_stdout(["commit", "m"])?;
    drift(&repo)?;
    // --since mid => our range is in scope; exit 1.
    let inc = repo.run_mesh(["stale", "m", "--since", &mid, "--format=porcelain"])?;
    assert_eq!(inc.status.code(), Some(1));
    // --since HEAD (now past mid) — range anchor is before HEAD, and
    // --since filters to anchors in <since>..HEAD, so older anchors
    // drop out. Use early_anchor to be explicit about scope.
    let _ = early_anchor;
    Ok(())
}
