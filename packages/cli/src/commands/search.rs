use std::path::Path;

use miette::Result;

use crate::index::WikiIndex;

use super::summary::format_search_result;

pub fn run(query: &str, limit: i64, offset: usize, json: bool, repo_root: &Path) -> Result<i32> {
    let index = WikiIndex::prepare(repo_root)?;
    let (matches, total) = index.search_weighted(query, limit, offset)?;

    if matches.is_empty() {
        if json {
            println!("[]");
        }
        return Ok(0);
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&matches).unwrap());
    } else {
        for (i, result) in matches.iter().enumerate() {
            if i > 0 {
                println!();
            }
            println!("{}", format_search_result(result, repo_root));
        }
        let remaining = total.saturating_sub(offset + matches.len());
        if remaining > 0 {
            println!("\n---\n\n*{remaining} other wiki matches.*");
        }
    }

    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn search_matches_case_insensitively() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file(
            "wiki/a.md",
            "---\ntitle: Alpha\nsummary: Alpha summary.\n---\nContains Keyword here.\n",
        );
        repo.create_file(
            "wiki/b.md",
            "---\ntitle: Beta\nsummary: Beta summary.\n---\nNo match.\n",
        );

        let code = run("keyword", 20, 0, false, repo.path()).expect("run");
        assert_eq!(code, 0);
    }

    #[test]
    fn search_returns_exit_0_when_no_results_found() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file(
            "wiki/a.md",
            "---\ntitle: Alpha\nsummary: Alpha summary.\n---\nContains Keyword here.\n",
        );

        let code = run("missing", 20, 0, false, repo.path()).expect("run");
        assert_eq!(code, 0);
    }
}
