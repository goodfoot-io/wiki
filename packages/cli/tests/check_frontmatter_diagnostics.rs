//! Reproduction tests for silent frontmatter diagnostic drops.
//!
//! `wiki check` must emit frontmatter diagnostics for invalid pages in every
//! discovery path: the default workspace walk and glob-selected files. Currently
//! both paths silently exclude pages with frontmatter errors from the diagnostic
//! loop — `is_wiki_member` pre-filters them during discovery and the diagnostic
//! loop only iterates `index_files`.

use std::fs;
use std::process::Command;

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

    fn run_check(&self, extra: &[&str]) -> std::process::Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_wiki"));
        cmd.current_dir(self.dir.path())
            .env("WIKI_BACKGROUND_FTS", "0");
        cmd.args(["check", "--format", "json"]);
        cmd.args(extra);
        cmd.output().expect("run wiki check")
    }

    fn parse_errors(output: &std::process::Output) -> Vec<serde_json::Value> {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let v: serde_json::Value = serde_json::from_str(&stdout)
            .unwrap_or_else(|e| panic!("parse json: {e}; stdout: {stdout}"));
        v.get("errors")
            .and_then(|x| x.as_array())
            .cloned()
            .unwrap_or_default()
    }
}

fn page_with_title_summary(title: &str, summary: &str) -> String {
    format!("---\ntitle: {title}\nsummary: {summary}\n---\n\nBody text.\n")
}

/// A page with `tags: not-an-array` (invalid — tags must be an array of strings)
/// but a valid title and summary.
fn page_with_invalid_tags() -> String {
    "---\ntitle: Broken Tags\nsummary: A page with invalid tags.\ntags: not-an-array\n---\n\nBody.\n"
        .to_string()
}

fn page_with_missing_summary() -> String {
    "---\ntitle: No Summary\n---\n\nBody.\n".to_string()
}

fn page_with_yaml_error() -> String {
    "---\ntitle: Unclosed\nsummary: Forgot the closing fence.\n".to_string()
}

// ── Walk-discovered pathway ──────────────────────────────────────────────────────

