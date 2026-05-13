//! Reproduction and regression tests for `wiki check` image reference handling.
//!
//! Markdown image references like `![screenshot](images/screenshot.png)` use
//! bare (not explicitly relative) paths.  The `resolve_link_path` function
//! resolves bare paths relative to the source page's directory (page-relative),
//! matching standard markdown behavior.
//!
//! Resolution rules:
//!   - Bare paths (`images/screenshot.png`) → page-relative
//!   - `./` and `../` paths → page-relative (unchanged)
//!   - `/`-prefixed paths (`/images/screenshot.png`) → repo-relative

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
    format!("---\ntitle: {title}\nsummary: A page about {title}.\n---\n{body}")
}

// ── Bare image path (page-relative resolution, found) ─────────────────────────

/// Bare paths (without `./` or `/` prefix) resolve relative to the page
/// directory.  When the image exists at the page-relative location, `wiki check`
/// must succeed.
#[test]
fn bare_image_path_found_at_page_relative_location() {
    let repo = TestRepo::new();
    // Page at wiki/design/pages/example.md references an image via bare path:
    //   `![screenshot](images/screenshot.png)`
    repo.create_file(
        "wiki/design/pages/example.md",
        &make_wiki_page("Example", "![screenshot](images/screenshot.png)"),
    );
    // The image exists at the page-relative path
    // (`wiki/design/pages/images/screenshot.png`), so the bare path resolves
    // correctly and no error is produced.
    repo.create_file("wiki/design/pages/images/screenshot.png", "");
    repo.commit("init");

    let out = repo.run_check_from("wiki");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        out.status.success(),
        "bare path must resolve page-relative and find the image.\n\
         stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        !stdout.contains("Missing File"),
        "bare path resolving page-relative must not produce 'Missing File'.\n\
         stdout:\n{stdout}"
    );
}

// ── Bare image path missing page-relative but present repo-relative ───────────

/// When a bare path doesn't exist at the page-relative location but does exist
/// at the repo-relative location, `wiki check` must report the error and
/// suggest using the `/` prefix for repo-relative paths.
#[test]
fn bare_image_path_suggests_slash_prefix_when_repo_relative_exists() {
    let repo = TestRepo::new();
    // Page at wiki/design/pages/example.md references an image via bare path:
    //   `![screenshot](images/screenshot.png)`
    repo.create_file(
        "wiki/design/pages/example.md",
        &make_wiki_page("Example", "![screenshot](images/screenshot.png)"),
    );
    // The image exists at the repo root level but NOT at the page-relative
    // location (`wiki/design/pages/images/screenshot.png`).
    repo.create_file("images/screenshot.png", "");
    repo.commit("init");

    let out = repo.run_check_from("wiki");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        !out.status.success(),
        "bare path with image only at repo root must produce an error.\n\
         stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("not found at page-relative path"),
        "error must mention page-relative resolution.\n\
         stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("use `/images/screenshot.png`"),
        "error must suggest the /-prefixed repo-relative path.\n\
         stdout:\n{stdout}"
    );
}

// ── `/`-prefixed image path (repo-relative resolution) ────────────────────────

/// A `/`-prefixed path resolves relative to the repository root.  When the
/// image exists at the repo root, `wiki check` must succeed.
#[test]
fn slash_prefixed_image_path_found_at_repo_relative_location() {
    let repo = TestRepo::new();
    // Page at wiki/design/pages/example.md references an image via `/`-prefixed
    // path: `![screenshot](/images/screenshot.png)`
    repo.create_file(
        "wiki/design/pages/example.md",
        &make_wiki_page("Example", "![screenshot](/images/screenshot.png)"),
    );
    // The image exists at the repo root level (`<repo>/images/screenshot.png`),
    // so the /-prefixed path resolves correctly and no error is produced.
    repo.create_file("images/screenshot.png", "");
    repo.commit("init");

    let out = repo.run_check_from("wiki");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        out.status.success(),
        "/-prefixed path must resolve repo-relative and find the image.\n\
         stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        !stdout.contains("Missing File"),
        "/-prefixed path must not produce 'Missing File'.\n\
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
    repo.create_file(
        "wiki/design/pages/example.md",
        &make_wiki_page("Example", "![screenshot](./images/screenshot.png)"),
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

/// An image reference (`![alt](nonexistent.png)`) inside a wiki member file
/// (identified by `title:` + `summary:` frontmatter) must produce a
/// `broken_link` diagnostic exactly once.
#[test]
fn image_reference_to_missing_file_emits_broken_link() {
    let repo = TestRepo::new();
    repo.create_file("wiki/index.md", &make_wiki_page("Default Index", "Hi."));
    repo.create_file(
        "floats/illustrated.md",
        "---\ntitle: Illustrated\nsummary: S.\n---\n\n![alt](nonexistent.png)\n",
    );
    repo.commit("init");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_wiki"));
    cmd.current_dir(repo.dir.path().join("wiki"))
        .env("WIKI_BACKGROUND_FTS", "0")
        .args(["check", "--no-mesh", "--format", "json"]);
    let out = cmd.output().expect("run wiki check");
    let stdout = String::from_utf8_lossy(&out.stdout);

    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("parse json: {e}; stdout: {stdout}"));
    let errs = v
        .get("errors")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default();
    let broken_links: Vec<&serde_json::Value> = errs
        .iter()
        .filter(|e| e["kind"].as_str() == Some("broken_link"))
        .collect();
    assert_eq!(
        broken_links.len(),
        1,
        "expected exactly one broken_link diagnostic for the float's image \
         reference; got: {broken_links:?}"
    );
}
