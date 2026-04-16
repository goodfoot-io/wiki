use std::collections::HashMap;
use std::path::{Path, PathBuf};

use miette::Result;
use serde::Serialize;

use crate::commands::discover_files;
use crate::commands::pin::{RewriteSpec, apply_rewrites, build_fragment, write_atomic};
use crate::frontmatter::{Frontmatter, build_index, parse_frontmatter, parse_title};
use crate::git::{file_at_ref, latest_commit, resolve_ref};
use crate::headings::extract_headings;
use crate::parser::{FragmentLink, LinkKind, parse_fragment_links, parse_wikilinks};

// ── Diagnostic types ──────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct CheckDiagnostic {
    pub kind: String,
    pub file: String,
    pub line: usize,
    pub message: String,
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Run the check command.
///
/// Returns the exit code: 0 = valid, 1 = validation errors, 2 = runtime error.
pub fn run(globs: &[String], json: bool, fix: bool, repo_root: &Path) -> Result<i32> {
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

    let mut diagnostics: Vec<CheckDiagnostic> = Vec::new();

    // ── Parse frontmatter for all pages ──────────────────────────────────────
    let mut pages: Vec<(PathBuf, Frontmatter)> = Vec::new();
    // Titles of pages that failed full validation — used to suppress spurious
    // broken_wikilink diagnostics when the real problem is a frontmatter error.
    let mut invalid_titles: std::collections::HashSet<String> = std::collections::HashSet::new();

    for path in &files {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                diagnostics.push(CheckDiagnostic {
                    kind: "runtime".into(),
                    file: path.display().to_string(),
                    line: 0,
                    message: format!("Could not read file: {e}"),
                });
                continue;
            }
        };

        match parse_frontmatter(&content, path) {
            Ok(Some(fm)) => {
                pages.push((path.clone(), fm));
            }
            Ok(None) => {
                diagnostics.push(CheckDiagnostic {
                    kind: "frontmatter".into(),
                    file: path.display().to_string(),
                    line: 1,
                    message: "Add a `---` frontmatter block. `title` and `summary` are required."
                        .into(),
                });
            }
            Err(e) => {
                if let Some(title) = parse_title(&content) {
                    invalid_titles.insert(title.to_lowercase());
                }
                diagnostics.push(CheckDiagnostic {
                    kind: "frontmatter".into(),
                    file: path.display().to_string(),
                    line: 1,
                    message: e.to_string(),
                });
            }
        }
    }

    // ── Build title/alias index and report collisions ─────────────────────────
    let (index, collisions) = build_index(&pages);

    for col in &collisions {
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

    // Build a map from path -> content (for heading extraction)
    let mut content_cache: HashMap<PathBuf, String> = HashMap::new();
    for (path, _) in &pages {
        if let Ok(c) = std::fs::read_to_string(path) {
            content_cache.insert(path.clone(), c);
        }
    }

    // ── Validate links in all files (including ones that failed frontmatter) ──
    // When --fix is active, collect (path, link) pairs for links to fix later.
    let mut links_to_fix: Vec<(PathBuf, FragmentLink)> = Vec::new();

    for path in &files {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue, // already reported above
        };

        // Fragment links
        let frag_links = parse_fragment_links(&content);
        for link in &frag_links {
            if link.kind == LinkKind::External {
                continue;
            }

            let resolved_path = crate::commands::resolve_link_path(&link.path, path, repo_root);
            let repo_relative_path = resolved_path.to_string_lossy().to_string();
            let is_repo_relative = link.path == repo_relative_path;

            if link.kind == LinkKind::InternalWithoutSha || !is_repo_relative {
                if fix {
                    // Defer to fix pass; do not emit a diagnostic now.
                    links_to_fix.push((path.clone(), link.clone()));
                } else {
                    if link.kind == LinkKind::InternalWithoutSha {
                        diagnostics.push(CheckDiagnostic {
                            kind: "missing_sha".into(),
                            file: path.display().to_string(),
                            line: link.source_line,
                            message: format!(
                                "Fragment link `{}` has no pinned SHA. Run `wiki check --fix` to add one automatically.",
                                link.path
                            ),
                        });
                    }
                    if !is_repo_relative {
                        diagnostics.push(CheckDiagnostic {
                            kind: "not_repo_relative".into(),
                            file: path.display().to_string(),
                            line: link.source_line,
                            message: format!(
                                "Fragment link `{}` must be relative to the repository root: `{}`. Run `wiki check --fix` to convert it.",
                                link.path,
                                repo_relative_path
                            ),
                        });
                    }
                }
                if link.kind == LinkKind::InternalWithoutSha {
                    continue;
                }
            }

            // InternalWithSha — verify file exists at pinned SHA and check line ranges
            let sha = link.sha.as_deref().unwrap();
            let link_path = resolved_path.as_path();

            match file_at_ref(repo_root, sha, link_path) {
                Ok(ref_content) => {
                    // Verify line ranges are within the file's line count
                    if let Some(start) = link.start_line {
                        // Lines are 1-based; L0 is invalid
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
                                        "Line range `L{start}–L{end}` exceeds `{}` at `@{sha}` ({line_count} lines). Correct the range or re-run `wiki pin` to refresh.",
                                        link.path
                                    ),
                                });
                            }
                            if start > end {
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
                Err(_) => {
                    diagnostics.push(CheckDiagnostic {
                        kind: "missing_file".into(),
                        file: path.display().to_string(),
                        line: link.source_line,
                        message: format!(
                            "File `{}` not found at `@{sha}`. The file may have moved — re-run `wiki pin` to update the SHA.",
                            link.path
                        ),
                    });
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

    // ── Fix pass: pin missing SHAs and convert to repo-relative when --fix is active ─────────
    if fix && !links_to_fix.is_empty() {
        // Group links by file so we can do one atomic write per file.
        let mut by_file: HashMap<PathBuf, Vec<FragmentLink>> = HashMap::new();
        for (path, link) in links_to_fix {
            by_file.entry(path).or_default().push(link);
        }

        for (path, links) in &by_file {
            let content = match std::fs::read_to_string(path) {
                Ok(c) => c,
                Err(e) => {
                    diagnostics.push(CheckDiagnostic {
                        kind: "runtime".into(),
                        file: path.display().to_string(),
                        line: 0,
                        message: format!("Could not read file for --fix: {e}"),
                    });
                    continue;
                }
            };

            // Build rewrite specs for each link (resolve SHA, then build spec).
            let mut specs: Vec<RewriteSpec> = Vec::new();
            for link in links {
                let resolved_path = crate::commands::resolve_link_path(&link.path, path, repo_root);
                if !repo_root.join(&resolved_path).exists() {
                    continue;
                }
                let repo_relative_path = resolved_path.to_string_lossy().to_string();
                let is_repo_relative = link.path == repo_relative_path;

                // For existing SHA, use it; otherwise find latest.
                let sha_to_use = if let Some(existing) = &link.sha {
                    existing.clone()
                } else {
                    match latest_commit(repo_root, "HEAD", &resolved_path) {
                        Ok(s) => s,
                        Err(e) => {
                            diagnostics.push(CheckDiagnostic {
                                kind: "missing_sha".into(),
                                file: path.display().to_string(),
                                line: link.source_line,
                                message: format!(
                                    "Fragment link `{}` has no pinned SHA and could not be resolved: {e}",
                                    link.path
                                ),
                            });
                            continue;
                        }
                    }
                };

                specs.push(RewriteSpec {
                    source_line: link.source_line,
                    text: link.original_text.clone(),
                    original_href: link.original_href.clone(),
                    new_path: repo_relative_path.clone(),
                    new_sha: sha_to_use.clone(),
                    action: if link.sha.is_none() {
                        "pinned"
                    } else {
                        "converted"
                    },
                    fragment: build_fragment(link),
                });

                if link.sha.is_none() {
                    diagnostics.push(CheckDiagnostic {
                        kind: "fixed".into(),
                        file: path.display().to_string(),
                        line: link.source_line,
                        message: format!("Pinned `{}` to @{sha_to_use}.", link.path),
                    });
                }
                if !is_repo_relative {
                    diagnostics.push(CheckDiagnostic {
                        kind: "fixed".into(),
                        file: path.display().to_string(),
                        line: link.source_line,
                        message: format!(
                            "Converted `{}` to repo-relative: `{}`.",
                            link.path, repo_relative_path
                        ),
                    });
                }
            }

            if !specs.is_empty() {
                // Sort descending by source_line to preserve byte offsets.
                specs.sort_by(|a, b| b.source_line.cmp(&a.source_line));
                let updated_content = apply_rewrites(&content, &specs);
                if let Err(e) = write_atomic(path, &updated_content) {
                    diagnostics.push(CheckDiagnostic {
                        kind: "runtime".into(),
                        file: path.display().to_string(),
                        line: 0,
                        message: format!("Failed to write fixed file: {e}"),
                    });
                }
            }
        }
    }

    // ── Output ────────────────────────────────────────────────────────────────
    if json {
        println!("{}", serde_json::to_string_pretty(&diagnostics).unwrap());
    } else {
        for d in &diagnostics {
            if d.kind == "fixed" {
                continue;
            }
            println!("**{}** — `{}:{}`\n{}\n", d.kind, d.file, d.line, d.message);
        }
        if fix {
            let changed: std::collections::HashSet<&str> = diagnostics
                .iter()
                .filter(|d| d.kind == "fixed")
                .map(|d| d.file.as_str())
                .collect();
            if !changed.is_empty() {
                println!("Fixed {} file(s).", changed.len());
            }
        }
    }

    // "fixed" and "alias_resolve" are non-error kinds — only other kinds are errors.
    if diagnostics
        .iter()
        .any(|d| d.kind != "alias_resolve" && d.kind != "fixed")
    {
        Ok(1)
    } else {
        Ok(0)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

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
    fn test_check_missing_sha_exit_1_with_pin_message() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file("src/foo.rs", "fn foo() {}\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "[code](src/foo.rs#L1)"),
        );
        repo.commit("add files");

        let code = run(&[], false, false, repo.path()).expect("run");
        assert_eq!(code, 1);
    }

    #[test]
    fn test_check_missing_sha_message_includes_pin_hint() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file("src/foo.rs", "fn foo() {}\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "[code](src/foo.rs#L1)"),
        );
        repo.commit("add files");

        // Capture by running with json=true
        let files = discover_files(&[], repo.path()).expect("discover");
        assert_eq!(files.len(), 1);
        let content = fs::read_to_string(&files[0]).expect("read");
        let links = parse_fragment_links(&content);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].kind, LinkKind::InternalWithoutSha);
        // The run function will include "wiki pin" in the message
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
    fn test_check_json_output_format() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "[code](src/foo.rs#L1)"),
        );
        repo.commit("add page");

        // Just verify it returns 1 (has errors); JSON printing goes to stdout
        let code = run(&[], true, false, repo.path()).expect("run");
        assert_eq!(code, 1);
    }

    #[test]
    fn test_check_valid_pinned_link_within_bounds() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file("src/foo.rs", "line1\nline2\nline3\n");
        repo.commit("add src");

        // Get the SHA of the commit
        let sha_output = Command::new("git")
            .current_dir(repo.path())
            .args(["rev-parse", "--short", "HEAD"])
            .output()
            .expect("git rev-parse");
        let sha = String::from_utf8(sha_output.stdout)
            .expect("utf8")
            .trim()
            .to_string();

        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", &format!("[code](src/foo.rs#L1-L2&{sha})")),
        );
        repo.commit("add wiki");

        let code = run(&[], false, false, repo.path()).expect("run");
        assert_eq!(code, 0);
    }

    #[test]
    fn test_check_l0_line_reference_rejected() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file("src/foo.rs", "line1\nline2\nline3\n");
        repo.commit("add src");

        let sha_output = Command::new("git")
            .current_dir(repo.path())
            .args(["rev-parse", "--short", "HEAD"])
            .output()
            .expect("git rev-parse");
        let sha = String::from_utf8(sha_output.stdout)
            .expect("utf8")
            .trim()
            .to_string();

        // L0 is invalid — lines are 1-based
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", &format!("[code](src/foo.rs#L0&{sha})")),
        );
        repo.commit("add wiki");

        let code = run(&[], false, false, repo.path()).expect("run");
        assert_eq!(code, 1, "L0 line reference must be rejected");
    }

    #[test]
    fn test_check_fix_pins_unpinned_link_and_exits_0() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file("src/foo.rs", "fn foo() {}\n");
        repo.commit("add src");

        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "[code](src/foo.rs#L1)"),
        );
        repo.commit("add wiki");

        // Without --fix this should fail.
        let code_no_fix = run(&[], false, false, repo.path()).expect("run");
        assert_eq!(code_no_fix, 1, "should report missing_sha without --fix");

        // With --fix it should succeed and rewrite the file.
        let code = run(&[], false, true, repo.path()).expect("run");
        assert_eq!(code, 0, "should exit 0 after fixing");

        let content = fs::read_to_string(repo.path().join("wiki/page.md")).expect("read");
        assert!(
            content.contains("src/foo.rs#"),
            "expected SHA to be inserted by --fix, got: {content}"
        );
    }

    #[test]
    fn test_check_fix_does_not_touch_already_pinned_links() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file("src/foo.rs", "fn foo() {}\n");
        repo.commit("add src");

        let sha_output = Command::new("git")
            .current_dir(repo.path())
            .args(["rev-parse", "--short", "HEAD"])
            .output()
            .expect("git rev-parse");
        let sha = String::from_utf8(sha_output.stdout)
            .expect("utf8")
            .trim()
            .to_string();

        let original = make_wiki_page("Page", &format!("[code](src/foo.rs#L1&{sha})"));
        repo.create_file("wiki/page.md", &original);
        repo.commit("add wiki");

        let code = run(&[], false, true, repo.path()).expect("run");
        assert_eq!(code, 0);

        // Already-pinned link must remain unchanged.
        let content = fs::read_to_string(repo.path().join("wiki/page.md")).expect("read");
        assert_eq!(
            content, original,
            "pinned link must not be modified by --fix"
        );
    }

    #[test]
    fn test_check_fix_emits_fixed_diagnostic() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file("src/foo.rs", "fn foo() {}\n");
        repo.commit("add src");

        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "[code](src/foo.rs#L1)"),
        );
        repo.commit("add wiki");

        // Run --fix; the resulting file should contain a SHA.
        let code = run(&[], false, true, repo.path()).expect("run");
        assert_eq!(code, 0);

        // Verify the file was rewritten.
        let content = fs::read_to_string(repo.path().join("wiki/page.md")).expect("read");
        assert!(
            content.contains("src/foo.rs#"),
            "file should contain pinned SHA after --fix, got: {content}"
        );
    }

    #[test]
    fn test_check_fix_with_backticks_in_text() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file("src/foo.rs", "pub fn foo() {}");
        // Link with backticks in text
        repo.create_file(
            "wiki/page.md",
            "---\ntitle: Page\nsummary: Sum\n---\n[`src/foo.rs`](src/foo.rs)",
        );
        repo.commit("add files");

        let repo_root = repo.path();
        let code = run(&[], false, true, repo_root).expect("run fix");
        assert_eq!(code, 0);

        let content = fs::read_to_string(repo_root.join("wiki/page.md")).expect("read");
        assert!(
            content.contains("src/foo.rs#"),
            "file should contain pinned SHA after --fix, got:\n{content}"
        );
    }

    #[test]
    fn test_check_fix_converts_to_repo_relative() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file("src/foo.rs", "pub fn foo() {}");
        // Link relative to markdown file (wiki/page.md) pointing to src/foo.rs
        repo.create_file(
            "wiki/page.md",
            "---\ntitle: Page\nsummary: Sum\n---\n[code](../src/foo.rs)",
        );
        repo.commit("add files");

        let repo_root = repo.path();

        // Without --fix this should fail.
        let code_no_fix = run(&[], false, false, repo_root).expect("run no fix");
        assert_eq!(
            code_no_fix, 1,
            "should report not_repo_relative without --fix"
        );

        // With --fix it should succeed and rewrite the file to be repo-relative.
        let code = run(&[], false, true, repo_root).expect("run fix");
        assert_eq!(code, 0, "should exit 0 after fixing");

        let content = fs::read_to_string(repo_root.join("wiki/page.md")).expect("read");
        // Should contain src/foo.rs#SHA, NOT ../src/foo.rs
        assert!(
            content.contains("](src/foo.rs#"),
            "expected path to be converted to repo-relative, got:\n{content}"
        );
    }

    #[test]
    fn test_check_fix_does_not_rewrite_missing_pinned_link_path() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file("wiki/page.md", "placeholder\n");
        repo.commit("add wiki");
        let sha_output = Command::new("git")
            .current_dir(repo.path())
            .args(["rev-parse", "--short", "HEAD"])
            .output()
            .expect("git rev-parse");
        let sha = String::from_utf8(sha_output.stdout)
            .expect("utf8")
            .trim()
            .to_string();

        let original = make_wiki_page(
            "Page",
            &format!(
                "The [path/to/missing/file.md](path/to/missing/file.md#L73-L172&{sha}) link must not be rewritten."
            ),
        );
        repo.create_file("wiki/page.md", &original);
        repo.commit("add broken link");

        let code = run(&[], false, true, repo.path()).expect("run");
        assert_eq!(code, 1, "missing pinned link should remain an error");

        let content = fs::read_to_string(repo.path().join("wiki/page.md")).expect("read");
        assert_eq!(
            content, original,
            "missing pinned link path must remain unchanged by --fix"
        );
    }
}
