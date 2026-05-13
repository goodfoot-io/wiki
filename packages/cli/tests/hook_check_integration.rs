//! Regression tests for `wiki hook` (PostToolUse:Edit hook).
//!
//! The hook must silently return exit 0 when given a `.md` file that is
//! not a wiki member (i.e., does not have both `title:` and `summary:` in its
//! YAML frontmatter). Only wiki member files should be validated.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use tempfile::TempDir;

struct TestRepo {
    dir: TempDir,
}

impl TestRepo {
    fn new() -> Self {
        let dir = TempDir::new().expect("tempdir");
        let repo = Self { dir };
        repo.git(&["init"]);
        repo.git(&["checkout", "-b", "main"]);
        repo
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn git(&self, args: &[&str]) {
        let output = Command::new("git")
            .current_dir(self.dir.path())
            .args(args)
            .env("GIT_AUTHOR_NAME", "Test Author")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test Committer")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .expect("spawn git");
        assert!(
            output.status.success(),
            "git {:?} failed:\n{}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// A `.md` file without complete frontmatter (missing `title:` and/or
/// `summary:`) must be silently skipped: `wiki hook` should produce no
/// output and exit 0.
///
/// This test FAILS before the bug is fixed because `hook_check::run()` passes
/// the file path as an explicit glob to `check::collect`, bypassing the
/// wiki-member filter. As a result the non-member file is validated and a
/// `systemMessage` is printed to stdout.
#[test]
fn hook_check_skips_non_wiki_md_file() {
    let repo = TestRepo::new();

    // Create a plain README that is NOT a wiki member — no frontmatter.
    let readme_path = repo.path().join("src/README.md");
    fs::create_dir_all(readme_path.parent().unwrap()).expect("mkdir src/");
    fs::write(
        &readme_path,
        "# README\n\nThis is not a wiki page — no frontmatter.\n",
    )
    .expect("write README");

    // The wiki root is `wiki/`; the README at src/README.md has no frontmatter
    // and must be skipped by the hook.
    fs::create_dir_all(repo.path().join("wiki")).expect("mkdir wiki/");

    // PostToolUse JSON payload that Claude Code sends to the hook.
    let payload = serde_json::json!({
        "tool_input": {
            "file_path": readme_path.to_str().expect("path utf8")
        }
    });

    let binary = env!("CARGO_BIN_EXE_wiki");
    let mut child = Command::new(binary)
        .args(["--root", "wiki", "hook"])
        .current_dir(repo.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn wiki binary");

    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.to_string().as_bytes())
        .expect("write stdin");

    let output = child.wait_with_output().expect("wait");

    assert!(
        output.status.success(),
        "exit {:?}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");

    // The hook must produce no output when given a non-wiki-member .md file.
    assert!(
        stdout.trim().is_empty(),
        "hook must produce no output for a non-wiki-member .md file, got: {stdout}"
    );
}
