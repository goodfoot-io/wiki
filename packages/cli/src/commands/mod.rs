pub mod check;
pub mod check_fix;
pub mod drift;
pub mod list;
pub mod search;
pub mod summary;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use miette::{IntoDiagnostic, Result, WrapErr, miette};
use serde_json::json;

#[cfg(test)]
use crate::frontmatter::Frontmatter;
use crate::git::GitReader;
use crate::git::repo_inventory;
use crate::index::DocSource;
use crate::perf;

// ── Path helpers ──────────────────────────────────────────────────────────────

/// Returns true if `s` looks like a file path rather than a wiki title.
///
/// A string is treated as a path when it contains a `/` separator or ends
/// with `.md`, so `wiki/page.md` and `./wiki/page.md` are both paths, while
/// `My Page Title` is a title.
#[cfg(test)]
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
    // Same-page anchor (empty path): the target is the source file itself.
    if link_path.is_empty() {
        return source_file
            .strip_prefix(repo_root)
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|_| source_file.to_path_buf());
    }

    // A genuine filesystem-absolute path that falls under repo_root is
    // stripped to repo-relative. This must be checked before the leading-
    // slash rule below: on POSIX an absolute path *also* starts with `/`,
    // so stripping the slash first would yield `tmp/.../packages/...`
    // instead of the repo-relative path.
    let path = Path::new(link_path);
    if path.is_absolute()
        && let Ok(stripped) = path.strip_prefix(repo_root)
    {
        return stripped.to_path_buf();
    }

    // Repo-root-absolute links use a POSIX-style leading slash (e.g.
    // `/packages/cli/src/parser.rs`). `Path::is_absolute()` is
    // platform-dependent — on Windows a rootless `/foo` path is *not*
    // absolute — so detect the leading slash explicitly and resolve it
    // relative to the repo root rather than relying on `is_absolute()`.
    if let Some(rest) = link_path.strip_prefix('/') {
        return PathBuf::from(rest);
    }

    if path.is_absolute() {
        return path.to_path_buf();
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

/// Suffix salvage for a repo-relative path that does not exist as written:
/// the longest repo-relative suffix of the path that DOES exist. A href like
/// `wiki/deep/path/src/code.rs` whose real file is `src/code.rs` resolves
/// to the existing suffix. The drift engine uses it for target resolution
/// (plan Decision 5).
pub(crate) fn locate_existing_suffix(rel_path: &str, repo_root: &Path) -> Option<String> {
    // If the path is an absolute path that resolves entirely outside the
    // repo, do not attempt suffix matching — a coincidental in-repo suffix
    // (e.g. `src/lib.rs`) would produce the wrong file.
    let p = Path::new(rel_path);
    if p.is_absolute() && !p.starts_with(repo_root) {
        return None;
    }

    if repo_root.join(rel_path).exists() {
        return Some(rel_path.to_string());
    }
    let parts: Vec<&str> = rel_path.split('/').collect();
    for start in 1..parts.len() {
        let candidate = parts[start..].join("/");
        if candidate.is_empty() {
            continue;
        }
        if repo_root.join(&candidate).exists() {
            return Some(candidate);
        }
    }
    None
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
/// When `globs` is empty, walks the repo from `repo_root` for all `.md` files
/// and filters by content-based wiki membership (both `title:` and `summary:`
/// frontmatter fields must be present and non-empty). Explicit globs are
/// matched relative to `repo_root`. Fail closed: returns an error if zero
/// `.md` files are matched.
///
/// `source` controls which tree is used to seed the candidate list when globs
/// are empty. For `Index` and `Head`, candidate paths are taken from
/// `source.list_paths()` so files absent from the worktree are still included.
/// Discover the wiki `*.md` files to operate on.
///
/// File **selection** is scoped to `scan_root` (the current working directory):
/// a bare invocation discovers only the subtree beneath it, and explicit globs
/// resolve relative to it. `repo_root` (the git top-level) is used only for
/// path **resolution** — the returned paths are absolute under `repo_root` so
/// that downstream `strip_prefix(repo_root)` yields git-root-relative paths
/// regardless of where the caller stood. `scan_root` is always a descendant of
/// (or equal to) `repo_root`.
pub fn discover_files(
    globs: &[String],
    scan_root: &Path,
    repo_root: &Path,
    source: DocSource,
    git_reader: Option<&GitReader>,
) -> Result<Vec<PathBuf>> {
    perf::scope_result(
        "discover_files",
        json!({
            "globs": globs,
        }),
        || {
            let prefix = scan_prefix(scan_root, repo_root);
            // Reconstruct the walk base under `repo_root` so that walked paths
            // share its exact prefix and strip cleanly downstream.
            let walk_root = match &prefix {
                Some(p) => repo_root.join(p),
                None => repo_root.to_path_buf(),
            };

            let wiki_ignore = Arc::new(
                crate::wikiignore::WikiIgnore::load(repo_root)
                    .map_err(|e| miette!("{e}"))?,
            );

            let mut files = match source {
                DocSource::Index | DocSource::Head => {
                    if globs.is_empty() {
                        discover_default_files(repo_root, &walk_root, prefix.as_deref(), source, git_reader, &wiki_ignore)?
                    } else {
                        // For non-worktree sources we must never read the
                        // worktree filesystem to satisfy a glob.  Filter the
                        // source's own path list instead so the candidate set
                        // is internally consistent with `--source`.
                        discover_files_by_glob_in_source(
                            globs,
                            repo_root,
                            prefix.as_deref(),
                            source,
                            git_reader,
                            &wiki_ignore,
                        )?
                    }
                }
                DocSource::WorkingTree => {
                    let initial = if globs.is_empty() {
                        discover_default_files(repo_root, &walk_root, prefix.as_deref(), source, git_reader, &wiki_ignore)?
                    } else {
                        Vec::new()
                    };
                    if initial.is_empty() || !globs.is_empty() {
                        discover_files_by_walk(globs, &walk_root, repo_root, Arc::clone(&wiki_ignore))?
                    } else {
                        initial
                    }
                }
            };

            files.sort();
            files.dedup();

            // Empty-corpus is signalled by returning Ok(vec![]) so that callers
            // can distinguish it (by type) from real IO/git failures, which
            // still propagate as Err.  check::run emits the user-facing "no
            // wiki pages found" message and exits 2 in non-fix mode; in fix
            // mode it degrades gracefully so the cleanup pass still runs.

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

/// The path of `scan_root` relative to `repo_root`, or `None` when they are the
/// same directory (the whole repo is in scope). Falls back to a canonicalized
/// comparison so symlinked roots still resolve, and to `None` when `scan_root`
/// is not under `repo_root` (selection then spans the whole repo).
pub(crate) fn scan_prefix(scan_root: &Path, repo_root: &Path) -> Option<PathBuf> {
    let non_empty = |rel: &Path| (!rel.as_os_str().is_empty()).then(|| rel.to_path_buf());
    if let Ok(rel) = scan_root.strip_prefix(repo_root) {
        return non_empty(rel);
    }
    if let (Ok(cs), Ok(cr)) = (
        std::fs::canonicalize(scan_root),
        std::fs::canonicalize(repo_root),
    ) && let Ok(rel) = cs.strip_prefix(&cr)
    {
        return non_empty(rel);
    }
    None
}

/// Whether a repo-relative path lies within the selection `prefix`. A `None`
/// prefix means the whole repo is in scope.
pub(crate) fn path_under_prefix(path_rel: &str, prefix: Option<&Path>) -> bool {
    match prefix {
        None => true,
        Some(pre) => Path::new(path_rel).starts_with(pre),
    }
}

/// Convert a user-supplied (CWD-relative) glob into a repo-root-relative glob
/// string for matching against a `DocSource`'s repo-relative path list.
///
/// Filesystem-absolute globs are stripped to repo-relative; `/`-prefixed globs
/// are already repo-relative; plain relative globs are anchored under the
/// selection `prefix` so they match only the current subtree.
pub(crate) fn glob_to_repo_relative(glob: &str, prefix: Option<&Path>, repo_root: &Path) -> String {
    let path = Path::new(glob);
    if path.is_absolute() {
        return path
            .strip_prefix(repo_root)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| glob.to_string());
    }
    if let Some(rest) = glob.strip_prefix('/') {
        return rest.to_string();
    }
    let trimmed = glob.trim_start_matches("./");
    match prefix {
        Some(pre) => pre.join(trimmed).to_string_lossy().into_owned(),
        None => trimmed.to_string(),
    }
}

fn discover_default_files(
    repo_root: &Path,
    walk_root: &Path,
    prefix: Option<&Path>,
    source: DocSource,
    git_reader: Option<&GitReader>,
    wiki_ignore: &Arc<crate::wikiignore::WikiIgnore>,
) -> Result<Vec<PathBuf>> {
    // For non-worktree sources, seed from the source's own path list so that
    // files absent from the worktree (deleted locally but present in HEAD or
    // the index) are still included in the candidate set. Selection is scoped
    // to `prefix` (the current working directory) by filtering the repo-
    // relative path list.
    match source {
        DocSource::Index | DocSource::Head => {
            let all_paths = if let Some(gr) = git_reader {
                gr.list_paths(source)?
            } else {
                source.list_paths(repo_root)?
            };
            let files: Vec<PathBuf> = all_paths
                .into_iter()
                .filter(|p| {
                    if !p.ends_with(".md")
                        || is_fixture_path(p)
                        || !path_under_prefix(p, prefix)
                        || wiki_ignore.is_ignored(Path::new(p))
                    {
                        return false;
                    }
                    // Include .md files with a frontmatter fence — even if the
                    // YAML is malformed, they are trying to be wiki pages and
                    // callers like check/collect will emit diagnostics for errors.
                    let content = if let Some(gr) = git_reader {
                        gr.read_blob(source, p)
                    } else {
                        source.read(repo_root, p)
                    };
                    match content {
                        Ok(Some(c)) => crate::frontmatter::has_wiki_frontmatter(&c),
                        _ => false,
                    }
                })
                .map(|p| repo_root.join(p))
                .collect();
            return Ok(files);
        }
        DocSource::WorkingTree => {}
    }

    let inventory = match repo_inventory(repo_root) {
        Ok(inventory) => inventory,
        Err(_) => return discover_files_by_walk(&[], walk_root, repo_root, Arc::clone(wiki_ignore)),
    };

    // First pass: cheaply filter the inventory using string predicates only —
    // no filesystem access — into a list of absolute candidate paths.
    let candidates: Vec<PathBuf> = inventory
        .into_iter()
        .filter(|path_rel| {
            path_rel.ends_with(".md")
                && !is_fixture_path(path_rel)
                && path_under_prefix(path_rel, prefix)
                && !wiki_ignore.is_ignored(Path::new(path_rel))
        })
        .map(|path_rel| repo_root.join(&path_rel))
        .collect();

    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    // Second pass: read+check the candidates in parallel. On a hostile
    // (fuseblk) filesystem each `read_to_string` blocks in userspace, so the
    // serial version dominated latency. This mirrors the worker pattern in
    // `ContentCache::warm_working_tree`.
    let parallelism = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let worker_count = parallelism.min(8).min(candidates.len()).max(1);

    let chunk_size = candidates.len().div_ceil(worker_count);
    let mut files: Vec<PathBuf> = Vec::with_capacity(candidates.len());
    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(worker_count);
        for chunk in candidates.chunks(chunk_size) {
            handles.push(scope.spawn(move || {
                let mut out: Vec<PathBuf> = Vec::with_capacity(chunk.len());
                for path in chunk {
                    if path.is_file()
                        && let Ok(content) = std::fs::read_to_string(path)
                        && crate::frontmatter::has_wiki_frontmatter(&content)
                    {
                        out.push(path.clone());
                    }
                }
                out
            }));
        }
        for handle in handles {
            if let Ok(out) = handle.join() {
                files.extend(out);
            }
        }
    });

    Ok(files)
}

/// Test-fixture wiki files (under any `tests/fixtures/` directory) are part of
/// the CLI's own integration suites — they are not part of the repo's authored
/// wiki and must be excluded from default discovery so commands like
/// `wiki check` don't validate them as real pages. Explicit globs that target
/// these paths still match.
fn is_fixture_path(path_rel: &str) -> bool {
    path_rel.contains("/tests/fixtures/") || path_rel.contains("\\tests\\fixtures\\")
}

/// Filter a `DocSource`'s path list against the same glob semantics as
/// `discover_files_by_walk`: globs are normalised to repo-relative form and
/// matched against the source's repo-relative paths.  Used under
/// `--source=index|head` so glob discovery never reads the worktree.
fn discover_files_by_glob_in_source(
    globs: &[String],
    repo_root: &Path,
    prefix: Option<&Path>,
    source: DocSource,
    git_reader: Option<&GitReader>,
    wiki_ignore: &Arc<crate::wikiignore::WikiIgnore>,
) -> Result<Vec<PathBuf>> {
    let mut glob_builder = globset::GlobSetBuilder::new();
    for glob in globs {
        let normalized = glob_to_repo_relative(glob, prefix, repo_root);
        let glob = globset::Glob::new(&normalized)
            .into_diagnostic()
            .wrap_err_with(|| format!("invalid glob pattern: {normalized}"))?;
        glob_builder.add(glob);
    }
    let glob_set = glob_builder
        .build()
        .into_diagnostic()
        .wrap_err("failed to build glob set")?;

    let all_paths = if let Some(gr) = git_reader {
        gr.list_paths(source)?
    } else {
        source.list_paths(repo_root)?
    };
    let mut files = Vec::new();
    for path_rel in all_paths {
        if !path_rel.ends_with(".md") {
            continue;
        }
        if glob_set.is_match(&path_rel) && !wiki_ignore.is_ignored(Path::new(&path_rel)) {
            files.push(repo_root.join(&path_rel));
        }
    }
    files.sort();
    files.dedup();
    Ok(files)
}

fn discover_files_by_walk(
    globs: &[String],
    base_dir: &Path,
    repo_root: &Path,
    wiki_ignore: Arc<crate::wikiignore::WikiIgnore>,
) -> Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = Vec::new();

    let candidates = if globs.is_empty() {
        discover_files_by_parallel_walk(base_dir, &["**/*.md".to_string()], repo_root, Arc::clone(&wiki_ignore))?
    } else {
        // Globs are CWD-relative; the walk matches them against paths relative
        // to `base_dir`, so normalize against the same base.
        let normalized_globs = globs
            .iter()
            .map(|glob| normalize_repo_relative_path(glob, base_dir))
            .collect::<Vec<_>>();
        discover_files_by_parallel_walk(base_dir, &normalized_globs, repo_root, Arc::clone(&wiki_ignore))?
    };

    for path in candidates {
        // For explicit globs, include all .md matches (membership is checked
        // by callers like check/collect). For default walks, require a
        // frontmatter fence so plain .md files (README, CHANGELOG, etc.) are
        // excluded, but pages with malformed YAML still reach the diagnostic
        // loop.
        if globs.is_empty() {
            if let Ok(content) = std::fs::read_to_string(&path)
                && crate::frontmatter::has_wiki_frontmatter(&content)
            {
                files.push(path);
            }
        } else {
            files.push(path);
        }
    }

    files.sort();
    files.dedup();

    Ok(files)
}

fn discover_files_by_parallel_walk(
    base_dir: &Path,
    patterns: &[String],
    repo_root: &Path,
    wiki_ignore: Arc<crate::wikiignore::WikiIgnore>,
) -> Result<Vec<PathBuf>> {
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
            let repo_root = repo_root.to_path_buf();
            let wiki_ignore = Arc::clone(&wiki_ignore);

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

                let repo_relative = path.strip_prefix(&repo_root).unwrap_or(path);
                if wiki_ignore.is_ignored(repo_relative) {
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
    use std::process::Command; // used by TestRepo::git
    use tempfile::TempDir;

    fn make_fm(title: &str) -> Frontmatter {
        Frontmatter {
            title: title.into(),
            aliases: vec![],
            tags: vec![],
            keywords: vec![],
            summary: "A summary.".into(),
            links_reviewed: None,
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

    // ── locate_existing_suffix salvage boundary ──────────────────────────────

    #[test]
    fn locate_existing_suffix_matches_outside_repo_suffix() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_root = tmp.path().join("repo");
        std::fs::create_dir_all(repo_root.join("src")).unwrap();
        std::fs::write(repo_root.join("src/lib.rs"), "x").unwrap();

        // Simulate a link that resolves to a path outside the repo,
        // e.g. `../other-repo/src/lib.rs` from a wiki page at
        // `<repo>/wiki/page.md`. The resolved absolute path is
        // `<tmpdir>/other-repo/src/lib.rs`.
        let outside_path = tmp.path().join("other-repo/src/lib.rs");
        let outside_str = outside_path.to_string_lossy().replace('\\', "/");

        // The suffix `src/lib.rs` exists inside the repo at
        // `<repo_root>/src/lib.rs`. locate_existing_suffix must
        // NOT match it — the original path is completely outside
        // the repo and shares the suffix only by coincidence.
        let result = locate_existing_suffix(&outside_str, &repo_root);

        assert_eq!(
            result,
            None,
            "locate_existing_suffix must not match in-repo files \
             for paths that resolve outside the repo"
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
    fn test_discover_no_md_files_exits_with_no_pages() {
        let repo = TestRepo::new();
        // No .md files at all — discover_files returns Ok(vec![]) for empty corpus.
        let files = discover_files(&[], repo.path(), repo.path(), DocSource::WorkingTree, None).unwrap();
        assert!(
            files.is_empty(),
            "expected empty vec for no wiki pages, got: {files:?}"
        );
    }

    #[test]
    fn test_discover_md_without_frontmatter_not_found() {
        let repo = TestRepo::new();
        repo.create_file("wiki/.gitkeep", "");
        repo.create_file("wiki/plain.md", "# no frontmatter\n");
        // No frontmatter fence → not a wiki candidate → returns Ok(vec![]) not Err.
        let files = discover_files(&[], repo.path(), repo.path(), DocSource::WorkingTree, None).unwrap();
        assert!(
            files.is_empty(),
            "expected empty vec for plain md, got: {files:?}"
        );
    }

    #[test]
    fn test_discover_finds_md_files_with_frontmatter() {
        let repo = TestRepo::new();
        repo.create_file("wiki/page.md", "---\ntitle: Page\nsummary: A page.\n---\n");
        let files = discover_files(&[], repo.path(), repo.path(), DocSource::WorkingTree, None)
            .expect("discover");
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("page.md"));
    }

    #[test]
    fn test_discover_finds_member_md_anywhere_in_repo() {
        let repo = TestRepo::new();
        repo.create_file("wiki/page.md", "---\ntitle: Page\nsummary: A page.\n---\n");
        repo.create_file(
            "src/component/docs.md",
            "---\ntitle: Docs\nsummary: Component docs.\n---\n",
        );
        repo.create_file("src/component/ordinary.md", "# ordinary\n");
        let files = discover_files(&[], repo.path(), repo.path(), DocSource::WorkingTree, None)
            .expect("discover");
        // Only files with a frontmatter fence are discovered. `ordinary.md`
        // has no fence and is excluded.
        assert_eq!(files.len(), 2);
        let paths: Vec<_> = files
            .into_iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        assert!(paths.iter().any(|p| p.ends_with("src/component/docs.md")));
        assert!(paths.iter().any(|p| p.ends_with("wiki/page.md")));
        assert!(!paths.iter().any(|p| p.ends_with("ordinary.md")));
    }

    #[test]
    fn test_discover_explicit_glob_zero_matches_exits_2() {
        let repo = TestRepo::new();
        repo.create_file("wiki/page.md", "---\ntitle: Page\nsummary: A page.\n---\n");
        let globs = vec!["wiki/nonexistent/**/*.md".to_string()];
        // Zero matches returns Ok(vec![]) not Err.
        let files =
            discover_files(&globs, repo.path(), repo.path(), DocSource::WorkingTree, None).unwrap();
        assert!(
            files.is_empty(),
            "expected empty vec for no-match glob, got: {files:?}"
        );
    }

    #[test]
    fn test_discover_explicit_glob_finds_files() {
        let repo = TestRepo::new();
        repo.create_file("docs/page.md", "---\ntitle: Page\nsummary: A page.\n---\n");
        let globs = vec!["docs/**/*.md".to_string()];
        let files = discover_files(&globs, repo.path(), repo.path(), DocSource::WorkingTree, None)
            .expect("explicit glob should succeed");
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("page.md"));
    }

    #[test]
    fn test_discover_explicit_glob_with_dot_slash_prefix() {
        let repo = TestRepo::new();
        repo.create_file("wiki/page.md", "---\ntitle: Page\nsummary: A page.\n---\n");
        let globs = vec!["./wiki/page.md".to_string()];
        let files = discover_files(&globs, repo.path(), repo.path(), DocSource::WorkingTree, None)
            .expect("discover");
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("wiki/page.md"));
    }

    #[test]
    fn test_discover_skips_gitignored_directories() {
        let repo = TestRepo::new();
        repo.create_file("wiki/page.md", "---\ntitle: Page\nsummary: A page.\n---\n");
        // Create a wiki page inside a gitignored directory.
        repo.create_file(
            "ignored-dir/stale.md",
            "---\ntitle: Stale\nsummary: Should be excluded.\n---\n",
        );
        // Gitignore the directory — discover_files must not return files from it.
        repo.create_file(".gitignore", "ignored-dir/\n");
        let files = discover_files(&[], repo.path(), repo.path(), DocSource::WorkingTree, None)
            .expect("discover");
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
        repo.create_file("wiki/page.md", "---\ntitle: Page\nsummary: A page.\n---\n");
        // Simulate a real git worktree: it contains a .git FILE (not directory)
        // pointing back to the main repo's .git/worktrees/... directory.
        repo.create_file(
            ".worktrees/cards/main-265/1/.git",
            "gitdir: /workspace/.git/worktrees/main-265-1\n",
        );
        repo.create_file(
            ".worktrees/cards/main-265/1/documentation/monetezation.md",
            "---\ntitle: compare branch monetization\nsummary: Stale.\n---\n",
        );
        // Gitignore the worktrees directory (as this repo does in production).
        repo.create_file(".gitignore", ".worktrees\n");
        let files = discover_files(&[], repo.path(), repo.path(), DocSource::WorkingTree, None)
            .expect("discover");
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
        repo.create_file("wiki/page.md", "---\ntitle: Page\nsummary: A page.\n---\n");
        repo.create_file(
            "wiki/nested/child.md",
            "---\ntitle: Child\nsummary: Child page.\n---\n",
        );
        repo.create_file(
            "docs/reference.md",
            "---\ntitle: Reference\nsummary: Reference.\n---\n",
        );
        repo.create_file(
            "ignored-dir/stale.md",
            "---\ntitle: Stale\nsummary: Should be excluded.\n---\n",
        );
        repo.create_file(".gitignore", "ignored-dir/\n");
        repo.git(&["add", "-A"]);

        let wiki_ignore = Arc::new(crate::wikiignore::WikiIgnore::load(repo.path()).unwrap());
        let inventory_files = discover_default_files(
            repo.path(),
            repo.path(),
            None,
            DocSource::WorkingTree,
            None,
            &wiki_ignore,
        )
        .expect("inventory discover");
        let walk_files =
            discover_files_by_walk(&[], repo.path(), repo.path(), Arc::clone(&wiki_ignore))
                .expect("walk discover");

        assert_eq!(inventory_files, walk_files);
    }

    #[test]
    fn test_discover_respects_wikiignore_default_walk() {
        let repo = TestRepo::new();
        repo.create_file("wiki/page.md", "---\ntitle: Page\nsummary: A page.\n---\n");
        repo.create_file(
            "wiki/secret.md",
            "---\ntitle: Secret\nsummary: Hidden.\n---\n",
        );
        repo.create_file(".wiki/.wikiignore", "wiki/secret.md\n");
        let files = discover_files(&[], repo.path(), repo.path(), DocSource::WorkingTree, None)
            .expect("discover");
        let paths: Vec<_> = files
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        assert!(paths.iter().any(|p| p.ends_with("wiki/page.md")));
        assert!(
            !paths.iter().any(|p| p.ends_with("wiki/secret.md")),
            "wikiignored file must be excluded, got: {paths:?}"
        );
    }

    #[test]
    fn test_discover_wikiignore_glob_pattern() {
        let repo = TestRepo::new();
        repo.create_file("wiki/page.md", "---\ntitle: Page\nsummary: A page.\n---\n");
        repo.create_file("drafts/a.md", "---\ntitle: A\nsummary: A.\n---\n");
        repo.create_file("drafts/b.md", "---\ntitle: B\nsummary: B.\n---\n");
        repo.create_file(".wiki/.wikiignore", "drafts/*.md\n");
        let files = discover_files(&[], repo.path(), repo.path(), DocSource::WorkingTree, None)
            .expect("discover");
        let paths: Vec<_> = files
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        assert!(paths.iter().any(|p| p.ends_with("wiki/page.md")));
        assert!(
            !paths.iter().any(|p| p.contains("drafts/")),
            "all drafts files must be excluded, got: {paths:?}"
        );
    }

    #[test]
    fn test_discover_wikiignore_applies_to_explicit_glob() {
        let repo = TestRepo::new();
        repo.create_file("drafts/page.md", "---\ntitle: Draft\nsummary: D.\n---\n");
        repo.create_file(".wiki/.wikiignore", "drafts/\n");
        let globs = vec!["drafts/page.md".to_string()];
        let files = discover_files(&globs, repo.path(), repo.path(), DocSource::WorkingTree, None)
            .expect("discover");
        assert!(
            files.is_empty(),
            "explicit glob on a wikiignored file must return nothing, got: {files:?}"
        );
    }

    #[test]
    fn test_discover_wikiignore_absent_is_noop() {
        let repo = TestRepo::new();
        repo.create_file("wiki/page.md", "---\ntitle: Page\nsummary: A page.\n---\n");
        repo.create_file(
            "drafts/a.md",
            "---\ntitle: A\nsummary: A.\n---\n",
        );
        let files = discover_files(&[], repo.path(), repo.path(), DocSource::WorkingTree, None)
            .expect("discover");
        assert_eq!(files.len(), 2, "no .wikiignore → behaviour unchanged");
    }
}
