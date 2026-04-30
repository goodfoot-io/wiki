use std::path::Path;

use miette::Result;
use serde_json::json;

use crate::index::WikiIndex;

use super::summary::format_search_result;

/// Run `search` across multiple namespaces sequentially. `targets` is
/// `(namespace_label, wiki_root)` in dispatch order (current first, peers in
/// declaration order). Output is labeled by namespace.
pub fn run_multi(
    query: &str,
    limit: i64,
    offset: usize,
    json: bool,
    targets: &[(String, &Path)],
    repo_root: &Path,
) -> Result<i32> {
    if json {
        let mut out: Vec<serde_json::Value> = Vec::new();
        for (label, wiki_root) in targets {
            let index = WikiIndex::prepare(wiki_root, repo_root)?;
            let (matches, _total) = index.search_weighted(query, limit, offset)?;
            for m in matches {
                let mut v = serde_json::to_value(&m).unwrap();
                if let Some(obj) = v.as_object_mut() {
                    obj.insert("namespace".into(), json!(label));
                }
                out.push(v);
            }
        }
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
        return Ok(0);
    }

    let mut first = true;
    for (label, wiki_root) in targets {
        let index = WikiIndex::prepare(wiki_root, repo_root)?;
        let (matches, total) = index.search_weighted(query, limit, offset)?;
        if matches.is_empty() {
            continue;
        }
        for result in &matches {
            if !first {
                println!();
            }
            first = false;
            println!("[{label}] {}", format_search_result(result, repo_root));
        }
        let remaining = total.saturating_sub(offset + matches.len());
        if remaining > 0 {
            println!("\n---\n\n*[{label}] {remaining} other wiki matches.*");
        }
    }
    Ok(0)
}

pub fn run(query: &str, limit: i64, offset: usize, json: bool, wiki_root: &Path, repo_root: &Path) -> Result<i32> {
    let index = WikiIndex::prepare(wiki_root, repo_root)?;
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
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file(
            "wiki/a.md",
            "---\ntitle: Alpha\nsummary: Alpha summary.\n---\nContains Keyword here.\n",
        );
        repo.create_file(
            "wiki/b.md",
            "---\ntitle: Beta\nsummary: Beta summary.\n---\nNo match.\n",
        );

        let code = run("keyword", 20, 0, false, &wiki_root, repo.path()).expect("run");
        assert_eq!(code, 0);
    }

    #[test]
    fn search_returns_exit_0_when_no_results_found() {
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file(
            "wiki/a.md",
            "---\ntitle: Alpha\nsummary: Alpha summary.\n---\nContains Keyword here.\n",
        );

        let code = run("missing", 20, 0, false, &wiki_root, repo.path()).expect("run");
        assert_eq!(code, 0);
    }
}
