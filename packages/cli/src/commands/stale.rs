use std::collections::HashMap;
use std::path::Path;

use miette::Result;
use serde::Serialize;

use crate::commands::discover_files;
use crate::git::{commits_since, diff_patch, diff_stat};
use crate::parser::{LinkKind, parse_fragment_links};

// ── Output types ──────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct StaleEntry {
    pub wiki_file: String,
    pub source_line: usize,
    pub referenced_path: String,
    pub pinned_sha: String,
    pub commit_count: usize,
    pub latest_commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct StaleError {
    pub wiki_file: String,
    pub source_line: usize,
    pub referenced_path: String,
    pub pinned_sha: String,
    pub error: String,
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Run the stale command.
///
/// `diff_mode`: `None` = no diff, `Some("stat")` = git diff --stat,
/// `Some("patch")` = full diff.
///
/// Returns exit code: 0 = none stale, 1 = any stale, 2 = runtime error.
pub fn run(globs: &[String], diff_mode: Option<&str>, json: bool, repo_root: &Path) -> Result<i32> {
    run_with_git(
        globs,
        diff_mode,
        json,
        repo_root,
        commits_since,
        diff_stat,
        diff_patch,
    )
}

fn run_with_git(
    globs: &[String],
    diff_mode: Option<&str>,
    json: bool,
    repo_root: &Path,
    commits_since_fn: impl Fn(&Path, &str, &Path) -> Result<Vec<crate::git::CommitInfo>>,
    diff_stat_fn: impl Fn(&Path, &str, &Path) -> Result<String>,
    diff_patch_fn: impl Fn(&Path, &str, &Path) -> Result<String>,
) -> Result<i32> {
    let files = match discover_files(globs, repo_root) {
        Ok(f) => f,
        Err(e) => {
            if json {
                eprintln!("{}", serde_json::json!({"error": e.to_string()}));
            } else {
                eprintln!("error: {e}");
            }
            return Ok(2);
        }
    };

    let mut stale_entries: Vec<StaleEntry> = Vec::new();
    let mut stale_errors: Vec<StaleError> = Vec::new();
    let mut commits_cache: HashMap<(String, String), Result<Vec<crate::git::CommitInfo>>> =
        HashMap::new();
    let mut diff_stat_cache: HashMap<(String, String), Option<String>> = HashMap::new();
    let mut diff_patch_cache: HashMap<(String, String), Option<String>> = HashMap::new();

    for path in &files {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("warning: failed to read {}: {e}", path.display());
                continue;
            }
        };

        let frag_links = parse_fragment_links(&content);
        for link in &frag_links {
            if link.kind != LinkKind::InternalWithSha {
                continue;
            }

            let sha = link.sha.as_deref().unwrap();
            let resolved_path = crate::commands::resolve_link_path(&link.path, path, repo_root);
            let link_path = resolved_path.as_path();
            let cache_key = (sha.to_string(), link_path.to_string_lossy().into_owned());

            let commits = match commits_cache
                .entry(cache_key.clone())
                .or_insert_with(|| commits_since_fn(repo_root, sha, link_path))
            {
                Ok(c) => c,
                Err(e) => {
                    // Report the error rather than silently skipping
                    stale_errors.push(StaleError {
                        wiki_file: path.display().to_string(),
                        source_line: link.source_line,
                        referenced_path: link.path.clone(),
                        pinned_sha: sha.to_string(),
                        error: e.to_string(),
                    });
                    continue;
                }
            };

            if commits.is_empty() {
                continue;
            }

            let latest_commit = commits.first().map(|c| format!("{} {}", c.sha, c.message));

            let diff = match diff_mode {
                Some("stat") => diff_stat_cache
                    .entry(cache_key.clone())
                    .or_insert_with(|| diff_stat_fn(repo_root, sha, link_path).ok())
                    .clone(),
                Some("patch") => diff_patch_cache
                    .entry(cache_key)
                    .or_insert_with(|| diff_patch_fn(repo_root, sha, link_path).ok())
                    .clone(),
                _ => None,
            };

            stale_entries.push(StaleEntry {
                wiki_file: path.display().to_string(),
                source_line: link.source_line,
                referenced_path: link.path.clone(),
                pinned_sha: sha.to_string(),
                commit_count: commits.len(),
                latest_commit,
                diff,
            });
        }
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "stale": stale_entries,
                "errors": stale_errors,
            }))
            .unwrap()
        );
    } else {
        if stale_entries.is_empty() && stale_errors.is_empty() {
            println!("No stale links found.");
        } else {
            for entry in &stale_entries {
                let commit_word = if entry.commit_count == 1 {
                    "commit"
                } else {
                    "commits"
                };
                println!(
                    "`{}:{}` — `{}`",
                    entry.wiki_file, entry.source_line, entry.referenced_path
                );
                println!(
                    "Pinned `@{}` · {} {} behind",
                    entry.pinned_sha, entry.commit_count, commit_word
                );
                if let Some(latest) = &entry.latest_commit {
                    if let Some(idx) = latest.find(' ') {
                        println!("Latest: `{}` — {}", &latest[..idx], &latest[idx + 1..]);
                    } else {
                        println!("Latest: `{latest}`");
                    }
                }
                if let Some(diff) = &entry.diff {
                    println!("```bash\n{}\n```", diff.trim_end());
                }
                println!();
            }
            for err in &stale_errors {
                eprintln!(
                    "**error** — `{}:{}` `{}` pinned `@{}`\n{}\n",
                    err.wiki_file, err.source_line, err.referenced_path, err.pinned_sha, err.error
                );
            }
        }
    }

    if !stale_errors.is_empty() {
        Ok(2)
    } else if stale_entries.is_empty() {
        Ok(0)
    } else {
        Ok(1)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;

    struct TestRepo {
        dir: TempDir,
    }

    impl TestRepo {
        fn new() -> Self {
            let dir = TempDir::new().expect("tempdir");
            let repo = TestRepo { dir };
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
            fs::write(&full, content).expect("write file");
        }

        fn commit(&self, message: &str) -> String {
            self.git(&["add", "-A"]);
            self.git(&["commit", "-m", message]);
            let out = Command::new("git")
                .current_dir(self.dir.path())
                .args(["rev-parse", "--short", "HEAD"])
                .output()
                .expect("git rev-parse");
            String::from_utf8(out.stdout)
                .expect("utf8")
                .trim()
                .to_string()
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
    fn test_stale_clean_repo_exit_0() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file("src/foo.rs", "fn foo() {}\n");
        let sha = repo.commit("add src");

        repo.create_file(
            "wiki/page.md",
            &format!("---\ntitle: Page\nsummary: A page.\n---\n[code](src/foo.rs#L1&{sha})\n"),
        );
        repo.commit("add wiki");

        let code = run(&[], None, false, repo.path()).expect("run");
        assert_eq!(code, 0);
    }

    #[test]
    fn test_stale_detects_stale_after_modification() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file("src/foo.rs", "fn foo() {}\n");
        let sha = repo.commit("add src");

        repo.create_file(
            "wiki/page.md",
            &format!("---\ntitle: Page\nsummary: A page.\n---\n[code](src/foo.rs#L1&{sha})\n"),
        );
        repo.commit("add wiki");

        // Modify the source file after pinning
        repo.create_file("src/foo.rs", "fn foo() {}\nfn bar() {}\n");
        repo.commit("update src");

        let code = run(&[], None, false, repo.path()).expect("run");
        assert_eq!(code, 1);
    }

    #[test]
    fn test_stale_diff_stat_mode() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file("src/foo.rs", "fn foo() {}\n");
        let sha = repo.commit("add src");

        repo.create_file(
            "wiki/page.md",
            &format!("---\ntitle: Page\nsummary: A page.\n---\n[code](src/foo.rs#L1&{sha})\n"),
        );
        repo.commit("add wiki");

        repo.create_file("src/foo.rs", "fn foo() {}\nfn bar() {}\n");
        repo.commit("update src");

        let code = run(&[], Some("stat"), false, repo.path()).expect("run");
        assert_eq!(code, 1);
    }

    #[test]
    fn test_stale_diff_patch_mode() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file("src/foo.rs", "fn foo() {}\n");
        let sha = repo.commit("add src");

        repo.create_file(
            "wiki/page.md",
            &format!("---\ntitle: Page\nsummary: A page.\n---\n[code](src/foo.rs#L1&{sha})\n"),
        );
        repo.commit("add wiki");

        repo.create_file("src/foo.rs", "fn foo() {}\nfn bar() {}\n");
        repo.commit("update src");

        let code = run(&[], Some("patch"), false, repo.path()).expect("run");
        assert_eq!(code, 1);
    }

    #[test]
    fn test_stale_json_output() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file("src/foo.rs", "fn foo() {}\n");
        let sha = repo.commit("add src");

        repo.create_file(
            "wiki/page.md",
            &format!("---\ntitle: Page\nsummary: A page.\n---\n[code](src/foo.rs#L1&{sha})\n"),
        );
        repo.commit("add wiki");

        repo.create_file("src/foo.rs", "fn foo() {}\nfn bar() {}\n");
        repo.commit("update src");

        let code = run(&[], None, true, repo.path()).expect("run");
        assert_eq!(code, 1);
    }

    #[test]
    fn test_stale_bad_sha_reports_error_exit_2() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file("src/foo.rs", "fn foo() {}\n");
        repo.commit("add src");

        // Use a bogus 7-char hex SHA that won't exist in the repo
        let bogus_sha = "deadbee";
        repo.create_file(
            "wiki/page.md",
            &format!(
                "---\ntitle: Page\nsummary: A page.\n---\n[code](src/foo.rs#L1&{bogus_sha})\n"
            ),
        );
        repo.commit("add wiki");

        let code = run(&[], None, false, repo.path()).expect("run");
        // Should exit 2 because commits_since fails on a dangling SHA
        assert_eq!(code, 2);
    }

    #[test]
    fn test_stale_reuses_cached_git_results_for_duplicate_links() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file("src/foo.rs", "fn foo() {}\n");
        let sha = repo.commit("add src");

        repo.create_file(
            "wiki/page.md",
            &format!(
                "---\ntitle: Page\nsummary: A page.\n---\n[one](src/foo.rs#L1&{sha})\n[two](src/foo.rs#L1&{sha})\n"
            ),
        );
        repo.commit("add wiki");

        repo.create_file("src/foo.rs", "fn foo() {}\nfn bar() {}\n");
        repo.commit("update src");

        let commits_calls = Arc::new(AtomicUsize::new(0));
        let diff_calls = Arc::new(AtomicUsize::new(0));

        let commits_calls_for_closure = Arc::clone(&commits_calls);
        let diff_calls_for_closure = Arc::clone(&diff_calls);

        let code = run_with_git(
            &[],
            Some("stat"),
            false,
            repo.path(),
            |repo_root, current_sha, path| {
                commits_calls_for_closure.fetch_add(1, Ordering::SeqCst);
                commits_since(repo_root, current_sha, path)
            },
            |repo_root, current_sha, path| {
                diff_calls_for_closure.fetch_add(1, Ordering::SeqCst);
                diff_stat(repo_root, current_sha, path)
            },
            |_repo_root, _current_sha, _path| -> Result<String> {
                panic!("patch diff should not be used in stat mode")
            },
        )
        .expect("run");

        assert_eq!(code, 1);
        assert_eq!(commits_calls.load(Ordering::SeqCst), 1);
        assert_eq!(diff_calls.load(Ordering::SeqCst), 1);
    }
}
