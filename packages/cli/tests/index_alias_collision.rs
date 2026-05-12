//! Reproduction tests for the wiki index alias-conflict bug.
//!
//! A title/alias collision is a per-key data quality issue confined to the
//! offending document. The index should always build to completion for the
//! largest consistent subset of `lookup_keys`: a conflicting alias is dropped
//! from the index and every other lookup in the namespace continues to work.
//!
//! Two distinct bugs are exercised:
//!
//! 1. A *self-collision* — an alias whose lowercased form equals its own
//!    document's title key — is reported as a conflict by the transactional
//!    insert at `index.rs:1040-1055`, taking the namespace offline.
//! 2. A *cross-document* alias/title collision aborts the entire
//!    `sync_core_index` build (at `index.rs:1052` and `index.rs:1547`)
//!    instead of dropping just the conflicting alias.
//!
//! Both manifest the same way at the surface: `wiki summary "<any other key
//! in the namespace>"` returns the alias-conflict error instead of finding
//! the requested page.

use std::fs;
use std::process::{Command, Output};

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
        self.git(&["commit", "--allow-empty", "-m", msg]);
    }

    fn git(&self, args: &[&str]) {
        let output = Command::new("git")
            .current_dir(self.dir.path())
            .args(args)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "t@t.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "t@t.com")
            .output()
            .expect("git");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn wiki(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_wiki"))
            .current_dir(self.dir.path().join("wiki"))
            .args(args)
            .env("WIKI_BACKGROUND_FTS", "0")
            .output()
            .expect("wiki")
    }
}

fn page_with(title: &str, aliases: &[&str], body: &str) -> String {
    let mut out = String::from("---\n");
    out.push_str(&format!("title: {title}\n"));
    if !aliases.is_empty() {
        out.push_str("aliases:\n");
        for a in aliases {
            out.push_str(&format!("  - {a}\n"));
        }
    }
    out.push_str(&format!("summary: {title} summary.\n"));
    out.push_str("---\n");
    out.push_str(body);
    out.push('\n');
    out
}

/// Reproduces failure mode #2: a self-collision (alias key equal to the
/// document's own title key) falsely reports as a conflict in the
/// transactional insert path, which then propagates out of
/// `sync_core_index` and breaks lookups for every other page in the
/// namespace.
#[test]
fn self_collision_does_not_break_namespace_lookups() {
    let repo = TestRepo::new();
    repo.create_file("wiki/wiki.toml", "");
    // The offending page: title and alias normalize to the same key.
    repo.create_file(
        "wiki/glossary/index.md",
        &page_with("Glossary", &["glossary"], "Glossary body."),
    );
    // An unrelated page that should remain reachable.
    repo.create_file(
        "wiki/glossary/card-repo.md",
        &page_with("Card Repo", &[], "Card repo body."),
    );
    repo.commit("init");

    let out = repo.wiki(&["summary", "Card Repo"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "expected `wiki summary \"Card Repo\"` to succeed despite the self-collision on `Glossary`/`glossary`; stdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("Card Repo") || stdout.contains("card-repo"),
        "expected summary output to reference the Card Repo page; got: {stdout}"
    );
}

/// Reproduces failure mode #1: a real cross-document alias collision
/// aborts the entire `sync_core_index` build, so every unrelated
/// `wiki summary` in the same namespace returns the conflict error.
/// The offending alias should be dropped from `lookup_keys` and reporting
/// deferred to `wiki check`; unrelated lookups must continue to resolve.
#[test]
fn cross_document_alias_collision_does_not_break_namespace_lookups() {
    let repo = TestRepo::new();
    repo.create_file("wiki/wiki.toml", "");
    // Two pages with a real cross-document collision: page B's alias
    // collides with page A's title.
    repo.create_file(
        "wiki/alpha.md",
        &page_with("Shared Name", &[], "Alpha body."),
    );
    repo.create_file(
        "wiki/beta.md",
        &page_with("Beta", &["shared name"], "Beta body."),
    );
    // An entirely unrelated page that must remain reachable.
    repo.create_file(
        "wiki/unrelated.md",
        &page_with("Unrelated Topic", &[], "Unrelated body."),
    );
    repo.commit("init");

    let out = repo.wiki(&["summary", "Unrelated Topic"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "expected `wiki summary \"Unrelated Topic\"` to succeed despite the alpha/beta alias collision; stdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("Unrelated Topic") || stdout.contains("unrelated"),
        "expected summary output to reference the Unrelated Topic page; got: {stdout}"
    );
}
