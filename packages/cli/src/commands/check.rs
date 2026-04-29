use std::collections::HashMap;
use std::path::{Path, PathBuf};

use miette::Result;
use serde::Serialize;

use crate::commands::discover_files;
use crate::frontmatter::{Frontmatter, build_index, parse_frontmatter, parse_title};
use crate::git::resolve_ref;
use crate::headings::extract_headings;
use crate::parser::{LinkKind, parse_fragment_links, parse_wikilinks};

use super::mesh_coverage;

// ── Diagnostic types ──────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct CheckDiagnostic {
    pub kind: String,
    pub file: String,
    pub line: usize,
    pub message: String,
}

// ── Public entry points ───────────────────────────────────────────────────────

/// Run the check command.
///
/// Returns the exit code: 0 = valid, 1 = validation errors, 2 = runtime error.
pub fn run(globs: &[String], json: bool, mesh: bool, repo_root: &Path) -> Result<i32> {
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

    let index_files = if globs.is_empty() {
        files.clone()
    } else {
        discover_files(&[], repo_root).unwrap_or_else(|_| files.clone())
    };

    let diagnostics = match collect_for_files(&files, &index_files, mesh, repo_root) {
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
        println!("{}", serde_json::to_string_pretty(&diagnostics).unwrap());
    } else {
        for d in &diagnostics {
            println!("**{}** — `{}:{}`\n{}\n", d.kind, d.file, d.line, d.message);
        }
    }

    if diagnostics.iter().any(|d| d.kind != "alias_resolve") {
        Ok(1)
    } else {
        Ok(0)
    }
}

/// Collect diagnostics for the given glob patterns without printing output.
///
/// Returns `Err` only on discovery failure; validation errors are returned as
/// diagnostics.  On discovery failure the caller should treat this as exit
/// code 2.
pub fn collect(globs: &[String], mesh: bool, repo_root: &Path) -> Result<Vec<CheckDiagnostic>> {
    let files = discover_files(globs, repo_root)?;
    let index_files = if globs.is_empty() {
        files.clone()
    } else {
        discover_files(&[], repo_root).unwrap_or_else(|_| files.clone())
    };
    collect_for_files(&files, &index_files, mesh, repo_root)
}

