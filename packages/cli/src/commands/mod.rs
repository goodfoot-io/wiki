pub mod check;
pub mod hook_check;
pub mod init;
pub mod install;
pub mod links;
pub mod list;
pub mod mesh;
pub(crate) mod mesh_coverage;
pub mod refs;
pub mod search;
pub mod summary;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use miette::{IntoDiagnostic, Result, WrapErr, miette};
use serde_json::json;

#[cfg(test)]
use crate::frontmatter::Frontmatter;
use crate::git::repo_inventory;
use crate::index::DocSource;
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
        return path
            .strip_prefix(repo_root)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| input.to_string());
    }
    // Resolve cwd-relative paths (including those with `..` segments) by
    // joining against the current working directory and re-stripping the
    // repo root. Falls back to the literal input when canonicalization
    // fails or the path escapes the repo entirely.
    if input.contains("..")
        && let Ok(cwd) = std::env::current_dir()
    {
        let joined = cwd.join(input);
        let mut components = Vec::new();
        for component in joined.components() {
            match component {
                std::path::Component::ParentDir => {
                    components.pop();
                }
                std::path::Component::CurDir => {}
                c => components.push(c),
            }
        }
        let normalized: PathBuf = components.into_iter().collect();
        if let Ok(stripped) = normalized.strip_prefix(repo_root) {
            return stripped.to_string_lossy().into_owned();
        }
        if let (Ok(c1), Ok(c2)) = (
            std::fs::canonicalize(&normalized),
            std::fs::canonicalize(repo_root),
        ) && let Ok(stripped) = c1.strip_prefix(&c2)
        {
            return stripped.to_string_lossy().into_owned();
        }
    }
    input.trim_start_matches("./").to_string()
}

/// Resolve a fragment link path relative to the file it was found in,
/// then return it relative to the repository root.
pub fn resolve_link_path(link_path: &str, source_file: &Path, repo_root: &Path) -> PathBuf {
    let path = Path::new(link_path);
    if path.is_absolute() {
        return path
            .strip_prefix(repo_root)
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|_| {
                path.strip_prefix("/")
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|_| path.to_path_buf())
            });
    }

    // Treat as relative to the source file.
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

