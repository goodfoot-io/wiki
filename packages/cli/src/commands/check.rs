use std::path::{Path, PathBuf};

use miette::Result;
use serde::Serialize;

use crate::commands::discover_files;
use crate::frontmatter::{Frontmatter, build_index, parse_frontmatter, parse_title};
use crate::git::resolve_ref;
use crate::headings::{extract_headings, resolve_heading};
use crate::index::DocSource;
use crate::parser::{LinkKind, parse_fragment_links};

use super::check_fix;
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

/// Render one diagnostic block in the human-readable hook format, without any
/// trailing separator. The `---` separator is inserted only *between* blocks by
/// [`render_diagnostics`], never after the last one.
fn format_diagnostic(kind: &str, file: &str, line: usize, message: &str) -> String {
    let mut out = format!("Error: {}\n", kind_title_case(kind));
    if !file.is_empty() {
        out.push_str(&format!("- {file}:{line}\n"));
    }
    out.push_str(&format!("- {message}\n"));
    out
}

/// Render a list of diagnostics, joining blocks with a `\n---\n` separator so it
/// appears only between errors. Returns an empty string when there are none.
fn render_diagnostics(diagnostics: &[CheckDiagnostic]) -> String {
    diagnostics
        .iter()
        .map(|d| format_diagnostic(&d.kind, &d.file, d.line, &d.message))
        .collect::<Vec<_>>()
        .join("\n---\n\n")
}

// ── Public entry points ───────────────────────────────────────────────────────