fn collect_for_files(
    files: &[PathBuf],
    index_files: &[PathBuf],
    mesh: bool,
    repo_root: &Path,
) -> Result<Vec<CheckDiagnostic>> {
    let mut diagnostics: Vec<CheckDiagnostic> = Vec::new();

    let files_set: std::collections::HashSet<&PathBuf> = files.iter().collect();

    // ── Parse frontmatter for all pages ──────────────────────────────────────
    let mut pages: Vec<(PathBuf, Frontmatter)> = Vec::new();
    // Titles of pages that failed full validation — used to suppress spurious
    // broken_wikilink diagnostics when the real problem is a frontmatter error.
    let mut invalid_titles: std::collections::HashSet<String> = std::collections::HashSet::new();

    for path in index_files {
        let in_scope = files_set.contains(path);
        let content = match std::fs::read_to_string(path) {
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
                if let Some(title) = parse_title(&content) {
                    invalid_titles.insert(title.to_lowercase());
                }
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

    // ── Build title/alias index and report collisions ─────────────────────────
    let (index, collisions) = build_index(&pages);

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

    // Build a map from path -> content (for heading extraction)
    let mut content_cache: HashMap<PathBuf, String> = HashMap::new();
    for (path, _) in &pages {
        if let Ok(c) = std::fs::read_to_string(path) {
            content_cache.insert(path.clone(), c);
        }
    }

    // ── Validate links in all files (including ones that failed frontmatter) ──
    for path in files {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue, // already reported above
        };

        // Fragment links — validate path existence and line range bounds
        let frag_links = parse_fragment_links(&content);
        for link in &frag_links {
            if link.kind == LinkKind::External {
                continue;
            }
            let resolved = crate::commands::resolve_link_path(&link.path, path, repo_root);
            let abs = repo_root.join(&resolved);
            match std::fs::read_to_string(&abs) {
                Err(_) => {
                    if abs.is_dir() {
                        continue;
                    }
                    diagnostics.push(CheckDiagnostic {
                        kind: "missing_file".into(),
                        file: path.display().to_string(),
                        line: link.source_line,
                        message: format!(
                            "File `{}` not found. Check the path or update the link.",
                            link.path
                        ),
                    });
                }
                Ok(ref_content) => {
                    if let Some(start) = link.start_line {
                        if start == 0 {
                            diagnostics.push(CheckDiagnostic {
                                kind: "line_range".into(),
                                file: path.display().to_string(),
                                line: link.source_line,
                                message: format!(
                                    "Line numbers are 1-based. Replace `L0` with `L1` in `{}`.",
                                    link.path
                                ),
                            });
                        } else {
                            let line_count = ref_content.lines().count() as u32;
                            let end = link.end_line.unwrap_or(start);
                            if start > line_count || end > line_count {
                                diagnostics.push(CheckDiagnostic {
                                    kind: "line_range".into(),
                                    file: path.display().to_string(),
                                    line: link.source_line,
                                    message: format!(
                                        "Line range `L{start}–L{end}` exceeds `{}` ({line_count} lines).",
                                        link.path
                                    ),
                                });
                            } else if start > end {
                                diagnostics.push(CheckDiagnostic {
                                    kind: "line_range".into(),
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
                }
            }
        }

        // Wikilinks
        let wiki_links = parse_wikilinks(&content);
        for wl in &wiki_links {
            let key = wl.title.to_lowercase();
            match index.get(&key) {
                None => {
                    if !invalid_titles.contains(&key) {
                        diagnostics.push(CheckDiagnostic {
                            kind: "broken_wikilink".into(),
                            file: path.display().to_string(),
                            line: wl.source_line,
                            message: format!(
                                "No page has title or alias `{}`. Check the spelling or create the page.",
                                wl.title
                            ),
                        });
                    }
                }
                Some(target_path) => {
                    // Warn if resolved via alias (title differs from key)
                    // Check if resolved via alias: look if any page with this path has a
                    // title that lowercases to `key`
                    let resolved_by_title = pages
                        .iter()
                        .any(|(p, fm)| p == target_path && fm.title.to_lowercase() == key);
                    if !resolved_by_title {
                        diagnostics.push(CheckDiagnostic {
                            kind: "alias_resolve".into(),
                            file: path.display().to_string(),
                            line: wl.source_line,
                            message: format!(
                                "Wikilink `[[{}]]` resolved via alias to `{}`. Use the canonical title to suppress this warning.",
                                wl.title,
                                target_path.display()
                            ),
                        });
                    }

                    // Verify heading fragment if present
                    if let Some(heading_frag) = &wl.heading {
                        let target_content = content_cache.get(target_path);
                        if let Some(tc) = target_content {
                            let headings = extract_headings(tc);
                            if !crate::headings::resolve_heading(heading_frag, &headings) {
                                diagnostics.push(CheckDiagnostic {
                                    kind: "missing_heading".into(),
                                    file: path.display().to_string(),
                                    line: wl.source_line,
                                    message: format!(
                                        "Heading `#{heading_frag}` not found in `{}`. Check that the heading exists and the slug is correct.",
                                        target_path.display()
                                    ),
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    // ── Resolve ref to check git is callable (soft check, non-fatal) ─────────
    let _ = resolve_ref(repo_root, "HEAD");

    // ── Mesh coverage pass (opt-in) ───────────────────────────────────────────
    if mesh {
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

    /// Serialize all tests that read or write PATH for `git-mesh` resolution.
    /// Cargo's default harness is multi-threaded; without serialization, one test
    /// stripping git-mesh from PATH races with another test that needs it.
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

        /// Run `git-mesh <args>` in the test repo.
        ///
        /// Panics if git-mesh exits non-zero.
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

        /// Install a counting shim for `git-mesh` in a temp directory prepended to PATH.
        ///
        /// The shim records each invocation to a counter file and then delegates to the
        /// real `git-mesh`. Returns the temp dir (must be kept alive) and the path to the
        /// counter file.
        fn install_counting_shim(&self) -> (tempfile::TempDir, std::path::PathBuf) {
            let shim_dir = tempfile::TempDir::new().expect("shim tempdir");
            let shim_path = shim_dir.path().join("git-mesh");
            let counter_path = shim_dir.path().join("count");
            let real_git_mesh = Command::new("which")
                .arg("git-mesh")
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .expect("git-mesh must be installed to run caching test");
            // Write a shell script that increments a counter and then delegates.
            let script = format!(
                "#!/bin/sh\nCOUNTER=\"{}\"\nCURRENT=$(cat \"$COUNTER\" 2>/dev/null || echo 0)\necho $((CURRENT + 1)) > \"$COUNTER\"\nexec {} \"$@\"\n",
                counter_path.display(),
                real_git_mesh
            );
            fs::write(&shim_path, &script).expect("write shim");
            // Make shim executable
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&shim_path, fs::Permissions::from_mode(0o755))
                    .expect("chmod shim");
            }
            (shim_dir, counter_path)
        }
    }

    fn make_wiki_page(title: &str, body: &str) -> String {
        format!("---\ntitle: {title}\nsummary: A page about {title}.\n---\n{body}")
    }

    #[test]
    fn test_check_valid_pages_exit_0() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file("wiki/page.md", &make_wiki_page("Page", "No links here."));
        repo.commit("add page");

        let code = run(&[], false, false, repo.path()).expect("run");
        assert_eq!(code, 0);
    }

    #[test]
    fn test_check_broken_wikilink_exit_1() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [[Nonexistent Page]]."),
        );
        repo.commit("add page");

        let code = run(&[], false, false, repo.path()).expect("run");
        assert_eq!(code, 1);
    }

    #[test]
    fn test_check_title_collision_exit_1() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file("wiki/a.md", &make_wiki_page("Shared", ""));
        repo.create_file("wiki/b.md", &make_wiki_page("Shared", ""));
        repo.commit("add pages");

        let code = run(&[], false, false, repo.path()).expect("run");
        assert_eq!(code, 1);
    }

    #[test]
    fn test_check_missing_frontmatter_exit_1() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file("wiki/page.md", "# Just a heading\n\nNo frontmatter.");
        repo.commit("add page");

        let code = run(&[], false, false, repo.path()).expect("run");
        assert_eq!(code, 1);
    }

    #[test]
    fn test_check_wikilink_via_alias_warns_exit_0() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file(
            "wiki/target.md",
            "---\ntitle: Target Page\naliases:\n  - tp\nsummary: The target page.\n---\n",
        );
        repo.create_file("wiki/source.md", &make_wiki_page("Source", "See [[tp]]."));
        repo.commit("add pages");

        let code = run(&[], false, false, repo.path()).expect("run");
        // alias_resolve warnings should not cause exit 1
        assert_eq!(code, 0);
    }

    #[test]
    fn test_check_heading_fragment_not_found_exit_1() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file(
            "wiki/target.md",
            &make_wiki_page("Target", "## Introduction\n"),
        );
        repo.create_file(
            "wiki/source.md",
            &make_wiki_page("Source", "See [[Target#Nonexistent]]."),
        );
        repo.commit("add pages");

        let code = run(&[], false, false, repo.path()).expect("run");
        assert_eq!(code, 1);
    }

    #[test]
    fn test_check_heading_fragment_found_exit_0() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file(
            "wiki/target.md",
            &make_wiki_page("Target", "## Introduction\n"),
        );
        repo.create_file(
            "wiki/source.md",
            &make_wiki_page("Source", "See [[Target#Introduction]]."),
        );
        repo.commit("add pages");

        let code = run(&[], false, false, repo.path()).expect("run");
        assert_eq!(code, 0);
    }

    #[test]
    fn test_check_glob_resolves_wikilinks_against_full_index() {
        // Regression: passing a file path must not limit the index to that file only.
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file(
            "wiki/page_a.md",
            &make_wiki_page("Page A", "See [[Page B]]."),
        );
        repo.create_file("wiki/page_b.md", &make_wiki_page("Page B", "Target."));
        repo.commit("add pages");

        let globs = vec!["wiki/page_a.md".to_string()];
        let code = run(&globs, false, false, repo.path()).expect("run");
        assert_eq!(
            code, 0,
            "wikilink to a page outside the glob must resolve against the full wiki index"
        );
    }

    #[test]
    fn test_check_glob_still_reports_genuinely_missing_wikilinks() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file(
            "wiki/page_a.md",
            &make_wiki_page("Page A", "See [[Does Not Exist]]."),
        );
        repo.create_file("wiki/page_b.md", &make_wiki_page("Page B", "Unrelated."));
        repo.commit("add pages");

        let globs = vec!["wiki/page_a.md".to_string()];
        let code = run(&globs, false, false, repo.path()).expect("run");
        assert_eq!(
            code, 1,
            "a truly missing wikilink must still be reported when using a file glob"
        );
    }

    #[test]
    fn test_check_directory_link_is_valid() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file("src/lib.rs", "fn main() {}");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [src](src/) for details."),
        );
        repo.commit("add files");

        let code = run(&[], false, false, repo.path()).expect("run");
        assert_eq!(
            code, 0,
            "directory fragment links must not produce missing_file"
        );
    }

    #[test]
    fn test_check_glob_does_not_report_collisions_outside_scope() {
        // Collisions between pages not in the glob must not appear in the output.
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file("wiki/a.md", &make_wiki_page("Shared Title", ""));
        repo.create_file("wiki/b.md", &make_wiki_page("Shared Title", ""));
        repo.create_file("wiki/c.md", &make_wiki_page("Clean", "No issues here."));
        repo.commit("add pages");

        let globs = vec!["wiki/c.md".to_string()];
        let diagnostics = collect(&globs, false, repo.path()).expect("collect");
        assert!(
            diagnostics.is_empty(),
            "collision between out-of-scope pages must not appear when checking an unrelated file: {diagnostics:?}"
        );
    }

    // ── Mesh coverage tests ───────────────────────────────────────────────────

    #[test]
    fn mesh_flag_off_does_not_check_coverage() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file("src/code.rs", "fn a() {}\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [code](src/code.rs#L1-L1)."),
        );
        repo.commit("add files");

        // mesh=false: no coverage check, exit 0
        let code = run(&[], false, false, repo.path()).expect("run");
        assert_eq!(code, 0, "mesh flag off must not check coverage");
    }

    #[test]
    fn mesh_uncovered_link_exits_1() {
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        let _guard = PATH_MUTEX.lock().expect("path mutex");
        let repo = TestRepo::new();
        repo.create_file("src/code.rs", "fn a() {}\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [code](src/code.rs#L1-L1)."),
        );
        repo.commit("add files");

        // No mesh created — link is uncovered
        let diagnostics = collect(&[], true, repo.path()).expect("collect");
        let mesh_diags: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.kind == "mesh_uncovered")
            .collect();
        assert_eq!(mesh_diags.len(), 1, "expected one mesh_uncovered: {diagnostics:?}");
        let code = run(&[], false, true, repo.path()).expect("run");
        assert_eq!(code, 1);
    }

    #[test]
    fn mesh_covered_link_exits_0() {
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        let _guard = PATH_MUTEX.lock().expect("path mutex");
        let repo = TestRepo::new();
        repo.create_file("src/code.rs", "fn a() {}\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [code](src/code.rs#L1-L1)."),
        );
        repo.commit("add files");

        // Create a mesh that anchors both the wiki file and the code file
        repo.git_mesh(&["add", "test-mesh", "wiki/page.md", "src/code.rs#L1-L1"]);
        repo.git_mesh(&["why", "test-mesh", "-m", "Links wiki page to code."]);
        repo.git_mesh(&["commit"]);

        let diagnostics = collect(&[], true, repo.path()).expect("collect");
        let mesh_diags: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.kind == "mesh_uncovered")
            .collect();
        assert!(
            mesh_diags.is_empty(),
            "covered link must not produce mesh_uncovered: {diagnostics:?}"
        );
        let code = run(&[], false, true, repo.path()).expect("run");
        assert_eq!(code, 0);
    }

    #[test]
    fn mesh_covers_code_but_not_wiki_file() {
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        let _guard = PATH_MUTEX.lock().expect("path mutex");
        let repo = TestRepo::new();
        repo.create_file("src/code.rs", "fn a() {}\n");
        repo.create_file("src/other.rs", "fn b() {}\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [code](src/code.rs#L1-L1)."),
        );
        repo.commit("add files");

        // Mesh anchors code file and a different file — not the wiki page
        repo.git_mesh(&["add", "test-mesh", "src/other.rs", "src/code.rs#L1-L1"]);
        repo.git_mesh(&["why", "test-mesh", "-m", "Code only mesh."]);
        repo.git_mesh(&["commit"]);

        let diagnostics = collect(&[], true, repo.path()).expect("collect");
        let mesh_diags: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.kind == "mesh_uncovered")
            .collect();
        assert_eq!(
            mesh_diags.len(),
            1,
            "mesh not anchoring wiki file must emit mesh_uncovered: {diagnostics:?}"
        );
    }

    #[test]
    fn mesh_whole_file_code_anchor_covers_ranged_link() {
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        let _guard = PATH_MUTEX.lock().expect("path mutex");
        let repo = TestRepo::new();
        repo.create_file("src/code.rs", "fn a() {}\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [code](src/code.rs#L1-L1)."),
        );
        repo.commit("add files");

        // Whole-file anchor on code.rs should match any ranged query against it
        repo.git_mesh(&["add", "test-mesh", "wiki/page.md", "src/code.rs"]);
        repo.git_mesh(&["why", "test-mesh", "-m", "Whole-file anchor."]);
        repo.git_mesh(&["commit"]);

        let diagnostics = collect(&[], true, repo.path()).expect("collect");
        let mesh_diags: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.kind == "mesh_uncovered")
            .collect();
        assert!(
            mesh_diags.is_empty(),
            "whole-file anchor must cover ranged link: {diagnostics:?}"
        );
    }

    #[test]
    fn mesh_range_outside_link_does_not_cover() {
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        let _guard = PATH_MUTEX.lock().expect("path mutex");
        let repo = TestRepo::new();
        // Create a file with at least 20 lines
        let content: String = (1..=20).map(|i| format!("fn line_{i}() {{}}\n")).collect();
        repo.create_file("src/code.rs", &content);
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [code](src/code.rs#L1-L1)."),
        );
        repo.commit("add files");

        // Mesh covers L10-L20, but link is L1-L1 — should NOT cover
        repo.git_mesh(&["add", "test-mesh", "wiki/page.md", "src/code.rs#L10-L20"]);
        repo.git_mesh(&["why", "test-mesh", "-m", "Different range."]);
        repo.git_mesh(&["commit"]);

        let diagnostics = collect(&[], true, repo.path()).expect("collect");
        let mesh_diags: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.kind == "mesh_uncovered")
            .collect();
        assert_eq!(
            mesh_diags.len(),
            1,
            "mesh with non-overlapping range must not cover link: {diagnostics:?}"
        );
    }

    #[test]
    fn mesh_skips_links_without_line_range() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file("src/code.rs", "fn a() {}\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [code](src/code.rs)."),
        );
        repo.commit("add files");

        // No mesh — but link has no range so it should be skipped
        let diagnostics = collect(&[], true, repo.path()).expect("collect");
        let mesh_diags: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.kind == "mesh_uncovered")
            .collect();
        assert!(
            mesh_diags.is_empty(),
            "links without line range must not produce mesh_uncovered: {diagnostics:?}"
        );
    }

    #[test]
    fn mesh_skips_external_links() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [external](https://example.com/file.rs#L1-L5)."),
        );
        repo.commit("add files");

        let diagnostics = collect(&[], true, repo.path()).expect("collect");
        let mesh_diags: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.kind == "mesh_uncovered")
            .collect();
        assert!(
            mesh_diags.is_empty(),
            "external links must not produce mesh_uncovered: {diagnostics:?}"
        );
    }

    #[test]
    fn mesh_unavailable_emits_warning_and_exits_1() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file("src/code.rs", "fn a() {}\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [code](src/code.rs#L1-L1)."),
        );
        repo.commit("add files");

        // Hold the PATH mutex for the entire test to prevent races with other
        // tests that resolve git-mesh from PATH.
        let _guard = PATH_MUTEX.lock().expect("path mutex");

        let shim_dir = tempfile::TempDir::new().expect("shim tempdir");
        let original_path = std::env::var("PATH").unwrap_or_default();

        {
            let filtered_path: String = original_path
                .split(':')
                .filter(|dir| {
                    let gm = std::path::Path::new(dir).join("git-mesh");
                    !gm.exists()
                })
                .collect::<Vec<_>>()
                .join(":");
            let test_path = format!("{}:{}", shim_dir.path().display(), filtered_path);

            // SAFETY: PATH_MUTEX is held; no other test reads/writes PATH concurrently.
            unsafe { std::env::set_var("PATH", &test_path) };
            let result = collect(&[], true, repo.path());
            // Restore PATH before asserting so failures don't leak state.
            // SAFETY: PATH_MUTEX is held.
            unsafe { std::env::set_var("PATH", &original_path) };

            let diagnostics = result.expect("collect");
            let unavailable: Vec<_> = diagnostics
                .iter()
                .filter(|d| d.kind == "mesh_unavailable")
                .collect();
            assert_eq!(
                unavailable.len(),
                1,
                "missing git-mesh must emit exactly one mesh_unavailable: {diagnostics:?}"
            );
            let uncovered: Vec<_> = diagnostics
                .iter()
                .filter(|d| d.kind == "mesh_uncovered")
                .collect();
            assert!(
                uncovered.is_empty(),
                "mesh_unavailable must prevent mesh_uncovered diagnostics: {diagnostics:?}"
            );
        }

        let code = {
            let filtered_path: String = original_path
                .split(':')
                .filter(|dir| {
                    let gm = std::path::Path::new(dir).join("git-mesh");
                    !gm.exists()
                })
                .collect::<Vec<_>>()
                .join(":");
            // SAFETY: PATH_MUTEX is held.
            unsafe { std::env::set_var("PATH", &filtered_path) };
            let code = run(&[], false, true, repo.path()).expect("run");
            // SAFETY: PATH_MUTEX is held.
            unsafe { std::env::set_var("PATH", &original_path) };
            code
        };
        assert_eq!(code, 1, "mesh_unavailable must cause exit 1 (fail closed)");
    }

    #[test]
    fn mesh_caches_per_anchor() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file("src/code.rs", "fn a() {}\n");
        // Two wiki pages that both link to the same code anchor
        repo.create_file(
            "wiki/page_a.md",
            &make_wiki_page("Page A", "See [code](src/code.rs#L1-L1)."),
        );
        repo.create_file(
            "wiki/page_b.md",
            &make_wiki_page("Page B", "See [code](src/code.rs#L1-L1)."),
        );
        repo.commit("add files");

        let (shim_dir, counter_path) = repo.install_counting_shim();

        // Hold the PATH mutex for the entire test.
        let _guard = PATH_MUTEX.lock().expect("path mutex");

        let original_path = std::env::var("PATH").unwrap_or_default();
        let shim_path_str = shim_dir.path().display().to_string();
        // Filter out real git-mesh from PATH so only our shim is used
        let filtered_path: String = original_path
            .split(':')
            .filter(|dir| {
                let gm = std::path::Path::new(dir).join("git-mesh");
                !gm.exists()
            })
            .collect::<Vec<_>>()
            .join(":");
        let new_path = format!("{shim_path_str}:{filtered_path}");
        // SAFETY: PATH_MUTEX is held; no other test reads/writes PATH concurrently.
        unsafe { std::env::set_var("PATH", &new_path) };

        let _diagnostics = collect(&[], true, repo.path()).expect("collect");

        // SAFETY: PATH_MUTEX is held.
        unsafe { std::env::set_var("PATH", &original_path) };

        // Read the counter — git-mesh should have been called exactly once for the shared anchor
        let count_str = fs::read_to_string(&counter_path).unwrap_or_else(|_| "0".to_string());
        let count: u32 = count_str.trim().parse().unwrap_or(0);
        assert_eq!(
            count, 1,
            "git-mesh ls must be called exactly once for a shared (target, range) anchor, got {count}"
        );
    }

    #[test]
    fn mesh_runtime_error_exits_2() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file("src/code.rs", "fn a() {}\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [code](src/code.rs#L1-L1)."),
        );
        repo.commit("add files");

        // Install a shim that always exits 128 with a fatal-looking stderr message.
        let shim_dir = tempfile::TempDir::new().expect("shim tempdir");
        let shim_path = shim_dir.path().join("git-mesh");
        let script = "#!/bin/sh\necho 'fatal: not a git repo' >&2\nexit 128\n";
        fs::write(&shim_path, script).expect("write shim");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&shim_path, fs::Permissions::from_mode(0o755))
                .expect("chmod shim");
        }

        // Hold the PATH mutex for the entire test.
        let _guard = PATH_MUTEX.lock().expect("path mutex");

        let original_path = std::env::var("PATH").unwrap_or_default();
        let shim_path_str = shim_dir.path().display().to_string();
        let filtered_path: String = original_path
            .split(':')
            .filter(|dir| {
                let gm = std::path::Path::new(dir).join("git-mesh");
                !gm.exists()
            })
            .collect::<Vec<_>>()
            .join(":");
        let new_path = format!("{shim_path_str}:{filtered_path}");

        // SAFETY: PATH_MUTEX is held; no other test reads/writes PATH concurrently.
        unsafe { std::env::set_var("PATH", &new_path) };
        let code = run(&[], false, true, repo.path()).expect("run");
        // SAFETY: PATH_MUTEX is held.
        unsafe { std::env::set_var("PATH", &original_path) };

        assert_eq!(code, 2, "git-mesh non-zero exit must produce exit code 2 (runtime error)");
    }
}
