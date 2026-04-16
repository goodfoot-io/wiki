pub mod check;
pub mod extract;
pub mod hook;
pub mod html;
pub mod install;
pub mod links;
pub mod list;
pub mod pin;
pub mod search;
pub mod serve;
pub mod stale;
pub mod summary;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use miette::{IntoDiagnostic, Result, WrapErr, miette};
use serde_json::json;

#[cfg(test)]
use crate::frontmatter::Frontmatter;
use crate::git::repo_inventory;
use crate::perf;

// ── Path helpers ──────────────────────────────────────────────────────────────

/// Returns true if `s` looks like a file path rather than a wiki title.
///
/// A string is treated as a path when it contains a `/` separator or ends
/// with `.md`, so `wiki/page.md` and `./wiki/page.md` are both paths, while
/// `My Page Title` is a title.
pub fn looks_like_path(s: &str) -> bool {
    s.contains('/') || s.ends_with(".md")
}

/// Normalize a user-supplied path to a repo-relative string for index lookup.
pub fn normalize_repo_relative_path(input: &str, repo_root: &Path) -> String {
    let path = Path::new(input);
    if path.is_absolute() {
        path.strip_prefix(repo_root)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| input.to_string())
    } else {
        input.trim_start_matches("./").to_string()
    }
}

/// Resolve a fragment link path relative to the file it was found in,
/// then return it relative to the repository root.
pub fn resolve_link_path(link_path: &str, source_file: &Path, repo_root: &Path) -> PathBuf {
    let path = Path::new(link_path);
    if path.is_absolute() {
        return path
            .strip_prefix(repo_root)
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|_| path.to_path_buf());
    }

    // Try repo-relative first: if it exists at repo_root/link_path, use it.
    let repo_relative = repo_root.join(path);
    if repo_relative.exists() {
        return PathBuf::from(link_path);
    }

    // Otherwise, treat as relative to the source file.
    let source_dir = source_file.parent().unwrap_or_else(|| Path::new("."));
    let combined = source_dir.join(path);

    // Normalize the path (resolve .. and .)
    let mut components = Vec::new();
    for component in combined.components() {
        match component {
            std::path::Component::ParentDir => {
                components.pop();
            }
            std::path::Component::CurDir => {}
            _ => {
                components.push(component);
            }
        }
    }
    let normalized: PathBuf = components.into_iter().collect();

    normalized
        .strip_prefix(repo_root)
        .map(|p| p.to_path_buf())
        .unwrap_or(normalized)
}

#[cfg(test)]
/// Find a discovered page whose file path corresponds to `path_str`.
///
/// Resolution order for relative paths:
/// 1. Current working directory.
/// 2. `repo_root`.
///
/// Uses `canonicalize` for robust comparison; falls back to literal path
/// equality when canonicalization fails (e.g. on unsaved tempdir paths).
///
/// Returns the page's `PathBuf` as stored in `pages` if found.
pub fn find_page_by_path(
    path_str: &str,
    pages: &[(PathBuf, Frontmatter)],
    repo_root: &Path,
) -> Option<PathBuf> {
    let input = Path::new(path_str);
    let candidates: Vec<PathBuf> = if input.is_absolute() {
        vec![input.to_path_buf()]
    } else {
        let mut v = Vec::new();
        if let Ok(cwd) = std::env::current_dir() {
            v.push(cwd.join(input));
        }
        v.push(repo_root.join(input));
        v
    };

    for (page_path, _) in pages {
        let page_canonical = page_path.canonicalize().ok();
        for candidate in &candidates {
            let c_canonical = candidate.canonicalize().ok();
            if let (Some(cc), Some(cp)) = (&c_canonical, &page_canonical)
                && cc == cp
            {
                return Some(page_path.clone());
            }
            if candidate == page_path {
                return Some(page_path.clone());
            }
        }
    }
    None
}

/// Discover wiki pages via glob expansion.
///
/// Algorithm:
/// 1. Resolve `WIKI_DIR` env var (default `"wiki"`) relative to `repo_root`.
/// 2. If `globs` is non-empty, use those patterns; otherwise default to
///    `$WIKI_DIR/**/*.md`.
/// 3. Fail closed: if `WIKI_DIR` does not exist, return exit-code-2 error.
///    If zero `.md` files are matched, return exit-code-2 error.
pub fn discover_files(globs: &[String], repo_root: &Path) -> Result<Vec<PathBuf>> {
    perf::scope_result(
        "discover_files",
        json!({
            "globs": globs,
        }),
        || {
            let mut files = if globs.is_empty() {
                discover_default_files(repo_root)?
            } else {
                Vec::new()
            };

            if files.is_empty() || !globs.is_empty() {
                files = discover_files_by_walk(globs, repo_root)?;
            }

            files.sort();
            files.dedup();

            if files.is_empty() {
                return Err(miette!("no wiki pages found (no .md files matched)"));
            }

            perf::log_event(
                "discover_files_result",
                0.0,
                "ok",
                json!({
                    "count": files.len(),
                }),
            );

            Ok(files)
        },
    )
}

