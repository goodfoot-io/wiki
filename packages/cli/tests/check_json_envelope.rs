//! Integration test for `wiki check --format json` envelope shape.
//!
//! Expected: an envelope object `{"errors": [...]}` so the shape is forward-
//! compatible (warnings, summary, etc.). Currently emits a bare array.

use std::fs;
use std::process::Command;

use tempfile::TempDir;

#[test]
fn check_json_output_is_envelope_object_with_errors_key() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();

    // Init a git repo so wiki has stable context.
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

    // Seed a clean wiki with one valid article.
    fs::create_dir_all(root.join("wiki")).unwrap();
    fs::write(root.join("wiki/wiki.toml"), "").unwrap();
    fs::write(
        root.join("wiki/page.md"),
        "---\ntitle: Page\nsummary: A page.\n---\nHello.\n",
    )
    .unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-m", "init"]);

    let out = Command::new(env!("CARGO_BIN_EXE_wiki"))
        .current_dir(root.join("wiki"))
        .env("WIKI_BACKGROUND_FTS", "0")
        .args(["check", "--format", "json"])
        .output()
        .expect("run wiki check");

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();

    let parsed: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout is not JSON: {e}\nstdout: {stdout}\nstderr: {stderr}"));

    assert!(
        parsed.is_object(),
        "expected JSON object envelope, got: {stdout}\nstderr: {stderr}"
    );
    let errors = parsed
        .get("errors")
        .unwrap_or_else(|| panic!("missing `errors` key; got: {stdout}"));
    assert!(
        errors.is_array(),
        "`errors` must be an array; got: {errors}"
    );
    assert_eq!(
        errors.as_array().unwrap().len(),
        0,
        "clean wiki must have empty errors array; got: {errors}"
    );
}