/// A walk-discovered page with `tags: not-an-array` must produce a frontmatter
/// diagnostic — it is currently silently excluded from the corpus.
#[test]
fn walk_discovered_invalid_tags_emits_diagnostic() {
    let repo = TestRepo::new();
    repo.create_file("wiki/broken.md", &page_with_invalid_tags());
    repo.commit("init");

    let out = repo.run_check(&[]);
    assert!(
        !out.status.success(),
        "walk-discovered page with invalid tags must exit non-zero"
    );

    let errors = TestRepo::parse_errors(&out);
    let frontmatter_errors: Vec<_> = errors
        .iter()
        .filter(|e| e["kind"].as_str() == Some("frontmatter"))
        .collect();
    assert!(
        !frontmatter_errors.is_empty(),
        "must have at least one frontmatter diagnostic, got: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let has_tags_error = frontmatter_errors
        .iter()
        .any(|e| e["message"].as_str().is_some_and(|m| m.contains("tags")));
    assert!(
        has_tags_error,
        "expected 'tags' diagnostic, got errors: {frontmatter_errors:?}"
    );
}

/// A walk-discovered page with a missing `summary` field must produce a
/// frontmatter diagnostic.
#[test]
fn walk_discovered_missing_summary_emits_diagnostic() {
    let repo = TestRepo::new();
    repo.create_file("wiki/no_summary.md", &page_with_missing_summary());
    repo.commit("init");

    let out = repo.run_check(&[]);
    assert!(
        !out.status.success(),
        "walk-discovered page with missing summary must exit non-zero"
    );

    let errors = TestRepo::parse_errors(&out);
    let frontmatter_errors: Vec<_> = errors
        .iter()
        .filter(|e| e["kind"].as_str() == Some("frontmatter"))
        .collect();
    assert!(
        !frontmatter_errors.is_empty(),
        "must have at least one frontmatter diagnostic, got: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// A walk-discovered page with an unclosed YAML frontmatter fence must produce
/// a frontmatter diagnostic.
#[test]
fn walk_discovered_yaml_parse_error_emits_diagnostic() {
    let repo = TestRepo::new();
    repo.create_file("wiki/unclosed.md", &page_with_yaml_error());
    repo.commit("init");

    let out = repo.run_check(&[]);
    assert!(
        !out.status.success(),
        "walk-discovered page with YAML parse error must exit non-zero"
    );

    let errors = TestRepo::parse_errors(&out);
    let frontmatter_errors: Vec<_> = errors
        .iter()
        .filter(|e| e["kind"].as_str() == Some("frontmatter"))
        .collect();
    assert!(
        !frontmatter_errors.is_empty(),
        "must have at least one frontmatter diagnostic, got: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// A glob-selected page with no frontmatter at all (`---` fence absent) must
/// produce a frontmatter diagnostic saying to add one. Files without a fence
/// are not auto-discovered by the default walk (plain .md files like README
/// are not wiki candidates), but explicit glob selection signifies user intent
/// to check this file.
#[test]
fn glob_selected_no_frontmatter_emits_diagnostic() {
    let repo = TestRepo::new();
    // Place outside the default scan root so it's only reachable via glob.
    repo.create_file("drafts/plain.md", "# Just a heading\n\nNo frontmatter here.\n");
    // Also create a valid wiki page so the walk doesn't produce "no wiki pages found".
    repo.create_file(
        "wiki/valid.md",
        &page_with_title_summary("Valid Page", "A valid wiki page for corpus membership."),
    );
    repo.commit("init");

    let out = repo.run_check(&["drafts/*.md"]);
    assert!(
        !out.status.success(),
        "glob-selected page with no frontmatter must exit non-zero, got: {}",
        String::from_utf8_lossy(&out.stdout)
    );

    let errors = TestRepo::parse_errors(&out);
    let frontmatter_errors: Vec<_> = errors
        .iter()
        .filter(|e| e["kind"].as_str() == Some("frontmatter"))
        .collect();
    assert!(
        !frontmatter_errors.is_empty(),
        "must have at least one frontmatter diagnostic, got: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// A file with no `---` frontmatter fence is a plain markdown file, not a wiki
/// candidate. The default walk must not flag it — it was never trying to be a
/// wiki page.
#[test]
fn walk_does_not_discover_plain_md_without_fence() {
    let repo = TestRepo::new();
    repo.create_file("wiki/plain.md", "# Just a heading\n\nNo frontmatter here.\n");
    // Add a valid wiki page so the check doesn't exit 2 with "no wiki pages found".
    repo.create_file(
        "wiki/valid.md",
        &page_with_title_summary("Valid Page", "A valid wiki page for corpus membership."),
    );
    repo.commit("init");

    let out = repo.run_check(&[]);
    assert!(
        out.status.success(),
        "plain .md files without --- fence must not produce diagnostics"
    );

    let errors = TestRepo::parse_errors(&out);
    let frontmatter_errors: Vec<_> = errors
        .iter()
        .filter(|e| e["kind"].as_str() == Some("frontmatter"))
        .collect();
    assert!(
        frontmatter_errors.is_empty(),
        "must have no frontmatter diagnostics for plain .md, got: {frontmatter_errors:?}"
    );
}

// ── Glob-selected pathway ────────────────────────────────────────────────────────

/// A glob-selected page with `tags: not-an-array` must produce a frontmatter
/// diagnostic. Glob-selected files bypass `is_wiki_member` during discovery
/// but are stored in `files`, not `index_files` — the diagnostic loop skips them.
#[test]
fn glob_selected_invalid_tags_emits_diagnostic() {
    let repo = TestRepo::new();
    // Place the file outside the default scan root so it is only reachable via glob.
    repo.create_file("drafts/broken.md", &page_with_invalid_tags());
    // Also create a valid wiki page so the walk doesn't produce "no wiki pages found".
    repo.create_file(
        "wiki/valid.md",
        &page_with_title_summary("Valid Page", "A valid wiki page for corpus membership."),
    );
    repo.commit("init");

    let out = repo.run_check(&["drafts/*.md"]);
    assert!(
        !out.status.success(),
        "glob-selected page with invalid tags must exit non-zero, got: {}",
        String::from_utf8_lossy(&out.stdout)
    );

    let errors = TestRepo::parse_errors(&out);
    let frontmatter_errors: Vec<_> = errors
        .iter()
        .filter(|e| e["kind"].as_str() == Some("frontmatter"))
        .collect();
    assert!(
        !frontmatter_errors.is_empty(),
        "must have at least one frontmatter diagnostic for glob-selected file, got: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let has_tags_error = frontmatter_errors
        .iter()
        .any(|e| e["message"].as_str().is_some_and(|m| m.contains("tags")));
    assert!(
        has_tags_error,
        "expected 'tags' diagnostic for glob-selected file, got errors: {frontmatter_errors:?}"
    );
}