fn discover_default_files(repo_root: &Path) -> Result<Vec<PathBuf>> {
    let wiki_dir_name = std::env::var("WIKI_DIR").unwrap_or_else(|_| "wiki".to_string());
    let wiki_dir_path = PathBuf::from(&wiki_dir_name);

    if wiki_dir_path.is_absolute() {
        return discover_files_by_walk(&[], repo_root);
    }

    let inventory = match repo_inventory(repo_root) {
        Ok(inventory) => inventory,
        Err(_) => return discover_files_by_walk(&[], repo_root),
    };

    let mut files = Vec::new();
    for path_rel in inventory {
        if !matches_default_discovery_path(&path_rel, &wiki_dir_name) {
            continue;
        }

        let path = repo_root.join(&path_rel);
        if path.is_file() {
            files.push(path);
        }
    }

    Ok(files)
}

fn matches_default_discovery_path(path_rel: &str, wiki_dir_name: &str) -> bool {
    if !path_rel.ends_with(".md") {
        return false;
    }

    if path_rel.ends_with(".wiki.md") {
        return true;
    }

    let path = Path::new(path_rel);
    let Some(first_component) = path.components().next() else {
        return false;
    };

    let wiki_dir = Path::new(wiki_dir_name);
    let Some(wiki_root) = wiki_dir.components().next() else {
        return false;
    };

    if first_component != wiki_root {
        return false;
    }

    path.starts_with(wiki_dir)
}

fn discover_files_by_walk(globs: &[String], repo_root: &Path) -> Result<Vec<PathBuf>> {
    let wiki_dir_name = std::env::var("WIKI_DIR").unwrap_or_else(|_| "wiki".to_string());
    let wiki_dir_path = PathBuf::from(&wiki_dir_name);
    let wiki_dir = if wiki_dir_path.is_absolute() {
        wiki_dir_path.clone()
    } else {
        repo_root.join(&wiki_dir_name)
    };

    let mut files: Vec<PathBuf> = Vec::new();
    let mut search_tasks: Vec<(PathBuf, Vec<String>)> = Vec::new();

    if globs.is_empty() {
        if wiki_dir.exists() {
            if wiki_dir_path.is_absolute() {
                search_tasks.push((wiki_dir.clone(), vec!["**/*.md".to_string()]));
            } else {
                search_tasks.push((
                    repo_root.to_path_buf(),
                    vec![format!("{wiki_dir_name}/**/*.md")],
                ));
            }
        }
        search_tasks.push((repo_root.to_path_buf(), vec!["**/*.wiki.md".to_string()]));
    } else {
        let normalized_globs = globs
            .iter()
            .map(|glob| normalize_repo_relative_path(glob, repo_root))
            .collect();
        search_tasks.push((repo_root.to_path_buf(), normalized_globs));
    };

    for (base_dir, patterns) in search_tasks {
        files.extend(discover_files_by_parallel_walk(&base_dir, &patterns)?);
    }

    files.sort();
    files.dedup();

    Ok(files)
}