/// Discover wiki pages.
///
/// When `globs` is empty, the default set is `wiki_root/**/*.md` ∪
/// `repo_root/**/*.wiki.md`. Explicit globs are matched relative to
/// `repo_root`. Fail closed: returns an error if zero `.md` files are matched.
///
/// `source` controls which tree is used to seed the candidate list when globs
/// are empty. For `Index` and `Head`, candidate paths are taken from
/// `source.list_paths()` so files absent from the worktree are still included.
pub fn discover_files(
    globs: &[String],
    wiki_root: &Path,
    repo_root: &Path,
    source: DocSource,
) -> Result<Vec<PathBuf>> {
    perf::scope_result(
        "discover_files",
        json!({
            "globs": globs,
        }),
        || {
            let mut files = match source {
                DocSource::Index | DocSource::Head => {
                    if globs.is_empty() {
                        discover_default_files(wiki_root, repo_root, source)?
                    } else {
                        // For non-worktree sources we must never read the
                        // worktree filesystem to satisfy a glob.  Filter the
                        // source's own path list instead so the candidate set
                        // is internally consistent with `--source`.
                        discover_files_by_glob_in_source(globs, wiki_root, repo_root, source)?
                    }
                }
                DocSource::WorkingTree => {
                    let initial = if globs.is_empty() {
                        discover_default_files(wiki_root, repo_root, source)?
                    } else {
                        Vec::new()
                    };
                    if initial.is_empty() || !globs.is_empty() {
                        discover_files_by_walk(globs, wiki_root, repo_root)?
                    } else {
                        initial
                    }
                }
            };

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

fn discover_default_files(
    wiki_root: &Path,
    repo_root: &Path,
    source: DocSource,
) -> Result<Vec<PathBuf>> {
    // For non-worktree sources, seed from the source's own path list so that
    // files absent from the worktree (deleted locally but present in HEAD or
    // the index) are still included in the candidate set.
    match source {
        DocSource::Index | DocSource::Head => {
            let all_paths = source.list_paths(repo_root)?;
            let files: Vec<PathBuf> = all_paths
                .into_iter()
                .filter(|p| matches_default_discovery_path(p, wiki_root, repo_root))
                .map(|p| repo_root.join(p))
                .collect();
            return Ok(files);
        }
        DocSource::WorkingTree => {}
    }

    let inventory = match repo_inventory(repo_root) {
        Ok(inventory) => inventory,
        Err(_) => return discover_files_by_walk(&[], wiki_root, repo_root),
    };

    let mut files = Vec::new();
    for path_rel in inventory {
        if !matches_default_discovery_path(&path_rel, wiki_root, repo_root) {
            continue;
        }

        let path = repo_root.join(&path_rel);
        if path.is_file() {
            files.push(path);
        }
    }

    Ok(files)
}

pub(crate) fn matches_default_discovery_path(
    path_rel: &str,
    wiki_root: &Path,
    repo_root: &Path,
) -> bool {
    if !path_rel.ends_with(".md") {
        return false;
    }

    if path_rel.ends_with(".wiki.md") {
        return true;
    }

    let abs = repo_root.join(path_rel);
    abs.starts_with(wiki_root)
}

/// Filter a `DocSource`'s path list against the same glob semantics as
/// `discover_files_by_walk`: globs are normalised to repo-relative form and
/// matched against the source's repo-relative paths.  Used under
/// `--source=index|head` so glob discovery never reads the worktree.
fn discover_files_by_glob_in_source(
    globs: &[String],
    _wiki_root: &Path,
    repo_root: &Path,
    source: DocSource,
) -> Result<Vec<PathBuf>> {
    let mut glob_builder = globset::GlobSetBuilder::new();
    for glob in globs {
        let normalized = normalize_repo_relative_path(glob, repo_root);
        let glob = globset::Glob::new(&normalized)
            .into_diagnostic()
            .wrap_err_with(|| format!("invalid glob pattern: {normalized}"))?;
        glob_builder.add(glob);
    }
    let glob_set = glob_builder
        .build()
        .into_diagnostic()
        .wrap_err("failed to build glob set")?;

    let mut files = Vec::new();
    for path_rel in source.list_paths(repo_root)? {
        if !path_rel.ends_with(".md") {
            continue;
        }
        if glob_set.is_match(&path_rel) {
            files.push(repo_root.join(&path_rel));
        }
    }
    files.sort();
    files.dedup();
    Ok(files)
}

fn discover_files_by_walk(
    globs: &[String],
    wiki_root: &Path,
    repo_root: &Path,
) -> Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = Vec::new();
    let mut search_tasks: Vec<(PathBuf, Vec<String>)> = Vec::new();

    if globs.is_empty() {
        if wiki_root.exists() {
            search_tasks.push((wiki_root.to_path_buf(), vec!["**/*.md".to_string()]));
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
            namespace: None,
        }
    }

    #[test]
    fn test_resolve_link_path_bare_nonexistent_is_page_relative() {
        // A bare path like `packages/wiki/src/commands/serve.rs` now resolves
        // relative to the source page's directory, regardless of file existence.
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        let source = root.join("wiki/guides/page.md");
        let result = resolve_link_path("packages/wiki/src/commands/serve.rs", &source, root);
        assert_eq!(
            result,
            PathBuf::from("wiki/guides/packages/wiki/src/commands/serve.rs"),
            "bare path must be resolved relative to the source page's directory"
        );
    }

    #[test]
    fn test_resolve_link_path_dotdot_uses_file_relative() {
        // An explicit `../` path must still be resolved relative to the source file.
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        let source = root.join("wiki/guides/page.md");
        let result = resolve_link_path("../architecture/design.md", &source, root);
        assert_eq!(result, PathBuf::from("wiki/architecture/design.md"));
    }

    #[test]
    fn test_resolve_link_path_bare_path_is_page_relative() {
        // Bare paths (without `./` or `../` prefix) are now resolved relative to
        // the source page's directory, matching standard markdown behavior.
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        let source = root.join("marketing/design/pages/example.md");
        let result = resolve_link_path("images/screenshot.png", &source, root);
        assert_eq!(
            result,
            PathBuf::from("marketing/design/pages/images/screenshot.png"),
            "bare paths must be resolved relative to the source page's directory"
        );
    }

    #[test]
    fn test_resolve_link_path_slash_prefix_is_repo_relative() {
        // A `/`-prefixed path resolves relative to the repository root.
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        let source = root.join("wiki/guides/page.md");
        let result = resolve_link_path("/packages/cli/src/main.rs", &source, root);
        assert_eq!(
            result,
            PathBuf::from("packages/cli/src/main.rs"),
            "/-prefixed paths must be resolved relative to the repository root"
        );
    }

    #[test]
    fn test_resolve_link_path_slash_prefix_absolute_under_repo_root() {
        // An absolute path that falls under repo_root is still stripped to repo-relative.
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        let source = root.join("wiki/guides/page.md");
        let absolute_path = root.join("packages/cli/src/main.rs");
        let result = resolve_link_path(absolute_path.to_str().unwrap(), &source, root);
        assert_eq!(
            result,
            PathBuf::from("packages/cli/src/main.rs"),
            "absolute paths under repo_root must be stripped to repo-relative"
        );
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
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        let err = discover_files(&[], &wiki_root, repo.path(), DocSource::WorkingTree).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no wiki pages found"), "got: {msg}");
    }

    #[test]
    fn test_discover_empty_wiki_dir_exits_2() {
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file("wiki/.gitkeep", "");
        let err = discover_files(&[], &wiki_root, repo.path(), DocSource::WorkingTree).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no wiki pages found"), "got: {msg}");
    }

    #[test]
    fn test_discover_finds_md_files() {
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file("wiki/page.md", "---\ntitle: Page\nsummary: A page.\n---\n");
        let files =
            discover_files(&[], &wiki_root, repo.path(), DocSource::WorkingTree).expect("discover");
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("page.md"));
    }

    #[test]
    fn test_discover_finds_wiki_md_files_outside_wiki_dir() {
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file("wiki/page.md", "---\ntitle: Page\nsummary: A page.\n---\n");
        repo.create_file(
            "src/component/docs.wiki.md",
            "---\ntitle: Docs\nsummary: Component docs.\n---\n",
        );
        repo.create_file("src/component/ordinary.md", "# ordinary\n");
        let files =
            discover_files(&[], &wiki_root, repo.path(), DocSource::WorkingTree).expect("discover");
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
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file(
            "src/component/docs.wiki.md",
            "---\ntitle: Docs\nsummary: Component docs.\n---\n",
        );
        let files =
            discover_files(&[], &wiki_root, repo.path(), DocSource::WorkingTree).expect("discover");
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("docs.wiki.md"));
    }

    #[test]
    fn test_discover_explicit_glob_zero_matches_exits_2() {
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file("wiki/page.md", "---\ntitle: Page\nsummary: A page.\n---\n");
        let globs = vec!["wiki/nonexistent/**/*.md".to_string()];
        let err =
            discover_files(&globs, &wiki_root, repo.path(), DocSource::WorkingTree).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no wiki pages found"), "got: {msg}");
    }

    #[test]
    fn test_discover_explicit_glob_works_without_wiki_dir() {
        let repo = TestRepo::new();
        // Set WIKI_DIR to a nonexistent directory — explicit globs must bypass this check
        let wiki_root = repo.path().join("does_not_exist");
        // Create docs/ instead of wiki/
        repo.create_file("docs/page.md", "---\ntitle: Page\nsummary: A page.\n---\n");
        let globs = vec!["docs/**/*.md".to_string()];
        let files = discover_files(&globs, &wiki_root, repo.path(), DocSource::WorkingTree)
            .expect("explicit glob should succeed without WIKI_DIR");
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("page.md"));
    }

    #[test]
    fn test_discover_explicit_glob_with_dot_slash_prefix() {
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file("wiki/page.md", "---\ntitle: Page\nsummary: A page.\n---\n");
        let globs = vec!["./wiki/page.md".to_string()];
        let files = discover_files(&globs, &wiki_root, repo.path(), DocSource::WorkingTree)
            .expect("discover");
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("wiki/page.md"));
    }

    #[test]
    fn test_discover_skips_gitignored_directories() {
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file("wiki/page.md", "---\ntitle: Page\nsummary: A page.\n---\n");
        // Create a wiki page inside a gitignored directory.
        repo.create_file(
            "ignored-dir/stale.wiki.md",
            "---\ntitle: Stale\nsummary: Should be excluded.\n---\n",
        );
        // Gitignore the directory — discover_files must not return files from it.
        repo.create_file(".gitignore", "ignored-dir/\n");
        let files =
            discover_files(&[], &wiki_root, repo.path(), DocSource::WorkingTree).expect("discover");
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
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
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
        let files =
            discover_files(&[], &wiki_root, repo.path(), DocSource::WorkingTree).expect("discover");
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
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
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

        let inventory_files =
            discover_default_files(&wiki_root, repo.path(), DocSource::WorkingTree)
                .expect("inventory discover");
        let walk_files =
            discover_files_by_walk(&[], &wiki_root, repo.path()).expect("walk discover");

        assert_eq!(inventory_files, walk_files);
    }
}
