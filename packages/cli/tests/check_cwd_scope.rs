//! Regression tests for the CWD-scoped selection / repo-root resolution split.
//!
//! `wiki check` selects the `*.md` pages to validate relative to the current
//! working directory, while links, anchors, and `/`-absolute paths resolve
//! against the git repository root. Two consequences are pinned here:
//!
//!   1. Running from a subdirectory validates only that subtree, and produces
//!      byte-for-byte the same diagnostics as the equivalent repo-relative glob
//!      run from the repo root.
//!   2. A page whose links reach outside the working directory (but stay inside
//!      the repo) still resolves them — resolution never narrows to the CWD.

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

    /// Run `wiki check --format json` from `cwd_rel` (relative to the repo
    /// root; empty string = repo root) with the given extra args. The fixtures
    /// use only markdown-to-markdown links with no line ranges, so the always-on
    /// mesh-coverage pass contributes no diagnostics here.
    fn check_json(&self, cwd_rel: &str, extra: &[&str]) -> Output {
        let cwd = if cwd_rel.is_empty() {
            self.dir.path().to_path_buf()
        } else {
            self.dir.path().join(cwd_rel)
        };
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_wiki"));
        cmd.current_dir(&cwd).env("WIKI_BACKGROUND_FTS", "0");
        cmd.args(["check", "--format", "json"]);
        cmd.args(extra);
        cmd.output().expect("run wiki check")
    }
}

fn wiki_page(title: &str, body: &str) -> String {
    format!("---\ntitle: {title}\nsummary: A page about {title}.\n---\n{body}\n")
}

fn broken_link_files(out: &Output) -> Vec<String> {
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("parse json: {e}; stdout: {stdout}"));
    v.get("errors")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|e| e["kind"].as_str() == Some("broken_link"))
        .map(|e| e["file"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Running from a subdirectory validates only the pages beneath it: a broken
/// link in a sibling directory is not reported.
#[test]
fn subdirectory_check_scopes_selection_to_the_cwd() {
    let repo = TestRepo::new();
    repo.create_file(
        "wiki/page.md",
        &wiki_page("Wiki Page", "See [gone](./missing-wiki.md)."),
    );
    repo.create_file(
        "docs/page.md",
        &wiki_page("Docs Page", "See [gone](./missing-docs.md)."),
    );
    repo.commit("init");

    let from_wiki = broken_link_files(&repo.check_json("wiki", &[]));
    assert!(
        from_wiki.iter().any(|f| f.ends_with("wiki/page.md")),
        "wiki/page.md broken link must be reported from wiki/: {from_wiki:?}"
    );
    assert!(
        !from_wiki.iter().any(|f| f.contains("docs/")),
        "docs/ pages must be out of scope when checking from wiki/: {from_wiki:?}"
    );
}

/// A bare check from a subdirectory yields byte-for-byte the same diagnostics
/// as the equivalent repo-relative glob run from the repo root.
#[test]
fn subtree_check_equals_repo_root_glob() {
    let repo = TestRepo::new();
    repo.create_file(
        "wiki/a.md",
        &wiki_page("A", "Link to [missing](./nope-a.md)."),
    );
    repo.create_file(
        "wiki/nested/b.md",
        &wiki_page("B", "Link to [missing](./nope-b.md)."),
    );
    // A page outside the subtree, present so the two runs would diverge if
    // selection were not actually scoped.
    repo.create_file(
        "docs/c.md",
        &wiki_page("C", "Link to [missing](./nope-c.md)."),
    );
    repo.commit("init");

    let from_subdir = repo.check_json("wiki", &[]);
    let from_root_glob = repo.check_json("", &["wiki/**/*.md"]);

    assert_eq!(
        from_subdir.stdout,
        from_root_glob.stdout,
        "subtree check and repo-root glob must produce identical diagnostics.\n\
         from wiki/:\n{}\nfrom root (glob):\n{}",
        String::from_utf8_lossy(&from_subdir.stdout),
        String::from_utf8_lossy(&from_root_glob.stdout),
    );
}

/// Links that reach outside the working directory but stay inside the repo
/// resolve correctly: resolution is anchored at the repo root, not the CWD.
#[test]
fn cross_tree_links_resolve_from_repo_root() {
    let repo = TestRepo::new();
    // The link target lives outside the directory we check from.
    repo.create_file("shared/ref.md", &wiki_page("Shared Ref", "Body."));
    repo.create_file(
        "wiki/page.md",
        &wiki_page(
            "Wiki Page",
            // `/`-absolute resolves against the repo root; `../` is page-relative.
            "Repo-absolute [ref](/shared/ref.md) and relative [ref2](../shared/ref.md).",
        ),
    );
    repo.commit("init");

    let from_wiki = broken_link_files(&repo.check_json("wiki", &[]));
    assert!(
        from_wiki.is_empty(),
        "cross-tree links must resolve against the repo root, not the CWD: {from_wiki:?}"
    );
}