fn discover_files_by_parallel_walk(base_dir: &Path, patterns: &[String]) -> Result<Vec<PathBuf>> {
    let mut glob_builder = globset::GlobSetBuilder::new();
    for pattern in patterns {
        let glob = globset::Glob::new(pattern)
            .into_diagnostic()
            .wrap_err_with(|| format!("invalid glob pattern: {pattern}"))?;
        glob_builder.add(glob);
    }
    let glob_set = Arc::new(
        glob_builder
            .build()
            .into_diagnostic()
            .wrap_err("failed to build glob set")?,
    );

    let files = Arc::new(Mutex::new(Vec::<PathBuf>::new()));
    let first_error = Arc::new(Mutex::new(None::<String>));

    ignore::WalkBuilder::new(base_dir)
        .hidden(false)
        .git_global(false)
        .build_parallel()
        .run(|| {
            let glob_set = Arc::clone(&glob_set);
            let files = Arc::clone(&files);
            let first_error = Arc::clone(&first_error);
            let base_dir = base_dir.to_path_buf();

            Box::new(move |entry| {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(error) => {
                        let mut guard = first_error.lock().expect("walk error lock");
                        if guard.is_none() {
                            *guard = Some(error.to_string());
                        }
                        return ignore::WalkState::Quit;
                    }
                };

                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("md") {
                    return ignore::WalkState::Continue;
                }

                let relative = path.strip_prefix(&base_dir).unwrap_or(path);
                if glob_set.is_match(relative) {
                    files
                        .lock()
                        .expect("walk files lock")
                        .push(path.to_path_buf());
                }

                ignore::WalkState::Continue
            })
        });

    if let Some(error) = first_error.lock().expect("walk error lock").clone() {
        return Err(miette!("error walking directory: {error}"));
    }

    let mut files = Arc::into_inner(files)
        .expect("parallel walk files still referenced")
        .into_inner()
        .expect("parallel walk files lock poisoned");
    files.sort();
    files.dedup();
    Ok(files)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontmatter::Frontmatter;
    use std::fs;
    use std::process::Command;
    use tempfile::TempDir;

    fn make_fm(title: &str) -> Frontmatter {
        Frontmatter {
            title: title.into(),
            aliases: vec![],
            tags: vec![],
            keywords: vec![],
            summary: "A summary.".into(),
        }
    }

    #[test]
    fn test_looks_like_path_with_slash() {
        assert!(looks_like_path("wiki/page.md"));
        assert!(looks_like_path("./wiki/page.md"));
        assert!(looks_like_path("some/path"));
    }

    #[test]
    fn test_looks_like_path_with_md_extension() {
        assert!(looks_like_path("page.md"));
    }

    #[test]
    fn test_looks_like_path_title_returns_false() {
        assert!(!looks_like_path("My Page Title"));
        assert!(!looks_like_path("check"));
        assert!(!looks_like_path("Wiki CLI Advanced Usage"));
    }

    #[test]
    fn test_find_page_by_path_repo_root_relative() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        let page_path = root.join("wiki").join("page.md");
        fs::create_dir_all(page_path.parent().unwrap()).expect("mkdir");
        fs::write(&page_path, "").expect("write");

        let pages = vec![(page_path.clone(), make_fm("Page"))];
        let result = find_page_by_path("wiki/page.md", &pages, root);
        assert_eq!(result, Some(page_path));
    }

    #[test]
    fn test_find_page_by_path_absolute() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        let page_path = root.join("wiki").join("page.md");
        fs::create_dir_all(page_path.parent().unwrap()).expect("mkdir");
        fs::write(&page_path, "").expect("write");

        let pages = vec![(page_path.clone(), make_fm("Page"))];
        let result = find_page_by_path(page_path.to_str().unwrap(), &pages, root);
        assert_eq!(result, Some(page_path));
    }

    #[test]
    fn test_find_page_by_path_not_found() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        let page_path = root.join("wiki").join("page.md");
        fs::create_dir_all(page_path.parent().unwrap()).expect("mkdir");
        fs::write(&page_path, "").expect("write");

        let pages = vec![(page_path.clone(), make_fm("Page"))];
        let result = find_page_by_path("wiki/other.md", &pages, root);
        assert!(result.is_none());
    }

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

        #[allow(dead_code)]
        fn commit(&self, message: &str) {
            self.git(&["add", "-A"]);
            self.git(&["commit", "-m", message]);
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
    fn test_discover_missing_wiki_dir_exits_2_with_no_pages() {
        let repo = TestRepo::new();
        // No wiki dir created
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        let err = discover_files(&[], repo.path()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no wiki pages found"), "got: {msg}");
    }

    #[test]
    fn test_discover_empty_wiki_dir_exits_2() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file("wiki/.gitkeep", "");
        let err = discover_files(&[], repo.path()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no wiki pages found"), "got: {msg}");
    }

    #[test]
    fn test_discover_finds_md_files() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file("wiki/page.md", "---\ntitle: Page\nsummary: A page.\n---\n");
        let files = discover_files(&[], repo.path()).expect("discover");
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("page.md"));
    }

    #[test]
    fn test_discover_finds_wiki_md_files_outside_wiki_dir() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file("wiki/page.md", "---\ntitle: Page\nsummary: A page.\n---\n");
        repo.create_file(
            "src/component/docs.wiki.md",
            "---\ntitle: Docs\nsummary: Component docs.\n---\n",
        );
        repo.create_file("src/component/ordinary.md", "# ordinary\n");
        let files = discover_files(&[], repo.path()).expect("discover");
        assert_eq!(files.len(), 2);
        let paths: Vec<_> = files
            .into_iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        assert!(
            paths
                .iter()
                .any(|p| p.ends_with("src/component/docs.wiki.md"))
        );
        assert!(paths.iter().any(|p| p.ends_with("wiki/page.md")));
        assert!(!paths.iter().any(|p| p.ends_with("ordinary.md")));
    }

    #[test]
    fn test_discover_finds_wiki_md_files_without_wiki_dir() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file(
            "src/component/docs.wiki.md",
            "---\ntitle: Docs\nsummary: Component docs.\n---\n",
        );
        let files = discover_files(&[], repo.path()).expect("discover");
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("docs.wiki.md"));
    }

    #[test]
    fn test_discover_explicit_glob_zero_matches_exits_2() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file("wiki/page.md", "---\ntitle: Page\nsummary: A page.\n---\n");
        let globs = vec!["wiki/nonexistent/**/*.md".to_string()];
        let err = discover_files(&globs, repo.path()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no wiki pages found"), "got: {msg}");
    }

    #[test]
    fn test_discover_explicit_glob_works_without_wiki_dir() {
        let repo = TestRepo::new();
        // Set WIKI_DIR to a nonexistent directory — explicit globs must bypass this check
        let _wiki_dir = crate::test_support::set_wiki_dir("does_not_exist");
        // Create docs/ instead of wiki/
        repo.create_file("docs/page.md", "---\ntitle: Page\nsummary: A page.\n---\n");
        let globs = vec!["docs/**/*.md".to_string()];
        let files = discover_files(&globs, repo.path())
            .expect("explicit glob should succeed without WIKI_DIR");
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("page.md"));
    }

    #[test]
    fn test_discover_explicit_glob_with_dot_slash_prefix() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file("wiki/page.md", "---\ntitle: Page\nsummary: A page.\n---\n");
        let globs = vec!["./wiki/page.md".to_string()];
        let files = discover_files(&globs, repo.path()).expect("discover");
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("wiki/page.md"));
    }

    #[test]
    fn test_discover_skips_gitignored_directories() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file("wiki/page.md", "---\ntitle: Page\nsummary: A page.\n---\n");
        // Create a wiki page inside a gitignored directory.
        repo.create_file(
            "ignored-dir/stale.wiki.md",
            "---\ntitle: Stale\nsummary: Should be excluded.\n---\n",
        );
        // Gitignore the directory — discover_files must not return files from it.
        repo.create_file(".gitignore", "ignored-dir/\n");
        let files = discover_files(&[], repo.path()).expect("discover");
        let paths: Vec<_> = files
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        assert!(
            paths.iter().all(|p| !p.contains("ignored-dir")),
            "gitignored directory must be excluded, got: {paths:?}"
        );
    }

    #[test]
    fn test_discover_skips_git_worktrees() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file("wiki/page.md", "---\ntitle: Page\nsummary: A page.\n---\n");
        // Simulate a real git worktree: it contains a .git FILE (not directory)
        // pointing back to the main repo's .git/worktrees/... directory.
        repo.create_file(
            ".worktrees/cards/main-265/1/.git",
            "gitdir: /workspace/.git/worktrees/main-265-1\n",
        );
        repo.create_file(
            ".worktrees/cards/main-265/1/documentation/monetezation.wiki.md",
            "---\ntitle: compare branch monetization\nsummary: Stale.\n---\n",
        );
        // Gitignore the worktrees directory (as this repo does in production).
        repo.create_file(".gitignore", ".worktrees\n");
        let files = discover_files(&[], repo.path()).expect("discover");
        let paths: Vec<_> = files
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        assert_eq!(
            files.len(),
            1,
            "only the main wiki page must be found, got: {paths:?}"
        );
        assert!(
            paths[0].ends_with("wiki/page.md"),
            "unexpected path: {}",
            paths[0]
        );
    }

    #[test]
    fn test_parallel_walk_matches_git_inventory_for_default_semantics() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file("wiki/page.md", "---\ntitle: Page\nsummary: A page.\n---\n");
        repo.create_file(
            "wiki/nested/child.md",
            "---\ntitle: Child\nsummary: Child page.\n---\n",
        );
        repo.create_file(
            "docs/reference.wiki.md",
            "---\ntitle: Reference\nsummary: Reference.\n---\n",
        );
        repo.create_file(
            "ignored-dir/stale.wiki.md",
            "---\ntitle: Stale\nsummary: Should be excluded.\n---\n",
        );
        repo.create_file(".gitignore", "ignored-dir/\n");
        repo.git(&["add", "-A"]);

        let inventory_files = discover_default_files(repo.path()).expect("inventory discover");
        let walk_files = discover_files_by_walk(&[], repo.path()).expect("walk discover");

        assert_eq!(inventory_files, walk_files);
    }
}