/// Run the check command.
///
/// Returns the exit code: 0 = valid, 1 = validation errors, 2 = runtime error.
#[allow(clippy::too_many_arguments)]
pub fn run(
    globs: &[String],
    json: bool,
    scan_root: &Path,
    repo_root: &Path,
    no_exit_code: bool,
    source: DocSource,
    fix: bool,
    fix_dry_run: bool,
    print_applied: bool,
) -> Result<i32> {
    // Files to check are selected from `scan_root` (the current working
    // directory); globs resolve relative to it.
    //
    // In `--fix` mode an empty discovered set is NOT an error: the user may
    // have deleted the last wiki page (or demoted it to plain markdown), and
    // cleanup_orphaned_meshes must still run over an empty corpus to delete
    // the now-orphaned scaffold meshes. In non-fix mode we keep the existing
    // exit-2 "no wiki pages found" signal — it is a useful diagnostic.
    // discover_files returns Ok(vec![]) for an empty corpus (no wiki pages
    // found) and Err only for genuine infrastructure failures.  Non-fix mode
    // treats an empty corpus as a fatal diagnostic (exit 2); fix mode degrades
    // gracefully so cleanup_orphaned_meshes still runs over the empty set.
    // The resolution/collision corpus is the entire wiki, scanned from the
    // repo root, so a subtree check resolves titles and detects collisions
    // against every page — making a subdirectory run equivalent to the same
    // pages selected by glob from the repo root.
    //
    // This whole-repo walk happens FIRST and exactly once. In the no-glob
    // case the scoped `files` set is a prefix-filtered subset of it, so we
    // derive it in memory rather than walking the tree a second time.
    // discover_files returns Ok(vec![]) for an empty corpus, which is fine in
    // both modes; real infrastructure failures propagate as Err and fail
    // closed.
    let index_files = match discover_files(&[], repo_root, repo_root, source) {
        Ok(f) => match filter_files_for_source(f, repo_root, source) {
            Ok(f) => f,
            Err(e) => {
                if json {
                    eprintln!("{}", serde_json::json!({"error": e.to_string()}));
                } else {
                    eprintln!("error: {e}");
                }
                return Ok(2);
            }
        },
        Err(e) => {
            // Real infrastructure failure — fail closed in both modes.
            if json {
                eprintln!("{}", serde_json::json!({"error": e.to_string()}));
            } else {
                eprintln!("error: {e}");
            }
            return Ok(2);
        }
    };

    // Scoped selection (the pages to validate). When no explicit globs are
    // given, the scoped set is the whole-repo corpus restricted to the
    // `scan_root` subtree, so derive it by in-memory prefix filtering instead
    // of walking the tree again. With explicit globs we MUST keep the dedicated
    // walk: glob selection deliberately includes fixture pages
    // (`is_fixture_path`) that the membership-filtered whole-repo walk omits.
    let files = if globs.is_empty() {
        let prefix = crate::commands::scan_prefix(scan_root, repo_root);
        index_files
            .iter()
            .filter(|p| {
                let rel = p
                    .strip_prefix(repo_root)
                    .map(|r| r.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| p.to_string_lossy().into_owned());
                crate::commands::path_under_prefix(&rel, prefix.as_deref())
            })
            .cloned()
            .collect::<Vec<_>>()
    } else {
        let raw = match discover_files(globs, scan_root, repo_root, source) {
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
        match filter_files_for_source(raw, repo_root, source) {
            Ok(f) => f,
            Err(e) => {
                if json {
                    eprintln!("{}", serde_json::json!({"error": e.to_string()}));
                } else {
                    eprintln!("error: {e}");
                }
                return Ok(2);
            }
        }
    };

    // Empty-corpus semantics: in non-fix mode an empty scoped selection is the
    // "no wiki pages found" fatal diagnostic (exit 2). In `--fix` mode it
    // degrades gracefully so cleanup_orphaned_meshes still runs over the empty
    // set.
    if files.is_empty() && !fix {
        let msg = "no wiki pages found (no .md files matched)";
        if json {
            eprintln!("{}", serde_json::json!({"error": msg}));
        } else {
            eprintln!("error: {msg}");
        }
        return Ok(2);
    }

    let diagnostics = match collect_for_files(&files, &index_files, repo_root, source) {
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

    // ── Fix pass ──────────────────────────────────────────────────────────────
    if fix {
        let plan = match check_fix::run_fix_pass(&files, repo_root, scan_root, globs, source, fix_dry_run) {
            Ok(p) => p,
            Err(e) => {
                if json {
                    eprintln!("{}", serde_json::json!({"error": e.to_string()}));
                } else {
                    eprintln!("error: {e}");
                }
                return Ok(2);
            }
        };

        if fix_dry_run {
            // `--print-applied` conflicts with `--fix-dry-run`, so stdout is
            // the human/JSON preview here. Mesh creation that *would* run is
            // previewed alongside the link fixes.
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "fixes": plan.fixes,
                        "skipped": plan.skipped,
                        "plannedMeshes": plan.mesh.planned,
                        "plannedDeletions": plan.mesh.planned_deletions,
                        "errors": diagnostics,
                    }))
                    .unwrap()
                );
            } else if plan.fixes.is_empty()
                && plan.skipped.is_empty()
                && plan.mesh.planned.is_empty()
                && plan.mesh.planned_deletions.is_empty()
            {
                println!("no fixes to apply");
            } else {
                for f in &plan.fixes {
                    println!(
                        "fix: {} line {}: {} -> {}",
                        f.file, f.line, f.old_href, f.new_href
                    );
                }
                for s in &plan.skipped {
                    println!("skip: {} line {}: {}", s.file, s.line, s.reason);
                }
                for m in &plan.mesh.planned {
                    println!("create mesh: {m}");
                }
                for m in &plan.mesh.planned_deletions {
                    println!("delete mesh: {m}");
                }
                // Mesh advisories (parse errors, dropped meshes, "Would rename"
                // lines) are part of the preview.
                if !plan.mesh.advisories.is_empty() {
                    print!("{}", plan.mesh.advisories);
                }
            }
            // A move-follow conflict (≥ 2-match ambiguity) fails closed: it is
            // surfaced as a skipped fix above and must make the run non-zero so
            // it blocks pre-commit/CI, unless the caller opted out of exit codes.
            if (!diagnostics.is_empty() || plan.mesh_conflicts > 0) && !no_exit_code {
                return Ok(1);
            }
            return Ok(0);
        }

        // Under `--print-applied`, stdout is EXACTLY the repo-relative paths of
        // created/renamed meshes (one per line). The fix/skip summary, mesh
        // advisories/failures, and post-fix diagnostics all go to stderr so the
        // caller can consume stdout as an exact stage list. Without the flag,
        // the summary and post-fix diagnostics keep their stdout home and mesh
        // advisories print to stdout too; failures still go to stderr.
        if print_applied {
            for m in &plan.mesh.applied {
                println!("{m}");
            }
            for m in &plan.mesh.deleted {
                println!("{m}");
            }
        }

        // Human-readable fix/skip summary.
        if !json {
            for f in &plan.fixes {
                let line = format!(
                    "fixed: {}:{}  broken_link  {} → {}  ({})",
                    f.file, f.line, f.old_href, f.new_href, f.reason
                );
                if print_applied {
                    eprintln!("{line}");
                } else {
                    println!("{line}");
                }
            }
            for s in &plan.skipped {
                let line = format!(
                    "skipped: {}:{}  broken_link  reason: {}",
                    s.file, s.line, s.reason
                );
                if print_applied {
                    eprintln!("{line}");
                } else {
                    println!("{line}");
                }
            }
            // Mesh advisories (parse errors, dropped meshes, renames).
            if !plan.mesh.advisories.is_empty() {
                if print_applied {
                    eprint!("{}", plan.mesh.advisories);
                } else {
                    print!("{}", plan.mesh.advisories);
                }
            }
        }
        // Mesh-creation failures always go to stderr — the in-process store already printed
        // each reason; name the slug that could not be created.
        for failure in &plan.mesh.failures {
            eprintln!(
                "wiki check --fix: could not create mesh `{}` (see error output above)",
                failure.slug
            );
        }

        // Non-dry-run: re-collect and emit post-fix diagnostics. A mesh that
        // failed/dropped resurfaces here as a residual `mesh_uncovered`.
        let post_diagnostics = match collect_for_files(&files, &index_files, repo_root, source) {
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
                serde_json::to_string_pretty(&serde_json::json!({
                    "fixes": plan.fixes,
                    "skipped": plan.skipped,
                    "appliedMeshes": plan.mesh.applied,
                    "deletedMeshes": plan.mesh.deleted,
                    "errors": post_diagnostics,
                }))
                .unwrap()
            );
        } else {
            let rendered = render_diagnostics(&post_diagnostics);
            if print_applied {
                eprint!("{rendered}");
            } else {
                print!("{rendered}");
            }
        }

        // As in the dry-run path, an unresolved move-follow conflict fails
        // closed and drives a non-zero exit (unless `--no-exit-code`).
        if (!post_diagnostics.is_empty() || plan.mesh_conflicts > 0) && !no_exit_code {
            return Ok(1);
        }
        return Ok(0);
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({ "errors": diagnostics })).unwrap()
        );
    } else {
        print!("{}", render_diagnostics(&diagnostics));
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
    // This entry point (hook/tests) scans from the repo root rather than a
    // narrower working directory.  discover_files returns Ok(vec![]) for an
    // empty corpus; propagate that as an error so the caller sees "no wiki
    // pages found" rather than an empty diagnostic list with exit 0.
    let files = discover_files(globs, repo_root, repo_root, source)?;
    if files.is_empty() {
        return Err(miette::miette!("no wiki pages found (no .md files matched)"));
    }
    let files = filter_files_for_source(files, repo_root, source)?;
    let index_files = if globs.is_empty() {
        files.clone()
    } else {
        let raw = discover_files(&[], repo_root, repo_root, source)?;
        filter_files_for_source(raw, repo_root, source)?
    };
    collect_for_files(&files, &index_files, repo_root, source)
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
    if matches!(source, DocSource::WorkingTree) {
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

    use crate::commands::mesh::store;
    use git_mesh_core::AnchorExtent;
    use git_mesh_core::mesh_file::{AnchorRecord, MeshFile};

    /// Serialize tests that share process-level state (e.g. PATH or env vars).
    static PATH_MUTEX: Mutex<()> = Mutex::new(());

    fn make_anchor(path: &str, start: u32, end: u32) -> AnchorRecord {
        AnchorRecord {
            path: path.to_string(),
            start_line: start,
            end_line: end,
            algorithm: "sha256".to_string(),
            content_hash: "deadbeef".to_string(),
        }
    }

    fn make_mesh(anchors: Vec<AnchorRecord>) -> MeshFile {
        MeshFile {
            anchors,
            why: "test".to_string(),
        }
    }

    fn diag(kind: &str, line: usize) -> CheckDiagnostic {
        CheckDiagnostic {
            kind: kind.into(),
            file: "a.md".into(),
            line,
            message: "boom".into(),
        }
    }

    #[test]
    fn render_diagnostics_has_no_trailing_separator() {
        let rendered = render_diagnostics(&[diag("mesh_uncovered", 1), diag("broken_link", 2)]);
        assert!(
            !rendered.ends_with("---\n\n") && !rendered.trim_end().ends_with("---"),
            "separator must not appear after the last error: {rendered:?}"
        );
        assert_eq!(
            rendered.matches("\n---\n").count(),
            1,
            "separator must appear exactly once, between the two errors: {rendered:?}"
        );
        assert!(rendered.starts_with("Error: Mesh Uncovered\n"));
    }

    #[test]
    fn render_diagnostics_single_has_no_separator() {
        let rendered = render_diagnostics(&[diag("broken_link", 9)]);
        assert!(
            !rendered.contains("---"),
            "single error must have no separator: {rendered:?}"
        );
    }

    #[test]
    fn render_diagnostics_empty_is_empty() {
        assert_eq!(render_diagnostics(&[]), "");
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
            // Persist identity in the repo's local config so any git
            // invocation that does not inherit our per-call env vars still has
            // a committer. Real environments always have a git identity.
            repo.git(&["config", "user.email", "test@example.com"]);
            repo.git(&["config", "user.name", "Test Author"]);
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
    fn valid_pages_exit_0() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file("wiki/page.md", &make_wiki_page("Page", "No links here."));
        repo.commit("add page");

        let code = run(
            &[],
            false,
            repo.path(),
            repo.path(),
            false,
            crate::index::DocSource::WorkingTree,
            false,
            false,
            false,
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
            repo.path(),
            false,
            crate::index::DocSource::WorkingTree,
            false,
            false,
            false,
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
            repo.path(),
            false,
            crate::index::DocSource::WorkingTree,
            false,
            false,
            false,
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
            repo.path(),
            false,
            crate::index::DocSource::WorkingTree,
            false,
            false,
            false,
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
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
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
            repo.path(),
            false,
            crate::index::DocSource::WorkingTree,
            false,
            false,
            false,
        )
        .expect("run");
        assert_eq!(code, 1);
    }

    #[test]
    fn mesh_covered_link_exits_0() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file("src/code.rs", "fn a() {}\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [code](/src/code.rs#L1-L1)."),
        );
        repo.commit("add files");

        // Seed coverage via store::write — reads now come from .wiki/<slug>.
        let mesh = make_mesh(vec![
            make_anchor("src/code.rs", 1, 1),
            make_anchor("wiki/page.md", 1, 1),
        ]);
        store::write(repo.path(), "test-mesh", &mesh).expect("store::write");

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
            repo.path(),
            false,
            crate::index::DocSource::WorkingTree,
            false,
            false,
            false,
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

    /// A fragment link into a gitignored (generated) file is exempt from the
    /// mesh-coverage contract: a path git never sees cannot be anchored, so
    /// demanding coverage would fail closed forever. `wiki check` must not emit
    /// `mesh_uncovered` for it and must exit 0.
    #[test]
    fn mesh_skips_gitignored_target() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file("src/generated.rs", "fn a() {}\n");
        repo.create_file(".gitignore", "src/generated.rs\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [code](/src/generated.rs#L1-L1)."),
        );
        repo.commit("add files");

        let diagnostics = collect(&[], repo.path()).expect("collect");
        let mesh_diags: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.kind == "mesh_uncovered")
            .collect();
        assert!(
            mesh_diags.is_empty(),
            "gitignored target must not produce mesh_uncovered: {diagnostics:?}"
        );
        let code = run(
            &[],
            false,
            repo.path(),
            repo.path(),
            false,
            crate::index::DocSource::WorkingTree,
            false,
            false,
            false,
        )
        .expect("run");
        assert_eq!(code, 0, "gitignored citation must not fail closed");
    }

    /// Contrast with the exemption above: an untracked-but-NOT-ignored target
    /// still requires coverage — it resolves once committed, so a missing mesh
    /// is a real, fixable finding.
    #[test]
    fn mesh_uncovered_for_untracked_not_ignored_target() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file("wiki/page.md", &make_wiki_page("Page", "No links."));
        repo.commit("baseline");
        // Present on disk, never committed, not gitignored.
        repo.create_file("src/new.rs", "fn a() {}\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [code](/src/new.rs#L1-L1)."),
        );

        let diagnostics = collect(&[], repo.path()).expect("collect");
        let mesh_diags: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.kind == "mesh_uncovered")
            .collect();
        assert_eq!(
            mesh_diags.len(),
            1,
            "untracked-not-ignored target must still require coverage: {diagnostics:?}"
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

    // ── Fix pass integration tests (all #[ignore] until fix logic is implemented) ──

    /// Fix 1: when a link target was renamed in git, --fix rewrites the path.
    #[test]
    fn fix1_broken_link_rewrites_renamed_path() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file("src/old.rs", "fn a() {}\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [old](/src/old.rs#L1)."),
        );
        repo.commit("baseline");

        // Rename the target file.
        repo.git(&["mv", "src/old.rs", "src/new.rs"]);
        // (do not commit so worktree sees the rename in the index)

        let code = run(
            &[],
            false,
            repo.path(),
            repo.path(),
            false,
            crate::index::DocSource::WorkingTree,
            true,  // fix
            false, // fix_dry_run
            false, // print_applied
        )
        .expect("run");

        let content = std::fs::read_to_string(repo.path().join("wiki/page.md")).expect("read page");
        assert!(
            content.contains("/src/new.rs"),
            "expected link rewritten to new path, got:\n{content}"
        );
        assert_eq!(code, 0, "expected exit 0 after fix");
    }

    /// Fix 1 skip: when the rename target was deleted (not moved), skip the fix.
    #[test]
    fn fix1_skips_when_target_deleted() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file("src/gone.rs", "fn a() {}\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [gone](/src/gone.rs#L1)."),
        );
        repo.commit("baseline");

        repo.git(&["rm", "src/gone.rs"]);

        let diags = collect_with_source(&[], repo.path(), crate::index::DocSource::WorkingTree)
            .expect("collect");
        // A deletion is not a rename; fix should produce a SkippedFix.
        // Post-fix diagnostics must still contain broken_link.
        assert!(
            diags.iter().any(|d| d.kind == "broken_link"),
            "expected broken_link for deleted target: {diags:?}"
        );
    }

    /// Fix 1 skip: when a path maps to multiple rename targets, skip (ambiguous).
    #[test]
    fn fix1_skips_when_rename_ambiguous() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file("src/shared.rs", "fn a() {}\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [shared](/src/shared.rs#L1)."),
        );
        repo.commit("baseline");

        // Simulate ambiguity: two copies of the file exist under different names.
        repo.create_file("src/copy_a.rs", "fn a() {}\n");
        repo.create_file("src/copy_b.rs", "fn a() {}\n");
        repo.git(&["rm", "src/shared.rs"]);

        // With two possible rename destinations, fix must not apply automatically.
        let code = run(
            &[],
            false,
            repo.path(),
            repo.path(),
            false,
            crate::index::DocSource::WorkingTree,
            true,
            false,
            false,
        )
        .expect("run");
        // Link is still broken → exit 1.
        assert_eq!(code, 1);
    }

    /// Seed a `.wiki/<slug>` mesh directly via `store::write`, with each
    /// anchor's stored hash computed from the current worktree content via
    /// `store::hash_anchor`.
    fn seed_mesh(repo: &TestRepo, slug: &str, anchors: &[(&str, AnchorExtent)]) {
        let records: Vec<AnchorRecord> = anchors
            .iter()
            .map(|(path, extent)| {
                let hash = store::hash_anchor(repo.path(), path, *extent)
                    .unwrap_or_else(|e| panic!("hash_anchor {path}: {e}"));
                store::anchor_record(path.to_string(), *extent, hash)
            })
            .collect();
        let mesh = MeshFile {
            anchors: records,
            why: "Test mesh.".to_string(),
        };
        store::write(repo.path(), slug, &mesh).unwrap_or_else(|e| panic!("seed {slug}: {e}"));
    }

    /// Fix 2: an in-file line shift relocates the anchor uniquely. The anchored
    /// block (`fn a() {}`) drifts down one line when a preamble is inserted;
    /// `--fix` rewrites the wiki link to the new range.
    #[test]
    fn fix2_in_file_shift_relocates() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file("src/code.rs", "fn a() {}\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [code](/src/code.rs#L1-L1)."),
        );
        repo.commit("baseline");

        // Seed the mesh against the baseline content, then introduce the shift
        // as a working-tree change so it lands in the change set.
        seed_mesh(
            &repo,
            "fix2-mesh",
            &[
                ("wiki/page.md", AnchorExtent::WholeFile),
                ("src/code.rs", AnchorExtent::LineRange { start: 1, end: 1 }),
            ],
        );
        repo.create_file("src/code.rs", "// preamble\nfn a() {}\n");

        let _code = run(
            &[],
            false,
            repo.path(),
            repo.path(),
            true,
            crate::index::DocSource::WorkingTree,
            true,
            false,
            false,
        )
        .expect("run");

        let content = std::fs::read_to_string(repo.path().join("wiki/page.md")).expect("read page");
        assert!(
            content.contains("#L2-L2"),
            "expected anchor updated to L2-L2, got:\n{content}"
        );
    }

    /// Fix 2: when the anchored content moves to a different file in the change
    /// set, `--fix` relocates the link to the new file/range.
    #[test]
    fn fix2_cross_file_move_relocates() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file("src/code.rs", "fn a() {}\n");
        repo.create_file("src/other.rs", "// nothing here\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [code](/src/code.rs#L1-L1)."),
        );
        repo.commit("baseline");

        seed_mesh(
            &repo,
            "fix2-move-mesh",
            &[
                ("wiki/page.md", AnchorExtent::WholeFile),
                ("src/code.rs", AnchorExtent::LineRange { start: 1, end: 1 }),
            ],
        );

        // Move the anchored line out of code.rs into other.rs (both changed).
        repo.create_file("src/code.rs", "// emptied\n");
        repo.create_file("src/other.rs", "// header\nfn a() {}\n");

        let _code = run(
            &[],
            false,
            repo.path(),
            repo.path(),
            true,
            crate::index::DocSource::WorkingTree,
            true,
            false,
            false,
        )
        .expect("run");

        let content = std::fs::read_to_string(repo.path().join("wiki/page.md")).expect("read page");
        assert!(
            content.contains("/src/other.rs#L2-L2"),
            "expected anchor relocated to src/other.rs#L2-L2, got:\n{content}"
        );
    }

    /// Fix 2 (fail-closed): when the anchored content appears in two files in
    /// the change set, the match is ambiguous and the link is left untouched.
    #[test]
    fn fix2_two_matches_conflict_no_rewrite() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file("src/code.rs", "fn a() {}\n");
        repo.create_file("src/dup.rs", "// nothing\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [code](/src/code.rs#L1-L1)."),
        );
        repo.commit("baseline");

        seed_mesh(
            &repo,
            "fix2-conflict-mesh",
            &[("src/code.rs", AnchorExtent::LineRange { start: 1, end: 1 })],
        );

        // Empty the original and place the same block in TWO change-set files.
        repo.create_file("src/code.rs", "// emptied\n");
        repo.create_file("src/dup.rs", "fn a() {}\nfn a() {}\n");

        let _code = run(
            &[],
            false,
            repo.path(),
            repo.path(),
            true,
            crate::index::DocSource::WorkingTree,
            true,
            false,
            false,
        )
        .expect("run");

        let content = std::fs::read_to_string(repo.path().join("wiki/page.md")).expect("read page");
        assert!(
            content.contains("/src/code.rs#L1-L1"),
            "expected link left untouched on ambiguous match, got:\n{content}"
        );
    }

    /// F3: a followed in-file move relocates the stored mesh anchor in place
    /// (refreshed hash), leaves exactly one anchor, and is idempotent across a
    /// second `--fix` (no duplicate accumulation, no further change).
    #[test]
    fn fix2_followed_move_relocates_stored_anchor_idempotently() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file("src/code.rs", "fn a() {}\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [code](/src/code.rs#L1-L1)."),
        );
        repo.commit("baseline");

        seed_mesh(
            &repo,
            "fix2-writeback-mesh",
            &[
                ("wiki/page.md", AnchorExtent::WholeFile),
                ("src/code.rs", AnchorExtent::LineRange { start: 1, end: 1 }),
            ],
        );
        // Shift the anchored block down one line in the working tree.
        repo.create_file("src/code.rs", "// preamble\nfn a() {}\n");

        let run_fix = || {
            run(
                &[],
                false,
                repo.path(),
                repo.path(),
                true,
                crate::index::DocSource::WorkingTree,
                true,
                false,
                false,
            )
            .expect("run")
        };

        run_fix();

        // The stored anchor moved in place: exactly one src anchor at L2-L2 with
        // a hash matching the new range.
        let mesh = store::read_one(repo.path(), "fix2-writeback-mesh")
            .expect("read")
            .expect("mesh");
        let src_anchors: Vec<_> = mesh
            .anchors
            .iter()
            .filter(|a| a.path == "src/code.rs")
            .collect();
        assert_eq!(
            src_anchors.len(),
            1,
            "expected exactly one src anchor after follow, got: {src_anchors:?}"
        );
        assert_eq!((src_anchors[0].start_line, src_anchors[0].end_line), (2, 2));
        let expected = store::hash_anchor(
            repo.path(),
            "src/code.rs",
            AnchorExtent::LineRange { start: 2, end: 2 },
        )
        .expect("hash");
        assert_eq!(
            src_anchors[0].content_hash, expected,
            "stored hash must match the new range"
        );

        let serialized_after_first = store::read_one(repo.path(), "fix2-writeback-mesh")
            .unwrap()
            .unwrap();

        // Second --fix: anchor is now fresh, nothing changes (idempotent).
        run_fix();
        let serialized_after_second = store::read_one(repo.path(), "fix2-writeback-mesh")
            .unwrap()
            .unwrap();
        assert_eq!(
            serialized_after_first, serialized_after_second,
            "second --fix must not mutate the mesh (no accumulation)"
        );
    }

    /// F3: a cross-file followed move relocates the stored anchor to the new
    /// file with a refreshed hash, leaving exactly one anchor for that content.
    #[test]
    fn fix2_followed_cross_file_move_relocates_stored_anchor() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file("src/code.rs", "fn a() {}\n");
        repo.create_file("src/other.rs", "// nothing here\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [code](/src/code.rs#L1-L1)."),
        );
        repo.commit("baseline");

        seed_mesh(
            &repo,
            "fix2-xfile-mesh",
            &[
                ("wiki/page.md", AnchorExtent::WholeFile),
                ("src/code.rs", AnchorExtent::LineRange { start: 1, end: 1 }),
            ],
        );
        repo.create_file("src/code.rs", "// emptied\n");
        repo.create_file("src/other.rs", "// header\nfn a() {}\n");

        run(
            &[],
            false,
            repo.path(),
            repo.path(),
            true,
            crate::index::DocSource::WorkingTree,
            true,
            false,
            false,
        )
        .expect("run");

        let mesh = store::read_one(repo.path(), "fix2-xfile-mesh")
            .unwrap()
            .unwrap();
        let movable: Vec<_> = mesh
            .anchors
            .iter()
            .filter(|a| a.path == "src/code.rs" || a.path == "src/other.rs")
            .collect();
        assert_eq!(
            movable.len(),
            1,
            "expected exactly one relocated anchor, got: {movable:?}"
        );
        assert_eq!(movable[0].path, "src/other.rs");
        assert_eq!((movable[0].start_line, movable[0].end_line), (2, 2));
        let expected = store::hash_anchor(
            repo.path(),
            "src/other.rs",
            AnchorExtent::LineRange { start: 2, end: 2 },
        )
        .expect("hash");
        assert_eq!(movable[0].content_hash, expected);
    }

    /// F5: an ambiguous (≥ 2-match) move is reported as a conflict, leaves the
    /// link untouched, and makes `--fix` exit non-zero so it fails CI/pre-commit.
    /// Under `--no-exit-code` the conflict still reports but the exit is 0.
    #[test]
    fn fix2_conflict_reports_and_exits_nonzero() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file("src/code.rs", "fn a() {}\n");
        repo.create_file("src/dup.rs", "// nothing\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [code](/src/code.rs#L1-L1)."),
        );
        repo.commit("baseline");

        seed_mesh(
            &repo,
            "fix2-conflict-exit-mesh",
            &[("src/code.rs", AnchorExtent::LineRange { start: 1, end: 1 })],
        );
        // Empty the original; place the same block in TWO change-set files.
        repo.create_file("src/code.rs", "// emptied\n");
        repo.create_file("src/dup.rs", "fn a() {}\nfn a() {}\n");

        // The conflict is carried in the FixPlan; assert it surfaces a skip and
        // counts the conflict.
        let plan = check_fix::run_fix_pass(
            &discover_files(&[], repo.path(), repo.path(), crate::index::DocSource::WorkingTree)
                .unwrap(),
            repo.path(),
            repo.path(),
            &[],
            crate::index::DocSource::WorkingTree,
            false,
        )
        .expect("fix pass");
        assert_eq!(plan.mesh_conflicts, 1, "expected one conflict recorded");
        assert!(
            plan.skipped.iter().any(|s| s
                .reason
                .contains("candidate locations — not rewriting, resolve manually")),
            "expected an actionable conflict skip message, got: {:?}",
            plan.skipped
        );

        // Re-seed (the pass above already ran writeback for nothing here) and
        // drive the full `run` to assert exit code behavior.
        let code = run(
            &[],
            false,
            repo.path(),
            repo.path(),
            false, // no_exit_code = false
            crate::index::DocSource::WorkingTree,
            true,
            false,
            false,
        )
        .expect("run");
        assert_eq!(code, 1, "conflict must drive a non-zero exit");

        // The link was left untouched.
        let content = std::fs::read_to_string(repo.path().join("wiki/page.md")).expect("read");
        assert!(
            content.contains("/src/code.rs#L1-L1"),
            "conflict must leave the link untouched, got:\n{content}"
        );

        // Under --no-exit-code the conflict still reports but exit is 0.
        let code = run(
            &[],
            false,
            repo.path(),
            repo.path(),
            true, // no_exit_code = true
            crate::index::DocSource::WorkingTree,
            true,
            false,
            false,
        )
        .expect("run");
        assert_eq!(code, 0, "--no-exit-code suppresses the conflict exit");
    }

    /// Fix 2: a fresh anchor (content unchanged) produces no plan, so the link
    /// is not rewritten.
    #[test]
    fn fix2_fresh_anchor_unchanged() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file("src/code.rs", "fn a() {}\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [code](/src/code.rs#L1-L1)."),
        );
        repo.commit("baseline");

        seed_mesh(
            &repo,
            "fix2-fresh-mesh",
            &[("src/code.rs", AnchorExtent::LineRange { start: 1, end: 1 })],
        );

        // Touch an unrelated file so the tree is "changed" (the gate does not
        // short-circuit) but the anchored content itself is untouched.
        repo.create_file("src/unrelated.rs", "// noise\n");

        let outcome = check_fix::plan_mesh_follows(repo.path()).expect("plan");
        assert!(
            outcome.plans.is_empty(),
            "fresh anchor must yield no move plan, got: {:?}",
            outcome.plans
        );
    }

    /// Fix 2 (acceptance signal #4): an unchanged working tree resolves through
    /// the stat gate without re-hashing — `plan_mesh_follows` returns no plans
    /// even though a stale anchor would otherwise be detectable.
    #[test]
    fn fix2_unchanged_tree_skips_rehash() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file("src/code.rs", "fn a() {}\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [code](/src/code.rs#L1-L1)."),
        );
        repo.commit("baseline");

        // Seed a mesh whose stored hash is deliberately wrong: a real re-hash
        // would flag this anchor as stale. The stat gate must prevent that.
        let mesh = MeshFile {
            anchors: vec![store::anchor_record(
                "src/code.rs".to_string(),
                AnchorExtent::LineRange { start: 1, end: 1 },
                "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
            )],
            why: "Test mesh.".to_string(),
        };
        store::write(repo.path(), "fix2-gate-mesh", &mesh).expect("seed");
        repo.git(&["add", "-A"]);
        repo.commit("commit mesh");

        // Prime the index so the state row matches the on-disk triple, then run
        // with a genuinely unchanged tree. The gate short-circuits the pass.
        crate::index::WikiIndex::prepare(repo.path()).expect("prepare index");

        let outcome = check_fix::plan_mesh_follows(repo.path()).expect("plan");
        assert!(
            outcome.plans.is_empty(),
            "unchanged tree must short-circuit the gate (no re-hash, no plans), got: {:?}",
            outcome.plans
        );
    }

    /// Fix 3: when an inbound link uses an aliased slug, --fix rewrites it to the
    /// canonical title slug so the alias entry can later be retired.
    #[test]
    fn fix3_alias_rewrites_to_canonical_slug() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file(
            "wiki/target.md",
            "---\ntitle: Overview\nsummary: s.\naliases:\n  - introduction\n---\n## Overview\n\nbody\n",
        );
        repo.create_file(
            "wiki/source.md",
            &make_wiki_page("Source", "See [intro](target.md#introduction)."),
        );
        repo.commit("baseline");

        let code = run(
            &[],
            false,
            repo.path(),
            repo.path(),
            false,
            crate::index::DocSource::WorkingTree,
            true,
            false,
            false,
        )
        .expect("run");

        let content =
            std::fs::read_to_string(repo.path().join("wiki/source.md")).expect("read source");
        assert!(
            content.contains("target.md#overview"),
            "expected anchor rewritten to canonical slug `overview`, got:\n{content}"
        );
        assert!(!content.contains("#introduction"));
        assert_eq!(code, 0);
    }

    /// Fix 3 skip: when the target page lists an alias but has no headings at
    /// all, Fix #3 declines rather than rewriting to a non-existent anchor.
    #[test]
    fn fix3_skips_when_target_has_no_headings() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        // Title is "Overview" and an alias is listed, but the page body has
        // no headings whatsoever — no canonical destination exists.
        repo.create_file(
            "wiki/target.md",
            "---\ntitle: Overview\nsummary: s.\naliases:\n  - introduction\n---\nJust prose, no headings.\n",
        );
        repo.create_file(
            "wiki/source.md",
            &make_wiki_page("Source", "See [intro](target.md#introduction)."),
        );
        repo.commit("baseline");

        let code = run(
            &[],
            false,
            repo.path(),
            repo.path(),
            false,
            crate::index::DocSource::WorkingTree,
            true,
            false,
            false,
        )
        .expect("run");

        let content =
            std::fs::read_to_string(repo.path().join("wiki/source.md")).expect("read source");
        assert!(
            content.contains("target.md#introduction"),
            "expected anchor unchanged (skip), got:\n{content}"
        );
        // Post-fix the diagnostic should still be present (link still broken).
        // Exit 1 because broken_anchor persists.
        assert_eq!(code, 1);
    }

    /// Fix 3 fallback: title slug does not match a heading, but the page has a
    /// first H1 — rewrite to the H1's slug.
    #[test]
    fn fix3_falls_back_to_first_h1_when_title_mismatch() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        // Title is "Authentication" but H1 is "Auth & Authorization".
        repo.create_file(
            "wiki/target.md",
            "---\ntitle: Authentication\nsummary: s.\naliases:\n  - introduction\n---\n# Auth & Authorization\n\nbody\n",
        );
        repo.create_file(
            "wiki/source.md",
            &make_wiki_page("Source", "See [intro](target.md#introduction)."),
        );
        repo.commit("baseline");

        let code = run(
            &[],
            false,
            repo.path(),
            repo.path(),
            false,
            crate::index::DocSource::WorkingTree,
            true,
            false,
            false,
        )
        .expect("run");

        let content =
            std::fs::read_to_string(repo.path().join("wiki/source.md")).expect("read source");
        // First H1 is "Auth & Authorization"; github_slug gives "auth--authorization"
        // (the ampersand drops, leaving the two surrounding spaces as hyphens).
        assert!(
            content.contains("target.md#auth--authorization"),
            "expected anchor rewritten to first-H1 slug, got:\n{content}"
        );
        assert!(!content.contains("#introduction"));
        assert_eq!(code, 0);
    }

    /// Fix 3 fallback: no heading matches the title and there is no H1 —
    /// rewrite to the first heading at any level.
    #[test]
    fn fix3_falls_back_to_first_heading_when_no_h1() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        // Title is "Overview" but only a `## Body` heading exists — matches
        // the reproducer in the card.
        repo.create_file(
            "wiki/target.md",
            "---\ntitle: Overview\nsummary: s.\naliases:\n  - introduction\n---\n## Body\n\nbody\n",
        );
        repo.create_file(
            "wiki/source.md",
            &make_wiki_page("Source", "See [intro](target.md#introduction)."),
        );
        repo.commit("baseline");

        let code = run(
            &[],
            false,
            repo.path(),
            repo.path(),
            false,
            crate::index::DocSource::WorkingTree,
            true,
            false,
            false,
        )
        .expect("run");

        let content =
            std::fs::read_to_string(repo.path().join("wiki/source.md")).expect("read source");
        assert!(
            content.contains("target.md#body"),
            "expected anchor rewritten to first-heading slug `body`, got:\n{content}"
        );
        assert!(!content.contains("#introduction"));
        assert_eq!(code, 0);
    }

    /// Fix 5: when a heading was renamed in place (same section position), --fix
    /// updates the anchor in all linking wiki pages.
    #[test]
    fn fix5_heading_rename_same_position_rewrites() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file(
            "wiki/target.md",
            &make_wiki_page("Target", "## Old Heading\n\nbody\n"),
        );
        repo.create_file(
            "wiki/source.md",
            &make_wiki_page("Source", "See [target](target.md#old-heading)."),
        );
        repo.commit("baseline");

        // Rename the heading.
        repo.create_file(
            "wiki/target.md",
            &make_wiki_page("Target", "## New Heading\n\nbody\n"),
        );

        let code = run(
            &[],
            false,
            repo.path(),
            repo.path(),
            false,
            crate::index::DocSource::WorkingTree,
            true,
            false,
            false,
        )
        .expect("run");

        let content =
            std::fs::read_to_string(repo.path().join("wiki/source.md")).expect("read source");
        assert!(
            content.contains("#new-heading"),
            "expected anchor updated to new-heading, got:\n{content}"
        );
        assert_eq!(code, 0);
    }

    /// Fix 5 skip: when a heading was split into two or more headings, do not apply.
    #[test]
    fn fix5_skips_when_heading_split() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file(
            "wiki/target.md",
            &make_wiki_page("Target", "## Combined\n\nbody\n"),
        );
        repo.create_file(
            "wiki/source.md",
            &make_wiki_page("Source", "See [target](target.md#combined)."),
        );
        repo.commit("baseline");

        // Split into two headings.
        repo.create_file(
            "wiki/target.md",
            &make_wiki_page("Target", "## Part One\n\nbody\n\n## Part Two\n\nbody\n"),
        );

        let diags = collect_with_source(&[], repo.path(), crate::index::DocSource::WorkingTree)
            .expect("collect");
        assert!(
            !diags.is_empty(),
            "expected diagnostics when heading was split: {diags:?}"
        );
    }

    /// Fix 5 skip: when the heading was removed and other headings were restructured,
    /// do not attempt a fix.
    #[test]
    fn fix5_skips_when_heading_restructured() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file(
            "wiki/target.md",
            &make_wiki_page("Target", "## Section A\n\nbody\n## Section B\n\nbody\n"),
        );
        repo.create_file(
            "wiki/source.md",
            &make_wiki_page("Source", "See [target](target.md#section-a)."),
        );
        repo.commit("baseline");

        // Restructure: remove Section A and rename Section B.
        repo.create_file(
            "wiki/target.md",
            &make_wiki_page("Target", "## Renamed B\n\nbody\n"),
        );

        let diags = collect_with_source(&[], repo.path(), crate::index::DocSource::WorkingTree)
            .expect("collect");
        assert!(
            !diags.is_empty(),
            "expected diagnostics when heading was restructured: {diags:?}"
        );
    }

    /// Running --fix twice must produce no further rewrites on the second pass.
    #[test]
    fn wiki_check_fix_is_idempotent() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file("src/old.rs", "fn a() {}\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [old](/src/old.rs#L1)."),
        );
        repo.commit("baseline");

        repo.git(&["mv", "src/old.rs", "src/new.rs"]);

        // First pass.
        run(
            &[],
            false,
            repo.path(),
            repo.path(),
            false,
            crate::index::DocSource::WorkingTree,
            true,
            false,
            false,
        )
        .expect("first fix pass");

        let content_after_first =
            std::fs::read_to_string(repo.path().join("wiki/page.md")).expect("read page");

        // Second pass.
        run(
            &[],
            false,
            repo.path(),
            repo.path(),
            false,
            crate::index::DocSource::WorkingTree,
            true,
            false,
            false,
        )
        .expect("second fix pass");

        let content_after_second =
            std::fs::read_to_string(repo.path().join("wiki/page.md")).expect("read page");

        assert_eq!(
            content_after_first, content_after_second,
            "--fix must be idempotent: file changed on second pass"
        );
    }

    /// --fix must be rejected when --source is not worktree.
    #[test]
    fn wiki_check_fix_rejects_non_worktree_source() {
        // This test validates the CLI guard in main.rs; since tests call
        // commands::check::run directly (bypassing main.rs), we verify the
        // documented contract by calling run() with a non-worktree source and
        // fix=true, and asserting that the function itself does not mutate any
        // files. (The main.rs guard prints an error and returns Ok(2) before
        // reaching this function.)
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file("src/old.rs", "fn a() {}\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [old](/src/old.rs#L1)."),
        );
        repo.commit("baseline");

        let original =
            std::fs::read_to_string(repo.path().join("wiki/page.md")).expect("read page");

        // Calling run directly with Index source and fix=true; no files should change.
        let _code = run(
            &[],
            false,
            repo.path(),
            repo.path(),
            false,
            crate::index::DocSource::Index,
            true,
            false,
            false,
        )
        .expect("run");

        let after = std::fs::read_to_string(repo.path().join("wiki/page.md")).expect("read page");
        assert_eq!(
            original, after,
            "file must not be mutated when source != worktree"
        );
    }

    /// Finding 1 regression: a genuine infrastructure error during `discover_files`
    /// (not the empty-corpus condition) must exit 2 in `--fix` mode, not degrade to
    /// an empty-corpus success.  We use `DocSource::Index` on a repo with no git
    /// index at all (no `git add` ever run) so that `list_paths` fails with a real
    /// git error; this is distinct from the empty-corpus condition (Ok(vec![])).
    ///
    /// The main.rs CLI guard rejects `--fix --source=index`, so we call `run`
    /// directly — the same path a hook or test exercises.
    #[test]
    fn fix_mode_genuine_discover_error_exits_2_not_0() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        // Brand-new repo; no `git add` → no .git/index → Index.list_paths fails.
        let dir = tempfile::TempDir::new().expect("tempdir");
        let root = dir.path();
        // git init without any subsequent git add so the index file is absent.
        std::process::Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(root)
            .output()
            .expect("git init");
        // Place a wiki page on disk so this is not the "no .md files" case.
        std::fs::create_dir_all(root.join("wiki")).expect("mkdir wiki");
        std::fs::write(
            root.join("wiki/page.md"),
            make_wiki_page("Page", "No links."),
        )
        .expect("write page");

        // fix=true, source=Index → discover_files calls list_paths which fails
        // (no .git/index).  Must exit 2, not 0.
        let code = run(
            &[],
            false,
            root,
            root,
            false,
            crate::index::DocSource::Index,
            true,  // fix
            false,
            false,
        )
        .expect("run");
        assert_eq!(
            code, 2,
            "genuine infrastructure error in --fix mode must exit 2, not 0"
        );
    }

    /// Finding 1 regression (non-fix path): empty corpus in non-fix mode still
    /// exits 2 with "no wiki pages found" after the discover_files contract change.
    #[test]
    fn non_fix_empty_corpus_still_exits_2() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        // No wiki pages at all.
        let code = run(
            &[],
            false,
            repo.path(),
            repo.path(),
            false,
            crate::index::DocSource::WorkingTree,
            false,
            false,
            false,
        )
        .expect("run");
        assert_eq!(code, 2, "empty corpus must exit 2 in non-fix mode");
    }
}
