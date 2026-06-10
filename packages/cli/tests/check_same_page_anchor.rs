//! Reproduction and regression tests for same-page anchor link validation.
//!
//! A same-page anchor link `[text](#heading)` has an empty path (`""`) and
//! references a heading within the source file itself.  The `resolve_link_path`
//! function must return the source file (not its parent directory) when the
//! link path is empty, so the anchor can be validated against the file's own
//! headings.
//!
//! Without this fix, `""` resolves to the source file's directory, `read_via_source`
//! fails on a directory, and anchor validation is silently skipped.

use std::fs;
use std::process::{Command, Output};

use tempfile::TempDir;

// ── TestRepo ──────────────────────────────────────────────────────────────────

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

    fn create_file(&self, path: &str, content: &str) {
        let full = self.dir.path().join(path);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).expect("create_dir_all");
        }
        fs::write(full, content).expect("write file");
    }

    fn commit(&self, msg: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-m", msg]);
    }

    fn git(&self, args: &[&str]) {
        let output = Command::new("git")
            .current_dir(self.dir.path())
            .args(args)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .expect("spawn git");
        assert!(
            output.status.success(),
            "git {:?} failed:\n{}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// Run `wiki check --format json` from the wiki directory.
    fn run_check_json(&self) -> Output {
        let cwd = self.dir.path().join("wiki");
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_wiki"));
        cmd.current_dir(&cwd).env("WIKI_BACKGROUND_FTS", "0");
        cmd.args(["check", "--format", "json"]);
        cmd.output().expect("run wiki check")
    }
}

fn make_wiki_page(title: &str, body: &str) -> String {
    format!("---\ntitle: {title}\nsummary: A page about {title}.\n---\n{body}")
}

fn parse_check_errors(output: &Output) -> Vec<serde_json::Value> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| {
            panic!(
                "stdout is not JSON: {e}\nstdout: {stdout}\nstderr: {}",
                String::from_utf8_lossy(&output.stderr)
            )
        });
    parsed["errors"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "missing `errors` array; got: {stdout}\nstderr: {}",
                String::from_utf8_lossy(&output.stderr)
            )
        })
        .clone()
}

// ── Same-page anchor to nonexistent heading emits broken_anchor ──────────────

#[test]
fn same_page_anchor_to_nonexistent_heading_emits_broken_anchor() {
    let repo = TestRepo::new();
    repo.create_file(
        "wiki/alpha.md",
        &make_wiki_page("Alpha", "[bad](#nonexistent-heading)\n\n## Real Heading\n"),
    );
    repo.commit("init");

    let output = repo.run_check_json();
    let errors = parse_check_errors(&output);

    let broken_anchors: Vec<&serde_json::Value> = errors
        .iter()
        .filter(|e| e["kind"].as_str() == Some("broken_anchor"))
        .collect();
    assert_eq!(
        broken_anchors.len(),
        1,
        "expected exactly one broken_anchor diagnostic for `#nonexistent-heading`; \
         got: {broken_anchors:?}\nall errors: {errors:?}"
    );

    let diag = broken_anchors[0];
    let msg = diag["message"].as_str().unwrap_or("");
    assert!(
        msg.contains("nonexistent-heading"),
        "diagnostic message must mention the broken heading; got: {msg}"
    );
    assert!(
        diag["file"].as_str().unwrap_or("").ends_with("alpha.md"),
        "diagnostic file must be alpha.md; got: {}",
        diag["file"].as_str().unwrap_or("")
    );
}

// ── Same-page anchor to existing heading passes ──────────────────────────────

#[test]
fn same_page_anchor_to_existing_heading_passes() {
    let repo = TestRepo::new();
    repo.create_file(
        "wiki/beta.md",
        &make_wiki_page("Beta", "[good](#real-heading)\n\n## Real Heading\n"),
    );
    repo.commit("init");

    let output = repo.run_check_json();
    let errors = parse_check_errors(&output);

    let broken_anchors: Vec<&serde_json::Value> = errors
        .iter()
        .filter(|e| e["kind"].as_str() == Some("broken_anchor"))
        .collect();
    assert!(
        broken_anchors.is_empty(),
        "expected no broken_anchor for valid same-page heading; got: {broken_anchors:?}"
    );
}
