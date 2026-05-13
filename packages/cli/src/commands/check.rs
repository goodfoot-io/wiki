use std::path::{Path, PathBuf};

use miette::Result;
use serde::Serialize;

use crate::commands::discover_files;
use crate::frontmatter::{Frontmatter, build_index, parse_frontmatter, parse_title};
use crate::git::resolve_ref;
use crate::headings::{extract_headings, resolve_heading};
use crate::index::DocSource;
use crate::parser::{LinkKind, parse_fragment_links};

use super::mesh_coverage;

/// Read `path` from the chosen `DocSource`.
fn read_via_source(path: &Path, repo_root: &Path, source: DocSource) -> std::io::Result<String> {
    match source {
        DocSource::WorkingTree => std::fs::read_to_string(path),
        DocSource::Index | DocSource::Head => {
            let path_rel = path
                .strip_prefix(repo_root)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| path.to_string_lossy().into_owned());
            match source.read(repo_root, &path_rel) {
                Ok(Some(s)) => Ok(s),
                Ok(None) => Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("{path_rel} not present in source {:?}", source),
                )),
                Err(e) => Err(std::io::Error::other(e.to_string())),
            }
        }
    }
}

/// Filter `files` to the candidates the chosen `DocSource` considers present.
fn filter_files_for_source(
    files: Vec<PathBuf>,
    repo_root: &Path,
    source: DocSource,
) -> Result<Vec<PathBuf>> {
    if matches!(source, DocSource::WorkingTree) {
        return Ok(files);
    }
    let listed: std::collections::HashSet<String> =
        source.list_paths(repo_root)?.into_iter().collect();
    Ok(files
        .into_iter()
        .filter(|p| {
            let rel = p
                .strip_prefix(repo_root)
                .map(|r| r.to_string_lossy().into_owned())
                .unwrap_or_else(|_| p.to_string_lossy().into_owned());
            listed.contains(&rel)
        })
        .collect())
}

// ── Diagnostic types ──────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct CheckDiagnostic {
    pub kind: String,
    pub file: String,
    pub line: usize,
    pub message: String,
}

