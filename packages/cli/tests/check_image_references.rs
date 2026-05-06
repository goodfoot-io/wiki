//! Reproduction and regression tests for `wiki check` image reference handling.
//!
//! Markdown image references like `![screenshot](images/screenshot.png)` use
//! bare (not explicitly relative) paths.  The `resolve_link_path` function
//! treats bare paths as repo-relative instead of page-relative, so the image
//! file is reported as missing even when it exists at the expected
//! page-relative path.
//!
//! Bug trace:
//!   1. `parse_fragment_links` in `parser.rs` extracts `images/screenshot.png`
//!      from `![screenshot](images/screenshot.png)`.
//!   2. `resolve_link_path` in `commands/mod.rs` checks whether the path
//!      starts with `.` or `..`.  `images/` does not, so it is treated as
//!      repo-relative and returned unchanged.
//!   3. The caller in `collect_for_files` (check.rs:467) prepends
//!      `repo_root`, yielding `<repo_root>/images/screenshot.png`.
//!   4. The file actually lives at `<repo_root>/wiki/design/pages/images/screenshot.png`.
//!   5. `read_via_source` fails → `missing_file` diagnostic.
//!
//! With `./images/screenshot.png` the path starts with `.`, is resolved from
//! the page directory, and is found correctly.

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

    /// Run `wiki check` from `cwd_rel` (relative to repo root).
    fn run_check_from(&self, cwd_rel: &str) -> Output {
        let cwd = if cwd_rel.is_empty() {
            self.dir.path().to_path_buf()
        } else {
            self.dir.path().join(cwd_rel)
        };
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_wiki"));
        cmd.current_dir(&cwd).env("WIKI_BACKGROUND_FTS", "0");
        cmd.args(["check"]);
        cmd.output().expect("run wiki check")
    }
}

fn make_wiki_page(title: &str, body: &str) -> String {
    format!(
        "---\ntitle: {title}\nsummary: A page about {title}.\n---\n{body}"
    )
}

// ── Bare image path (repo-relative guidance) ────────────────────────────────

/// Bare paths (without `./` prefix) are repo-relative by convention.  When
/// a bare path does not exist at the repo root, `wiki check` must report the
/// error with guidance suggesting the correct repo-relative path.
#[test]
fn bare_image_path_reports_error_with_repo_relative_guidance() {
    let repo = TestRepo::new();
    // Page at wiki/design/pages/example.md references an image via bare path:
    //   `![screenshot](images/screenshot.png)`
    repo.create_file("wiki/wiki.toml", "");
    repo.create_file(
        "wiki/design/pages/example.md",
        &make_wiki_page("Example", "![screenshot](images/screenshot.png)"),
    );
    // The image exists at the page-relative path, but the bare path
    // `images/screenshot.png` resolves to `<repo_root>/images/screenshot.png`,
    // which does not exist.
    repo.create_file("wiki/design/pages/images/screenshot.png", "");
    repo.commit("init");

    let out = repo.run_check_from("wiki");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        !out.status.success(),
        "bare path must produce an error because it resolves from repo root.\n\
         stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("Paths without a `./` prefix"),
        "error must explain the repo-relative resolution rule.\n\
         stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("wiki/design/pages/images/screenshot.png"),
        "error must suggest the correct repo-relative path.\n\
         stdout:\n{stdout}"
    );
}

// ── Explicitly relative image path (works correctly) ─────────────────────────

/// An explicitly relative image path `./images/screenshot.png` starts with `.`,
/// so `resolve_link_path` resolves it from the page directory and the image is
/// found correctly.  This test passes both before and after the fix.
#[test]
fn explicit_relative_image_path_does_not_produce_missing_file() {
    let repo = TestRepo::new();
    repo.create_file("wiki/wiki.toml", "");
    repo.create_file(
        "wiki/design/pages/example.md",
        &make_wiki_page(
            "Example",
            "![screenshot](./images/screenshot.png)",
        ),
    );
    repo.create_file("wiki/design/pages/images/screenshot.png", "");
    repo.commit("init");

    let out = repo.run_check_from("wiki");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        out.status.success(),
        "explicitly relative image path must resolve correctly.\n\
         stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        !stdout.contains("Missing File"),
        "explicitly relative image path must not produce 'Missing File'.\n\
         stdout:\n{stdout}"
    );
}
