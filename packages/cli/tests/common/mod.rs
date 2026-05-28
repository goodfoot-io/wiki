//! Shared fixture helpers for index integration tests.
//!
//! All helpers construct temporary git repositories using `std::process::Command::new("git")`
//! — the production-side ban does not apply to test fixture code.

use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

/// A temporary git repository with a configurable set of markdown files.
pub struct FixtureRepo {
    /// The `TempDir` that owns the directory; kept alive for the test's lifetime.
    pub dir: TempDir,
    /// Absolute path to the repo root (same as `dir.path()`).
    pub root: PathBuf,
}

impl FixtureRepo {
    /// Create a fresh, empty git repository in a temp directory.
    pub fn new() -> Self {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path().to_path_buf();

        git(&root, &["init", "-b", "main"]);
        git(&root, &["config", "user.email", "test@example.com"]);
        git(&root, &["config", "user.name", "Test"]);

        Self { dir, root }
    }

    /// Write a wiki-eligible markdown file (has both `title` and `summary`).
    pub fn write_wiki_md(&self, rel_path: &str, title: &str, summary: &str, body: &str) {
        self.write_file(
            rel_path,
            &format!(
                "---\ntitle: {title}\nsummary: {summary}\n---\n\n{body}\n",
                title = title,
                summary = summary,
                body = body,
            ),
        );
    }

    /// Write an arbitrary file with the given content.
    pub fn write_file(&self, rel_path: &str, content: &str) {
        let abs = self.root.join(rel_path);
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent).expect("create_dir_all");
        }
        std::fs::write(&abs, content).expect("write_file");
    }

    /// Stage a file with `git add`.
    pub fn git_add(&self, rel_path: &str) {
        git(&self.root, &["add", rel_path]);
    }

    /// Commit staged changes.
    pub fn git_commit(&self, msg: &str) {
        git(&self.root, &["commit", "-m", msg]);
    }

    /// Run `git mv` to rename a file.
    pub fn git_mv(&self, from: &str, to: &str) {
        git(&self.root, &["mv", from, to]);
    }

    /// Append a path to `.gitignore` (creating it if necessary) and stage it.
    pub fn ignore(&self, pattern: &str) {
        let gi = self.root.join(".gitignore");
        let existing = std::fs::read_to_string(&gi).unwrap_or_default();
        std::fs::write(&gi, format!("{existing}{pattern}\n")).expect("write .gitignore");
        git(&self.root, &["add", ".gitignore"]);
    }
}

/// Run a `git` command in `repo_root`, panicking on failure.
fn git(repo_root: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(repo_root)
        .args(args)
        .status()
        .expect("git command");
    assert!(status.success(), "git {:?} failed in {:?}", args, repo_root);
}

/// Build the "parity" fixture repo:
///   - one committed `.md` (wiki-eligible)
///   - one staged-only `.md` (wiki-eligible)
///   - one untracked `.md` (wiki-eligible)
///   - one gitignored `.md` (wiki-eligible)
///
/// Returns the `FixtureRepo`.
pub fn make_parity_fixture() -> FixtureRepo {
    let repo = FixtureRepo::new();

    // Committed file
    repo.write_wiki_md(
        "committed.md",
        "Committed Page",
        "A committed wiki page.",
        "Body text for committed.",
    );
    repo.git_add("committed.md");
    repo.git_commit("add committed.md");

    // Staged-only file (added but not committed)
    repo.write_wiki_md(
        "staged.md",
        "Staged Page",
        "A staged wiki page.",
        "Body text for staged.",
    );
    repo.git_add("staged.md");

    // Untracked file (not staged, not ignored)
    repo.write_wiki_md(
        "untracked.md",
        "Untracked Page",
        "An untracked wiki page.",
        "Body text for untracked.",
    );

    // Gitignored file
    repo.ignore("ignored.md");
    repo.write_wiki_md(
        "ignored.md",
        "Ignored Page",
        "A gitignored wiki page.",
        "Body text for ignored.",
    );
    repo.git_commit("add .gitignore");

    repo
}
