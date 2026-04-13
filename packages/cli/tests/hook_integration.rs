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

    fn create_file(&self, path: &str, content: &str) {
        let full = self.dir.path().join(path);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).expect("create_dir_all");
        }
        fs::write(full, content).expect("write file");
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

#[test]
fn hook_command_round_trips_via_binary() {
    let repo = TestRepo::new();
    repo.create_file(
        "wiki/some-page.md",
        "---\ntitle: Some Page\nsummary: The summary for Some Page.\n---\nBody text.\n",
    );
    // Commit so git operations inside WikiIndex::prepare work.
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-m", "init"]);

    let binary = env!("CARGO_BIN_EXE_wiki");
    let mut child = Command::new(binary)
        .arg("hook")
        .arg("--claude")
        .current_dir(repo.path())
        .env("WIKI_DIR", "wiki")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn wiki binary");

    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"see [[Some Page]] for details\n")
        .expect("write stdin");

    let output = child.wait_with_output().expect("wait");
    assert!(
        output.status.success(),
        "exit {:?}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("utf8");
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).expect("parse json");
    assert_eq!(
        parsed["hookSpecificOutput"]["hookEventName"],
        "PostToolUse"
    );
    let ctx = parsed["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .expect("additionalContext is string");
    assert!(
        ctx.contains("Some Page"),
        "additionalContext must contain page title, got: {ctx}"
    );
    assert!(
        ctx.contains("The summary for Some Page"),
        "additionalContext must contain summary, got: {ctx}"
    );
}
