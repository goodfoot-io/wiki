//! Regression test: `wiki list` must return wiki pages from all four file
//! classes — committed, staged (index-only), untracked, and gitignored.
//!
//! Before the fix the WalkBuilder in `commands/list.rs` respected
//! `.gitignore`, so gitignored wiki pages were silently omitted.

use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

struct TestRepo {
    dir: TempDir,
}

impl TestRepo {
    fn new() -> Self {
        let dir = TempDir::new().expect("tempdir");
        let repo = Self { dir };
        repo.git(&["init", "-b", "main"]);
        repo.git(&["config", "user.email", "test@example.com"]);
        repo.git(&["config", "user.name", "Test"]);
        repo
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn write_file(&self, rel: &str, content: &str) {
        let abs = self.dir.path().join(rel);
        if let Some(parent) = abs.parent() {
            fs::create_dir_all(parent).expect("create_dir_all");
        }
        fs::write(abs, content).expect("write file");
    }

    fn git(&self, args: &[&str]) {
        let out = Command::new("git")
            .current_dir(self.dir.path())
            .args(args)
            .output()
            .expect("spawn git");
        assert!(
            out.status.success(),
            "git {:?} failed:\n{}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

/// Build the four-class fixture and assert `wiki list` contains all four titles.
#[test]
fn list_returns_all_four_file_classes() {
    let wiki_bin = env!("CARGO_BIN_EXE_wiki");

    let repo = TestRepo::new();

    // 1. Committed wiki page.
    repo.write_file(
        "committed.md",
        "---\ntitle: Committed\nsummary: committed page.\n---\nbody\n",
    );
    repo.git(&["add", "committed.md"]);
    repo.git(&["commit", "-m", "add committed"]);

    // 2. Staged (index-only) wiki page.
    repo.write_file(
        "staged.md",
        "---\ntitle: Staged\nsummary: staged page.\n---\nbody\n",
    );
    repo.git(&["add", "staged.md"]);

    // 3. Untracked wiki page (on disk, not in index or HEAD, not ignored).
    repo.write_file(
        "untracked.md",
        "---\ntitle: Untracked\nsummary: untracked page.\n---\nbody\n",
    );

    // 4. Gitignored wiki page — ignored.md is excluded by .gitignore.
    repo.write_file(".gitignore", "ignored.md\n");
    repo.git(&["add", ".gitignore"]);
    repo.git(&["commit", "-m", "add gitignore"]);
    repo.write_file(
        "ignored.md",
        "---\ntitle: Ignored\nsummary: ignored page.\n---\nbody\n",
    );

    let output = Command::new(wiki_bin)
        .current_dir(repo.path())
        .arg("list")
        .output()
        .expect("spawn wiki list");

    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");

    assert!(
        output.status.success(),
        "wiki list exited non-zero:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    for title in &["Committed", "Staged", "Untracked", "Ignored"] {
        assert!(
            stdout.contains(title),
            "wiki list output missing '{title}'.\nFull output:\n{stdout}"
        );
    }
}