/// Convert a snake_case diagnostic kind to Title Case.
fn kind_title_case(kind: &str) -> String {
    kind.split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(c) => c.to_ascii_uppercase().to_string() + chars.as_str(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Render one diagnostic in the human-readable hook format.
fn format_diagnostic(kind: &str, file: &str, line: usize, message: &str) -> String {
    let mut out = format!("Error: {}\n", kind_title_case(kind));
    if !file.is_empty() {
        out.push_str(&format!("- {file}:{line}\n"));
    }
    out.push_str(&format!("- {message}\n"));
    out.push_str("\n---\n\n");
    out
}

// ── Public entry points ───────────────────────────────────────────────────────

/// Run the check command.
///
/// Returns the exit code: 0 = valid, 1 = validation errors, 2 = runtime error.
pub fn run(
    globs: &[String],
    json: bool,
    repo_root: &Path,
    no_exit_code: bool,
    no_mesh: bool,
    source: DocSource,
) -> Result<i32> {
    let files = match discover_files(globs, repo_root, source) {
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
    let files = match filter_files_for_source(files, repo_root, source) {
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

    let index_files = if globs.is_empty() {
        files.clone()
    } else {
        let raw = discover_files(&[], repo_root, source)?;
        filter_files_for_source(raw, repo_root, source)?
    };

    let diagnostics = match collect_for_files(&files, &index_files, repo_root, no_mesh, source) {
        Ok(d) => d,
        Err(e) => {
            if json {
                eprintln!("{}", serde_json::json!({"error": e.to_string()}));
            } else {
                eprintln!("error: {e}");
            }
            return Ok(2);
        }
    };

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({ "errors": diagnostics })).unwrap()
        );
    } else {
        for d in &diagnostics {
            print!(
                "{}",
                format_diagnostic(&d.kind, &d.file, d.line, &d.message)
            );
        }
    }

    if !diagnostics.is_empty() && !no_exit_code {
        Ok(1)
    } else {
        Ok(0)
    }
}

/// Collect diagnostics for the given glob patterns without printing output.
#[allow(dead_code)]
pub fn collect(globs: &[String], repo_root: &Path) -> Result<Vec<CheckDiagnostic>> {
    collect_with_source(globs, repo_root, DocSource::WorkingTree)
}

/// Collect diagnostics with an explicit `DocSource`.
pub fn collect_with_source(
    globs: &[String],
    repo_root: &Path,
    source: DocSource,
) -> Result<Vec<CheckDiagnostic>> {
    let files = discover_files(globs, repo_root, source)?;
    let files = filter_files_for_source(files, repo_root, source)?;
    let index_files = if globs.is_empty() {
        files.clone()
    } else {
        let raw = discover_files(&[], repo_root, source)?;
        filter_files_for_source(raw, repo_root, source)?
    };
    collect_for_files(&files, &index_files, repo_root, false, source)
}

/// Extract the anchor portion (after `#`) from a markdown link href, if present.
///
/// Returns `None` when the href contains no `#`. Line-range anchors like
/// `L10-L20` are still returned here; callers must check whether the
/// `FragmentLink::start_line` is `Some` to distinguish line ranges from
/// heading slugs.
fn anchor_of(href: &str) -> Option<&str> {
    href.find('#').map(|i| &href[i + 1..])
}

fn collect_for_files(
    files: &[PathBuf],
    index_files: &[PathBuf],
    repo_root: &Path,
    no_mesh: bool,
    source: DocSource,
) -> Result<Vec<CheckDiagnostic>> {
    let mut diagnostics: Vec<CheckDiagnostic> = Vec::new();

    let files_set: std::collections::HashSet<&PathBuf> = files.iter().collect();

    // ── Parse frontmatter for all pages ──────────────────────────────────────
    let mut pages: Vec<(PathBuf, Frontmatter)> = Vec::new();

    for path in index_files {
        let in_scope = files_set.contains(path);
        let content = match read_via_source(path, repo_root, source) {
            Ok(c) => c,
            Err(e) => {
                if in_scope {
                    diagnostics.push(CheckDiagnostic {
                        kind: "runtime".into(),
                        file: path.display().to_string(),
                        line: 0,
                        message: format!("Could not read file: {e}"),
                    });
                }
                continue;
            }
        };

        match parse_frontmatter(&content, path) {
            Ok(Some(fm)) => {
                pages.push((path.clone(), fm));
            }
            Ok(None) => {
                if in_scope {
                    diagnostics.push(CheckDiagnostic {
                        kind: "frontmatter".into(),
                        file: path.display().to_string(),
                        line: 1,
                        message:
                            "Add a `---` frontmatter block. `title` and `summary` are required."
                                .into(),
                    });
                }
            }
            Err(e) => {
                let _ = parse_title(&content);
                if in_scope {
                    diagnostics.push(CheckDiagnostic {
                        kind: "frontmatter".into(),
                        file: path.display().to_string(),
                        line: 1,
                        message: e.to_string(),
                    });
                }
            }
        }
    }

    // ── Title/alias collisions ────────────────────────────────────────────────
    let (_index, collisions) = build_index(&pages);
    for col in &collisions {
        if files_set.contains(&col.offending_path) {
            diagnostics.push(CheckDiagnostic {
                kind: "collision".into(),
                file: col.offending_path.display().to_string(),
                line: 1,
                message: format!(
                    "Title or alias `{}` is already defined in `{}`. Rename this page's title or remove the conflicting alias.",
                    col.key,
                    col.existing_path.display()
                ),
            });
        }
    }

    // ── Validate links in all in-scope files ─────────────────────────────────
    for path in files {
        let content = match read_via_source(path, repo_root, source) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let frag_links = parse_fragment_links(&content);
        for link in &frag_links {
            if link.kind == LinkKind::External {
                continue;
            }
            // Skip non-file URI schemes that snuck past kind detection (mailto, etc.).
            if link.original_href.starts_with("mailto:") {
                continue;
            }

            let resolved = crate::commands::resolve_link_path(&link.path, path, repo_root);
            let abs = repo_root.join(&resolved);

            // Try to read the target. Directories are valid link targets.
            let ref_content = match read_via_source(&abs, repo_root, source) {
                Ok(c) => Some(c),
                Err(_) => {
                    if abs.is_dir() {
                        None
                    } else {
                        // Missing file diagnostic with bare-path hint.
                        let first = Path::new(&link.path).components().next();
                        let is_explicit = matches!(
                            first,
                            Some(std::path::Component::CurDir)
                                | Some(std::path::Component::ParentDir)
                        );
                        let is_bare = !link.path.starts_with('/') && !is_explicit;

                        let message = if is_bare {
                            let repo_relative_abs = repo_root.join(&link.path);
                            if repo_relative_abs.exists() {
                                format!(
                                    "File `{}` not found at page-relative path.\n\
                                     If you meant a repo-relative path, use `/{}` instead.",
                                    link.path, link.path
                                )
                            } else {
                                format!("File `{}` not found.", link.path)
                            }
                        } else {
                            format!("File `{}` not found.", link.path)
                        };
                        diagnostics.push(CheckDiagnostic {
                            kind: "broken_link".into(),
                            file: path.display().to_string(),
                            line: link.source_line,
                            message,
                        });
                        continue;
                    }
                }
            };

            // Anchor validation: line range OR heading slug.
            if let Some(start) = link.start_line {
                if let Some(ref tc) = ref_content {
                    if start == 0 {
                        diagnostics.push(CheckDiagnostic {
                            kind: "broken_anchor".into(),
                            file: path.display().to_string(),
                            line: link.source_line,
                            message: format!(
                                "Line numbers are 1-based. Replace `L0` with `L1` in `{}`.",
                                link.path
                            ),
                        });
                    } else {
                        let line_count = tc.lines().count() as u32;
                        let end = link.end_line.unwrap_or(start);
                        if start > line_count || end > line_count {
                            diagnostics.push(CheckDiagnostic {
                                kind: "broken_anchor".into(),
                                file: path.display().to_string(),
                                line: link.source_line,
                                message: format!(
                                    "Line range `L{start}–L{end}` exceeds `{}` ({line_count} lines).",
                                    link.path
                                ),
                            });
                        } else if start > end {
                            diagnostics.push(CheckDiagnostic {
                                kind: "broken_anchor".into(),
                                file: path.display().to_string(),
                                line: link.source_line,
                                message: format!(
                                    "Line range start (`L{start}`) must not exceed end (`L{end}`) in `{}`.",
                                    link.path
                                ),
                            });
                        }
                    }
                }
            } else if let Some(anchor) = anchor_of(&link.original_href)
                && !anchor.is_empty()
                && let Some(ref tc) = ref_content
            {
                // Non-line-range anchor: validate as heading slug.
                let headings = extract_headings(tc);
                if !resolve_heading(anchor, &headings) {
                    diagnostics.push(CheckDiagnostic {
                        kind: "broken_anchor".into(),
                        file: path.display().to_string(),
                        line: link.source_line,
                        message: format!("Heading `#{anchor}` not found in `{}`.", link.path),
                    });
                }
            }
        }
    }

    // Soft check that git is callable.
    let _ = resolve_ref(repo_root, "HEAD");

    // ── Mesh coverage pass ────────────────────────────────────────────────────
    if !no_mesh && matches!(source, DocSource::WorkingTree) {
        let mesh_diags = mesh_coverage::collect_mesh_diagnostics(files, repo_root)?;
        diagnostics.extend(mesh_diags);
    }

    Ok(diagnostics)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use std::sync::Mutex;
    use tempfile::TempDir;

    /// Serialize tests that read or write PATH for `git-mesh` resolution.
    static PATH_MUTEX: Mutex<()> = Mutex::new(());

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

        fn git_mesh(&self, args: &[&str]) {
            let output = Command::new("git-mesh")
                .current_dir(self.dir.path())
                .args(args)
                .env("GIT_AUTHOR_NAME", "Test Author")
                .env("GIT_AUTHOR_EMAIL", "test@example.com")
                .env("GIT_COMMITTER_NAME", "Test Committer")
                .env("GIT_COMMITTER_EMAIL", "test@example.com")
                .output()
                .expect("spawn git-mesh");
            assert!(
                output.status.success(),
                "git-mesh {:?} failed:\nstdout: {}\nstderr: {}",
                args,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    fn make_wiki_page(title: &str, body: &str) -> String {
        format!("---\ntitle: {title}\nsummary: A page about {title}.\n---\n{body}")
    }

    #[test]
    fn valid_pages_exit_0() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file("wiki/page.md", &make_wiki_page("Page", "No links here."));
        repo.commit("add page");

        let code = run(
            &[],
            false,
            repo.path(),
            false,
            false,
            crate::index::DocSource::WorkingTree,
        )
        .expect("run");
        assert_eq!(code, 0);
    }

    #[test]
    fn title_collision_exit_1() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file("wiki/a.md", &make_wiki_page("Shared", ""));
        repo.create_file("wiki/b.md", &make_wiki_page("Shared", ""));
        repo.commit("add pages");

        let code = run(
            &[],
            false,
            repo.path(),
            false,
            false,
            crate::index::DocSource::WorkingTree,
        )
        .expect("run");
        assert_eq!(code, 1);
    }

    #[test]
    fn file_without_frontmatter_is_not_discovered() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file("wiki/page.md", "# Just a heading\n\nNo frontmatter.");
        repo.commit("add page");

        // File has no frontmatter → not a wiki member → not discovered → exit 2
        let code = run(
            &[],
            false,
            repo.path(),
            false,
            false,
            crate::index::DocSource::WorkingTree,
        )
        .expect("run");
        assert_eq!(code, 2);
    }

    #[test]
    fn missing_link_target_emits_broken_link() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [missing](/src/missing.rs)."),
        );
        repo.commit("add page");

        let diags = collect(&[], repo.path()).expect("collect");
        assert!(
            diags.iter().any(|d| d.kind == "broken_link"),
            "expected broken_link: {diags:?}"
        );
    }

    #[test]
    fn line_range_out_of_bounds_emits_broken_anchor() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file("src/code.rs", "fn a() {}\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [code](/src/code.rs#L100-L200)."),
        );
        repo.commit("add files");

        let diags = collect(&[], repo.path()).expect("collect");
        assert!(
            diags.iter().any(|d| d.kind == "broken_anchor"),
            "expected broken_anchor: {diags:?}"
        );
    }

    #[test]
    fn heading_anchor_resolves_when_present() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file(
            "wiki/target.md",
            &make_wiki_page("Target", "## Introduction\n\nbody\n"),
        );
        repo.create_file(
            "wiki/source.md",
            &make_wiki_page("Source", "See [intro](target.md#introduction)."),
        );
        repo.commit("add pages");

        let diags = collect(&[], repo.path()).expect("collect");
        assert!(
            diags.iter().all(|d| d.kind != "broken_anchor"),
            "anchor must resolve via slug: {diags:?}"
        );
    }

    #[test]
    fn missing_heading_anchor_emits_broken_anchor() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file(
            "wiki/target.md",
            &make_wiki_page("Target", "## Introduction\n"),
        );
        repo.create_file(
            "wiki/source.md",
            &make_wiki_page("Source", "See [intro](target.md#nonexistent)."),
        );
        repo.commit("add pages");

        let diags = collect(&[], repo.path()).expect("collect");
        assert!(
            diags.iter().any(|d| d.kind == "broken_anchor"),
            "expected broken_anchor: {diags:?}"
        );
    }

    #[test]
    fn directory_link_is_valid() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file("src/lib.rs", "fn main() {}");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [src](/src/) for details."),
        );
        repo.commit("add files");

        let code = run(
            &[],
            false,
            repo.path(),
            false,
            false,
            crate::index::DocSource::WorkingTree,
        )
        .expect("run");
        assert_eq!(code, 0);
    }

    #[test]
    fn mailto_link_does_not_produce_broken_link() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "Contact [us](mailto:someone@example.com)."),
        );
        repo.commit("add files");

        let diagnostics = collect(&[], repo.path()).expect("collect");
        assert!(
            diagnostics.iter().all(|d| d.kind != "broken_link"),
            "mailto: links must not produce broken_link: {diagnostics:?}"
        );
    }

    // ── Mesh coverage tests ───────────────────────────────────────────────────

    #[test]
    fn mesh_uncovered_link_exits_1() {
        let _guard = PATH_MUTEX.lock().expect("path mutex");
        let repo = TestRepo::new();
        repo.create_file("src/code.rs", "fn a() {}\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [code](/src/code.rs#L1-L1)."),
        );
        repo.commit("add files");

        let diagnostics = collect(&[], repo.path()).expect("collect");
        let mesh_diags: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.kind == "mesh_uncovered")
            .collect();
        assert_eq!(
            mesh_diags.len(),
            1,
            "expected one mesh_uncovered: {diagnostics:?}"
        );
        let code = run(
            &[],
            false,
            repo.path(),
            false,
            false,
            crate::index::DocSource::WorkingTree,
        )
        .expect("run");
        assert_eq!(code, 1);
    }

    #[test]
    fn mesh_covered_link_exits_0() {
        let _guard = PATH_MUTEX.lock().expect("path mutex");
        let repo = TestRepo::new();
        repo.create_file("src/code.rs", "fn a() {}\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [code](/src/code.rs#L1-L1)."),
        );
        repo.commit("add files");

        repo.git_mesh(&["add", "test-mesh", "wiki/page.md", "src/code.rs#L1-L1"]);
        repo.git_mesh(&["why", "test-mesh", "-m", "Links wiki page to code."]);
        repo.git_mesh(&["commit"]);

        let diagnostics = collect(&[], repo.path()).expect("collect");
        let mesh_diags: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.kind == "mesh_uncovered")
            .collect();
        assert!(
            mesh_diags.is_empty(),
            "covered link must not produce mesh_uncovered: {diagnostics:?}"
        );
        let code = run(
            &[],
            false,
            repo.path(),
            false,
            false,
            crate::index::DocSource::WorkingTree,
        )
        .expect("run");
        assert_eq!(code, 0);
    }

    #[test]
    fn mesh_skips_links_without_line_range() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file("src/code.rs", "fn a() {}\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [code](/src/code.rs)."),
        );
        repo.commit("add files");

        let diagnostics = collect(&[], repo.path()).expect("collect");
        let mesh_diags: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.kind == "mesh_uncovered")
            .collect();
        assert!(
            mesh_diags.is_empty(),
            "links without line range must not produce mesh_uncovered: {diagnostics:?}"
        );
    }

    /// `--source=index` must validate staged content; broken anchor staged but
    /// worktree clean → must report from index.
    #[test]
    fn source_index_validates_staged_broken_when_worktree_clean() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file("src/code.rs", "fn a() {}\n");
        repo.create_file("wiki/page.md", &make_wiki_page("Page", "No links."));
        repo.commit("clean baseline");

        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [code](/src/code.rs#L100-L100)."),
        );
        repo.git(&["add", "wiki/page.md"]);
        repo.create_file("wiki/page.md", &make_wiki_page("Page", "No links."));

        let diags_wt = collect_with_source(&[], repo.path(), crate::index::DocSource::WorkingTree)
            .expect("collect wt");
        assert!(
            diags_wt.is_empty(),
            "worktree should be clean, got: {:?}",
            diags_wt
        );

        let diags_idx = collect_with_source(&[], repo.path(), crate::index::DocSource::Index)
            .expect("collect idx");
        assert!(
            diags_idx.iter().any(|d| d.kind == "broken_anchor"),
            "index should see staged broken anchor, got: {:?}",
            diags_idx
        );
    }
}
