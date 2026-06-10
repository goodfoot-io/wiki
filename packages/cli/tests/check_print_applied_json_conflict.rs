//! Reproduction test: `--print-applied` and `--format json` must conflict.
//!
//! When both flags are set, mesh paths and the JSON envelope are both written
//! to stdout, mixing two formats. The documented contract at
//! [check.rs L308-L313] says stdout is exactly one repo-relative path per line
//! under `--print-applied`, which is incompatible with `--format json`.
//!
//! Adding `conflicts_with = "format"` to the `--print-applied` argument makes
//! clap reject the combination before the command runs.

use std::fs;
use std::process::Command;

use tempfile::TempDir;

#[test]
fn print_applied_and_format_json_must_conflict() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();

    let git = |args: &[&str]| {
        let out = Command::new("git")
            .current_dir(root)
            .args(args)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .expect("git");
        assert!(
            out.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init"]);
    git(&["checkout", "-b", "main"]);

    // Seed a clean wiki with one valid article so `--fix` has something to run on.
    fs::create_dir_all(root.join("wiki")).unwrap();
    fs::write(
        root.join("wiki/page.md"),
        "---\ntitle: Page\nsummary: A page.\n---\nHello.\n",
    )
    .unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-m", "init"]);

    // Run with conflicting flags. The combination should be rejected by clap.
    let out = Command::new(env!("CARGO_BIN_EXE_wiki"))
        .current_dir(root.join("wiki"))
        .env("WIKI_BACKGROUND_FTS", "0")
        .args(["check", "--fix", "--print-applied", "--format", "json"])
        .output()
        .expect("run wiki check");

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();

    assert!(
        !out.status.success(),
        "--print-applied and --format json must conflict, but command succeeded\n\
         stdout: {stdout}\nstderr: {stderr}"
    );
}
