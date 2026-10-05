//! Shared fixture helpers for index integration tests.
//!
//! All helpers construct temporary git repositories using `std::process::Command::new("git")`
//! — the production-side ban does not apply to test fixture code.
//!
//! This module is compiled into each integration-test binary that does `mod common;`.
#![allow(
    dead_code,
    reason = "each integration-test binary uses only a subset of these shared fixture \
              helpers, so items unused by one binary would trip dead_code there; \
              `FixtureRepo::dir` is an RAII guard kept alive and never read"
)]

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

    /// Run `git rm` to delete a file from the index and the worktree.
    pub fn git_rm(&self, rel_path: &str) {
        git(&self.root, &["rm", "-q", rel_path]);
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

/// Resolve the repository's common git dir via
/// `git rev-parse --git-common-dir`, absolutized against `repo_root`.
///
/// Every linked worktree of one repository shares this directory; the
/// merged store lives at `<common>/wiki/store.sqlite`. Target-layout port
/// helper for the spec suites relocated by the merged-store card.
pub fn git_common_dir(repo_root: &Path) -> PathBuf {
    let out = Command::new("git")
        .current_dir(repo_root)
        .args(["rev-parse", "--git-common-dir"])
        .output()
        .expect("spawn git rev-parse");
    assert!(
        out.status.success(),
        "git rev-parse --git-common-dir failed in {:?}: {}",
        repo_root,
        String::from_utf8_lossy(&out.stderr)
    );
    let resolved = String::from_utf8(out.stdout)
        .expect("utf8 output")
        .trim()
        .to_string();
    let path = PathBuf::from(&resolved);
    if path.is_absolute() {
        path
    } else {
        repo_root.join(path)
    }
}

/// The merged store's database file for `repo_root`:
/// `<git-common-dir>/wiki/store.sqlite`.
pub fn target_db_path(repo_root: &Path) -> PathBuf {
    git_common_dir(repo_root).join("wiki").join("store.sqlite")
}

/// The newest generation's id in the merged store — the generation a
/// `WikiIndex` that just refreshed serves.
fn newest_gen_id(conn: &rusqlite::Connection) -> i64 {
    conn.query_row(
        "SELECT gen_id FROM generations ORDER BY created_at DESC, gen_id DESC LIMIT 1",
        [],
        |r| r.get(0),
    )
    .expect("the store holds a generation")
}

/// The newest generation's `(path_rel, source literal, title)` rows,
/// ordered by path then source literal.
pub fn served_path_rows(repo_root: &Path) -> Vec<(String, String, String)> {
    let conn = rusqlite::Connection::open(target_db_path(repo_root)).expect("open merged store");
    let gen_id = newest_gen_id(&conn);
    let mut stmt = conn
        .prepare(
            "SELECT p.path_rel, p.source, b.title
             FROM gen_paths p JOIN blobs b ON b.oid = p.oid
             WHERE p.gen_id = ?1
             ORDER BY p.path_rel ASC, p.source ASC",
        )
        .expect("prepare gen_paths dump");
    let rows = stmt
        .query_map([gen_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .expect("gen_paths dump");
    rows.map(|r| r.expect("row")).collect()
}

/// `(global blobs rows, newest generation's gen_paths rows)` for `oid`.
pub fn served_blob_path_counts(repo_root: &Path, oid: &str) -> (usize, usize) {
    let conn = rusqlite::Connection::open(target_db_path(repo_root)).expect("open merged store");
    let gen_id = newest_gen_id(&conn);
    let blobs: i64 = conn
        .query_row("SELECT COUNT(*) FROM blobs WHERE oid = ?1", [oid], |r| r.get(0))
        .expect("count blobs");
    let paths: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM gen_paths WHERE oid = ?1 AND gen_id = ?2",
            rusqlite::params![oid, gen_id],
            |r| r.get(0),
        )
        .expect("count gen_paths");
    (blobs as usize, paths as usize)
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
