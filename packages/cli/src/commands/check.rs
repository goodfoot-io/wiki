use std::collections::HashMap;
use std::path::{Path, PathBuf};

use miette::Result;
use serde::Serialize;

use crate::commands::discover_files;
use crate::frontmatter::{Frontmatter, build_index, parse_frontmatter, parse_title};
use crate::git::resolve_ref;
use crate::headings::extract_headings;
use crate::index::DocSource;
use crate::parser::{LinkKind, parse_fragment_links, parse_wikilinks};
use crate::wiki_config::WikiConfig;

/// Read `path` from the chosen `DocSource`.
///
/// For `WorkingTree` this preserves today's behaviour (`fs::read_to_string`).
/// For `Index`/`Head`, the path is converted to a repo-relative form and read
/// through `DocSource::read`; absent paths surface as `Err(NotFound)` so
/// callers can keep their existing missing-file diagnostic flow.
fn read_via_source(
    path: &Path,
    repo_root: &Path,
    source: DocSource,
) -> std::io::Result<String> {
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

/// Return the repo-relative paths the chosen `DocSource` considers "present"
/// among the candidates produced by the worktree-default discovery.
///
/// For `WorkingTree`, the candidate list is returned unchanged.  For
/// `Index`/`Head`, the candidate list is filtered against the source's
/// `list_paths` so a worktree-only file does not appear under
/// `--source=index|head`.
fn filter_files_for_source(
    files: Vec<PathBuf>,
    repo_root: &Path,
    source: DocSource,
) -> Result<Vec<PathBuf>> {
    if matches!(source, DocSource::WorkingTree) {
        return Ok(files);
    }
    let listed: std::collections::HashSet<String> = source
        .list_paths(repo_root)?
        .into_iter()
        .collect();
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

use super::mesh_coverage;

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
///
/// ```text
/// Error: <Title Case Kind>
/// - <file>:<line>
/// - <message>
///
/// ---
///
/// ```
///
/// The `<file>:<line>` bullet is suppressed when the file is empty (e.g.
/// repo-wide diagnostics like `mesh_unavailable`).
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
#[allow(clippy::too_many_arguments)]
pub fn run(
    globs: &[String],
    json: bool,
    wiki_root: &Path,
    repo_root: &Path,
    wiki_config: Option<&WikiConfig>,
    no_exit_code: bool,
    no_mesh: bool,
    source: DocSource,
) -> Result<i32> {
    let files = match discover_files(globs, wiki_root, repo_root, source) {
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
        let raw = discover_files(&[], wiki_root, repo_root, source)?;
        filter_files_for_source(raw, repo_root, source)?
    };

    let diagnostics = match collect_for_files(&files, &index_files, wiki_root, repo_root, wiki_config, no_mesh, source) {
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
            print!("{}", format_diagnostic(&d.kind, &d.file, d.line, &d.message));
        }
    }

    if diagnostics.iter().any(|d| d.kind != "alias_resolve") && !no_exit_code {
        Ok(1)
    } else {
        Ok(0)
    }
}

/// Run `check` across multiple namespaces sequentially.
///
/// Each namespace is validated against its own `WikiIndex` (its own files,
/// its own peers per Phase 4 rules 5/6). Diagnostics are labeled with the
/// namespace they came from. Returns the worst exit code across runs.
pub fn run_multi(
    globs: &[String],
    json: bool,
    targets: &[(String, &Path)],
    repo_root: &Path,
    no_exit_code: bool,
    no_mesh: bool,
    source: DocSource,
) -> Result<i32> {
    let mut all: Vec<(String, Vec<CheckDiagnostic>)> = Vec::new();
    let mut runtime_error: Option<String> = None;

    for (label, wiki_root) in targets {
        // Load each namespace's own WikiConfig so rules 5/6 see that
        // namespace's own [peers] table.
        // Emit a diagnostic on load failure and continue — silent skip would
        // allow a green block even though rules 5/6 never ran for this namespace.
        let per_cfg = match WikiConfig::load(wiki_root, repo_root) {
            Ok(cfg) => Some(cfg),
            Err(e) => {
                all.push((label.clone(), vec![CheckDiagnostic {
                    kind: "namespace_config_invalid".into(),
                    file: wiki_root.display().to_string(),
                    line: 0,
                    message: format!(
                        "wiki.toml for namespace `{label}` could not be loaded: {e}. \
                         Rules 5/6 were not evaluated for this namespace."
                    ),
                }]));
                continue;
            }
        };
        let files = match discover_files(globs, wiki_root, repo_root, source) {
            Ok(f) => f,
            Err(e) => {
                runtime_error = Some(format!("[{label}] {e}"));
                break;
            }
        };
        let files = match filter_files_for_source(files, repo_root, source) {
            Ok(f) => f,
            Err(e) => {
                runtime_error = Some(format!("[{label}] {e}"));
                break;
            }
        };
        // When the user passed explicit globs, restrict to files inside this
        // namespace's wiki_root (keeping *.wiki.md files, which attach to a
        // namespace by frontmatter and can live anywhere in the repo).
        // Without this, an explicit file path is validated under every
        // namespace iteration, causing un-namespaced wikilinks to be reported
        // broken against indexes that can't possibly contain them.
        let files: Vec<PathBuf> = if globs.is_empty() {
            files
        } else {
            let canon_root = std::fs::canonicalize(wiki_root)
                .unwrap_or_else(|_| wiki_root.to_path_buf());
            files
                .into_iter()
                .filter(|p| {
                    p.to_string_lossy().ends_with(".wiki.md")
                        || std::fs::canonicalize(p)
                            .unwrap_or_else(|_| p.clone())
                            .starts_with(&canon_root)
                })
                .collect()
        };
        if files.is_empty() {
            continue;
        }
        let index_files = if globs.is_empty() {
            files.clone()
        } else {
            let raw = discover_files(&[], wiki_root, repo_root, source).unwrap_or_else(|_| files.clone());
            filter_files_for_source(raw, repo_root, source).unwrap_or_else(|_| files.clone())
        };
        match collect_for_files(&files, &index_files, wiki_root, repo_root, per_cfg.as_ref(), no_mesh, source) {
            Ok(d) => all.push((label.clone(), d)),
            Err(e) => {
                runtime_error = Some(format!("[{label}] {e}"));
                break;
            }
        }
    }

    if let Some(msg) = runtime_error {
        if json {
            eprintln!("{}", serde_json::json!({"error": msg}));
        } else {
            eprintln!("error: {msg}");
        }
        return Ok(2);
    }

    if json {
        let mut out: Vec<serde_json::Value> = Vec::new();
        for (label, diags) in &all {
            for d in diags {
                let mut v = serde_json::to_value(d).unwrap();
                if let Some(obj) = v.as_object_mut() {
                    obj.insert("namespace".into(), serde_json::json!(label));
                }
                out.push(v);
            }
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({ "errors": out })).unwrap()
        );
    } else {
        for (label, diags) in &all {
            for d in diags {
                let message = format!("[{label}] {}", d.message);
                print!("{}", format_diagnostic(&d.kind, &d.file, d.line, &message));
            }
        }
    }

    let any_error = all
        .iter()
        .flat_map(|(_, ds)| ds.iter())
        .any(|d| d.kind != "alias_resolve");
    Ok(if any_error && !no_exit_code { 1 } else { 0 })
}

/// Collect diagnostics for the given glob patterns without printing output.
///
/// Returns `Err` only on discovery failure; validation errors are returned as
/// diagnostics.  On discovery failure the caller should treat this as exit
/// code 2.
#[allow(dead_code)]
pub fn collect(globs: &[String], wiki_root: &Path, repo_root: &Path) -> Result<Vec<CheckDiagnostic>> {
    collect_with_config(globs, wiki_root, repo_root, None, DocSource::WorkingTree)
}

/// Collect diagnostics, with an optional `WikiConfig` for namespace rules 5 and 6.
pub fn collect_with_config(
    globs: &[String],
    wiki_root: &Path,
    repo_root: &Path,
    wiki_config: Option<&WikiConfig>,
    source: DocSource,
) -> Result<Vec<CheckDiagnostic>> {
    let files = discover_files(globs, wiki_root, repo_root, source)?;
    let files = filter_files_for_source(files, repo_root, source)?;
    let index_files = if globs.is_empty() {
        files.clone()
    } else {
        let raw = discover_files(&[], wiki_root, repo_root, source)?;
        filter_files_for_source(raw, repo_root, source)?
    };
    collect_for_files(&files, &index_files, wiki_root, repo_root, wiki_config, false, source)
}

fn collect_for_files(
    files: &[PathBuf],
    index_files: &[PathBuf],
    wiki_root: &Path,
    repo_root: &Path,
    wiki_config: Option<&WikiConfig>,
    no_mesh: bool,
    source: DocSource,
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
        if let Ok(c) = read_via_source(path, repo_root, source) {
            content_cache.insert(path.clone(), c);
        }
    }

    // ── Validate links in all files (including ones that failed frontmatter) ──
    for path in files {
        let content = match read_via_source(path, repo_root, source) {
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
            match read_via_source(&abs, repo_root, source) {
                Err(_) => {
                    // Directories are valid link targets and have no
                    // readable content under any source.
                    if abs.is_dir() {
                        continue;
                    }
                    // Check if this is a bare path that might be intended as repo-relative
                    let first = Path::new(&link.path).components().next();
                    let is_explicit = matches!(
                        first,
                        Some(std::path::Component::CurDir) | Some(std::path::Component::ParentDir)
                    );
                    let is_bare = !link.path.starts_with('/') && !is_explicit;

                    let message = if is_bare {
                        let repo_relative_abs = repo_root.join(&link.path);
                        if repo_relative_abs.exists() {
                            format!(
                                "File `{}` not found at page-relative path.\n\
                                 If you meant a repo-relative path, use `/{}` instead.",
                                link.path,
                                link.path
                            )
                        } else {
                            format!("File `{}` not found.", link.path)
                        }
                    } else {
                        format!("File `{}` not found.", link.path)
                    };
                    diagnostics.push(CheckDiagnostic {
                        kind: "missing_file".into(),
                        file: path.display().to_string(),
                        line: link.source_line,
                        message,
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
            // Cross-namespace wikilinks ([[ns:Article]]) are handled by rule 6;
            // skip them here to avoid spurious broken_wikilink diagnostics.
            if wl.namespace.is_some() {
                continue;
            }
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

    // ── Namespace rules 5 and 6 ───────────────────────────────────────────────
    if let Some(cfg) = wiki_config {
        // The "current" namespace is the one whose root is `wiki_root`.
        // Look it up from cfg.wikis so rules 5/6 know which wiki this run is for.
        let canon_wiki_root = std::fs::canonicalize(wiki_root)
            .unwrap_or_else(|_| wiki_root.to_path_buf());
        let current_ns = cfg
            .all()
            .find(|w| w.root == canon_wiki_root)
            .and_then(|w| w.namespace.as_deref());

        // Lazy per-namespace title-set cache for cross-namespace existence checks.
        let mut peer_title_cache: HashMap<String, Option<std::collections::HashSet<String>>> =
            HashMap::new();

        let known_namespaces: Vec<&str> =
            cfg.all().filter_map(|w| w.namespace.as_deref()).collect();

        for path in files {
            let content = match read_via_source(path, repo_root, source) {
                Ok(c) => c,
                Err(_) => continue,
            };

            // ── Rule 5: *.wiki.md with unknown namespace: frontmatter ─────────
            let path_str = path.to_string_lossy();
            if path_str.ends_with(".wiki.md")
                && let Ok(Some(fm)) = parse_frontmatter(&content, path)
                && let Some(declared_ns) = &fm.namespace
            {
                // Valid if it matches any known namespace in the repo.
                let is_known = cfg.wikis.contains_key(declared_ns.as_str());
                if !is_known {
                    diagnostics.push(CheckDiagnostic {
                        kind: "namespace_undeclared".into(),
                        file: path.display().to_string(),
                        line: 1,
                        message: format!(
                            "namespace `{declared_ns}` is not declared by any wiki.toml in this repo \
                             (current: {current}, known: [{known}]). Correct the namespace value or run `wiki init {declared_ns}` in the appropriate directory.",
                            current = current_ns.unwrap_or("default"),
                            known = known_namespaces.join(", "),
                        ),
                    });
                }
            }

            // ── Rule 6: [[ns:Article]] cross-namespace wikilinks ──────────────
            let wiki_links = parse_wikilinks(&content);
            for wl in &wiki_links {
                let ns = match &wl.namespace {
                    Some(ns) => ns,
                    None => continue, // same-namespace links handled by existing rule
                };

                // Is the namespace the current wiki itself?
                // [[current_ns:Article]] must validate against the current wiki's index.
                if current_ns == Some(ns.as_str()) {
                    let key = wl.title.to_lowercase();
                    if !index.contains_key(&key) && !invalid_titles.contains(&key) {
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
                    continue;
                }

                // Must be a known namespace in the repo.
                let peer_info = match cfg.wikis.get(ns.as_str()) {
                    Some(info) => info,
                    None => {
                        diagnostics.push(CheckDiagnostic {
                            kind: "cross_namespace_wikilink_unresolved".into(),
                            file: path.display().to_string(),
                            line: wl.source_line,
                            message: format!(
                                "namespace `{ns}` in `[[{ns}:{}]]` is not declared by any wiki.toml in this repo.",
                                wl.title
                            ),
                        });
                        continue;
                    }
                };

                // Lazily build/fetch the target wiki's title set.
                let title_set = peer_title_cache.entry(ns.clone()).or_insert_with(|| {
                    build_peer_title_set(&peer_info.root, repo_root, source)
                });

                let key = wl.title.to_lowercase();
                let exists = title_set
                    .as_ref()
                    .map(|s| s.contains(&key))
                    .unwrap_or(false);
                if !exists {
                    diagnostics.push(CheckDiagnostic {
                        kind: "cross_namespace_wikilink_unresolved".into(),
                        file: path.display().to_string(),
                        line: wl.source_line,
                        message: format!(
                            "`[[{ns}:{}]]` — no page with that title or alias exists in the `{ns}` namespace.",
                            wl.title
                        ),
                    });
                }
            }
        }
    }

    // ── Resolve ref to check git is callable (soft check, non-fatal) ─────────
    let _ = resolve_ref(repo_root, "HEAD");

    // ── Mesh coverage pass (skipped when --no-mesh) ───────────────────────────
    //
    // The mesh-coverage check shells out to `git-mesh` and reads files via the
    // worktree, so it is only meaningful under `--source=worktree`.  When the
    // user selected `--source=index|head` we skip mesh coverage explicitly
    // rather than silently mixing snapshot sources.
    if !no_mesh && matches!(source, DocSource::WorkingTree) {
        let mesh_diags = mesh_coverage::collect_mesh_diagnostics(files, repo_root)?;
        diagnostics.extend(mesh_diags);
    }

    Ok(diagnostics)
}

// ── Peer title-set builder ────────────────────────────────────────────────────

/// Lazily build a set of all lowercase title/alias keys for the given peer wiki
/// root. Returns `None` if the peer's files cannot be discovered or read
/// (treated as "title does not exist").
fn build_peer_title_set(
    peer_root: &Path,
    repo_root: &Path,
    source: DocSource,
) -> Option<std::collections::HashSet<String>> {
    let files = discover_files(&[], peer_root, repo_root, source).ok()?;
    let mut set = std::collections::HashSet::new();
    for path in &files {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        if let Ok(Some(fm)) = parse_frontmatter(&content, path) {
            set.insert(fm.title.to_lowercase());
            for alias in &fm.aliases {
                set.insert(alias.to_lowercase());
            }
        }
    }
    Some(set)
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
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file("wiki/page.md", &make_wiki_page("Page", "No links here."));
        repo.commit("add page");

        let code = run(&[], false, &wiki_root, repo.path(), None, false, false, crate::index::DocSource::WorkingTree).expect("run");
        assert_eq!(code, 0);
    }

    #[test]
    fn test_check_broken_wikilink_exit_1() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [[Nonexistent Page]]."),
        );
        repo.commit("add page");

        let code = run(&[], false, &wiki_root, repo.path(), None, false, false, crate::index::DocSource::WorkingTree).expect("run");
        assert_eq!(code, 1);
    }

    #[test]
    fn test_check_title_collision_exit_1() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file("wiki/a.md", &make_wiki_page("Shared", ""));
        repo.create_file("wiki/b.md", &make_wiki_page("Shared", ""));
        repo.commit("add pages");

        let code = run(&[], false, &wiki_root, repo.path(), None, false, false, crate::index::DocSource::WorkingTree).expect("run");
        assert_eq!(code, 1);
    }

    #[test]
    fn test_check_missing_frontmatter_exit_1() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file("wiki/page.md", "# Just a heading\n\nNo frontmatter.");
        repo.commit("add page");

        let code = run(&[], false, &wiki_root, repo.path(), None, false, false, crate::index::DocSource::WorkingTree).expect("run");
        assert_eq!(code, 1);
    }

    #[test]
    fn test_check_wikilink_via_alias_warns_exit_0() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file(
            "wiki/target.md",
            "---\ntitle: Target Page\naliases:\n  - tp\nsummary: The target page.\n---\n",
        );
        repo.create_file("wiki/source.md", &make_wiki_page("Source", "See [[tp]]."));
        repo.commit("add pages");

        let code = run(&[], false, &wiki_root, repo.path(), None, false, false, crate::index::DocSource::WorkingTree).expect("run");
        // alias_resolve warnings should not cause exit 1
        assert_eq!(code, 0);
    }

    #[test]
    fn test_check_heading_fragment_not_found_exit_1() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file(
            "wiki/target.md",
            &make_wiki_page("Target", "## Introduction\n"),
        );
        repo.create_file(
            "wiki/source.md",
            &make_wiki_page("Source", "See [[Target#Nonexistent]]."),
        );
        repo.commit("add pages");

        let code = run(&[], false, &wiki_root, repo.path(), None, false, false, crate::index::DocSource::WorkingTree).expect("run");
        assert_eq!(code, 1);
    }

    #[test]
    fn test_check_heading_fragment_found_exit_0() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file(
            "wiki/target.md",
            &make_wiki_page("Target", "## Introduction\n"),
        );
        repo.create_file(
            "wiki/source.md",
            &make_wiki_page("Source", "See [[Target#Introduction]]."),
        );
        repo.commit("add pages");

        let code = run(&[], false, &wiki_root, repo.path(), None, false, false, crate::index::DocSource::WorkingTree).expect("run");
        assert_eq!(code, 0);
    }

    #[test]
    fn test_check_glob_resolves_wikilinks_against_full_index() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        // Regression: passing a file path must not limit the index to that file only.
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file(
            "wiki/page_a.md",
            &make_wiki_page("Page A", "See [[Page B]]."),
        );
        repo.create_file("wiki/page_b.md", &make_wiki_page("Page B", "Target."));
        repo.commit("add pages");

        let globs = vec!["wiki/page_a.md".to_string()];
        let code = run(&globs, false, &wiki_root, repo.path(), None, false, false, crate::index::DocSource::WorkingTree).expect("run");
        assert_eq!(
            code, 0,
            "wikilink to a page outside the glob must resolve against the full wiki index"
        );
    }

    #[test]
    fn test_check_glob_still_reports_genuinely_missing_wikilinks() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file(
            "wiki/page_a.md",
            &make_wiki_page("Page A", "See [[Does Not Exist]]."),
        );
        repo.create_file("wiki/page_b.md", &make_wiki_page("Page B", "Unrelated."));
        repo.commit("add pages");

        let globs = vec!["wiki/page_a.md".to_string()];
        let code = run(&globs, false, &wiki_root, repo.path(), None, false, false, crate::index::DocSource::WorkingTree).expect("run");
        assert_eq!(
            code, 1,
            "a truly missing wikilink must still be reported when using a file glob"
        );
    }

    #[test]
    fn test_check_directory_link_is_valid() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file("src/lib.rs", "fn main() {}");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [src](/src/) for details."),
        );
        repo.commit("add files");

        let code = run(&[], false, &wiki_root, repo.path(), None, false, false, crate::index::DocSource::WorkingTree).expect("run");
        assert_eq!(
            code, 0,
            "directory fragment links must not produce missing_file"
        );
    }

    #[test]
    fn test_check_glob_does_not_report_collisions_outside_scope() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        // Collisions between pages not in the glob must not appear in the output.
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file("wiki/a.md", &make_wiki_page("Shared Title", ""));
        repo.create_file("wiki/b.md", &make_wiki_page("Shared Title", ""));
        repo.create_file("wiki/c.md", &make_wiki_page("Clean", "No issues here."));
        repo.commit("add pages");

        let globs = vec!["wiki/c.md".to_string()];
        let diagnostics = collect(&globs, &wiki_root, repo.path()).expect("collect");
        assert!(
            diagnostics.is_empty(),
            "collision between out-of-scope pages must not appear when checking an unrelated file: {diagnostics:?}"
        );
    }

    // ── Mesh coverage tests ───────────────────────────────────────────────────

    #[test]
    fn mesh_coverage_runs_without_opt_in() {
        // Regression: mesh coverage is always on. A wiki with an uncovered
        // fragment link must fail `wiki check`, with no flag.
        let _guard = PATH_MUTEX.lock().expect("path mutex");
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file("src/code.rs", "fn a() {}\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [code](/src/code.rs#L1-L1)."),
        );
        repo.commit("add files");

        let code = run(&[], false, &wiki_root, repo.path(), None, false, false, crate::index::DocSource::WorkingTree).expect("run");
        assert_eq!(code, 1, "uncovered fragment link must fail wiki check");
    }

    #[test]
    fn mesh_uncovered_link_exits_1() {
        let _guard = PATH_MUTEX.lock().expect("path mutex");
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file("src/code.rs", "fn a() {}\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [code](/src/code.rs#L1-L1)."),
        );
        repo.commit("add files");

        // No mesh created — link is uncovered
        let diagnostics = collect(&[], &wiki_root, repo.path()).expect("collect");
        let mesh_diags: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.kind == "mesh_uncovered")
            .collect();
        assert_eq!(mesh_diags.len(), 1, "expected one mesh_uncovered: {diagnostics:?}");
        let code = run(&[], false, &wiki_root, repo.path(), None, false, false, crate::index::DocSource::WorkingTree).expect("run");
        assert_eq!(code, 1);
    }

    #[test]
    fn mesh_covered_link_exits_0() {
        let _guard = PATH_MUTEX.lock().expect("path mutex");
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file("src/code.rs", "fn a() {}\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [code](/src/code.rs#L1-L1)."),
        );
        repo.commit("add files");

        // Create a mesh that anchors both the wiki file and the code file
        repo.git_mesh(&["add", "test-mesh", "wiki/page.md", "src/code.rs#L1-L1"]);
        repo.git_mesh(&["why", "test-mesh", "-m", "Links wiki page to code."]);
        repo.git_mesh(&["commit"]);

        let diagnostics = collect(&[], &wiki_root, repo.path()).expect("collect");
        let mesh_diags: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.kind == "mesh_uncovered")
            .collect();
        assert!(
            mesh_diags.is_empty(),
            "covered link must not produce mesh_uncovered: {diagnostics:?}"
        );
        let code = run(&[], false, &wiki_root, repo.path(), None, false, false, crate::index::DocSource::WorkingTree).expect("run");
        assert_eq!(code, 0);
    }

    #[test]
    fn mesh_covers_code_but_not_wiki_file() {
        let _guard = PATH_MUTEX.lock().expect("path mutex");
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file("src/code.rs", "fn a() {}\n");
        repo.create_file("src/other.rs", "fn b() {}\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [code](/src/code.rs#L1-L1)."),
        );
        repo.commit("add files");

        // Mesh anchors code file and a different file — not the wiki page
        repo.git_mesh(&["add", "test-mesh", "src/other.rs", "src/code.rs#L1-L1"]);
        repo.git_mesh(&["why", "test-mesh", "-m", "Code only mesh."]);
        repo.git_mesh(&["commit"]);

        let diagnostics = collect(&[], &wiki_root, repo.path()).expect("collect");
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
        let _guard = PATH_MUTEX.lock().expect("path mutex");
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file("src/code.rs", "fn a() {}\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [code](/src/code.rs#L1-L1)."),
        );
        repo.commit("add files");

        // Whole-file anchor on code.rs should match any ranged query against it
        repo.git_mesh(&["add", "test-mesh", "wiki/page.md", "src/code.rs"]);
        repo.git_mesh(&["why", "test-mesh", "-m", "Whole-file anchor."]);
        repo.git_mesh(&["commit"]);

        let diagnostics = collect(&[], &wiki_root, repo.path()).expect("collect");
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
        let _guard = PATH_MUTEX.lock().expect("path mutex");
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        // Create a file with at least 20 lines
        let content: String = (1..=20).map(|i| format!("fn line_{i}() {{}}\n")).collect();
        repo.create_file("src/code.rs", &content);
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [code](/src/code.rs#L1-L1)."),
        );
        repo.commit("add files");

        // Mesh covers L10-L20, but link is L1-L1 — should NOT cover
        repo.git_mesh(&["add", "test-mesh", "wiki/page.md", "src/code.rs#L10-L20"]);
        repo.git_mesh(&["why", "test-mesh", "-m", "Different range."]);
        repo.git_mesh(&["commit"]);

        let diagnostics = collect(&[], &wiki_root, repo.path()).expect("collect");
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
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file("src/code.rs", "fn a() {}\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [code](/src/code.rs)."),
        );
        repo.commit("add files");

        // No mesh — but link has no range so it should be skipped
        let diagnostics = collect(&[], &wiki_root, repo.path()).expect("collect");
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
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [external](https://example.com/file.rs#L1-L5)."),
        );
        repo.commit("add files");

        let diagnostics = collect(&[], &wiki_root, repo.path()).expect("collect");
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
    fn mailto_link_does_not_produce_missing_file() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "Contact [us](mailto:someone@example.com)."),
        );
        repo.commit("add files");

        let diagnostics = collect(&[], &wiki_root, repo.path()).expect("collect");
        let missing_file: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.kind == "missing_file")
            .collect();
        assert!(
            missing_file.is_empty(),
            "mailto: links must not produce missing_file: {diagnostics:?}"
        );
    }

    #[test]
    fn mesh_unavailable_emits_warning_and_exits_1() {
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file("src/code.rs", "fn a() {}\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [code](/src/code.rs#L1-L1)."),
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
            let result = collect(&[], &wiki_root, repo.path());
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
            let code = run(&[], false, &wiki_root, repo.path(), None, false, false, crate::index::DocSource::WorkingTree).expect("run");
            // SAFETY: PATH_MUTEX is held.
            unsafe { std::env::set_var("PATH", &original_path) };
            code
        };
        assert_eq!(code, 1, "mesh_unavailable must cause exit 1 (fail closed)");
    }

    #[test]
    fn mesh_caches_per_anchor() {
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file("src/code.rs", "fn a() {}\n");
        // Two wiki pages that both link to the same code anchor
        repo.create_file(
            "wiki/page_a.md",
            &make_wiki_page("Page A", "See [code](/src/code.rs#L1-L1)."),
        );
        repo.create_file(
            "wiki/page_b.md",
            &make_wiki_page("Page B", "See [code](/src/code.rs#L1-L1)."),
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

        let _diagnostics = collect(&[], &wiki_root, repo.path()).expect("collect");

        // SAFETY: PATH_MUTEX is held.
        unsafe { std::env::set_var("PATH", &original_path) };

        // Read the counter — git-mesh ls should be called exactly once regardless of anchor count
        let count_str = fs::read_to_string(&counter_path).unwrap_or_else(|_| "0".to_string());
        let count: u32 = count_str.trim().parse().unwrap_or(0);
        assert_eq!(
            count, 1,
            "git-mesh ls must be called exactly once total (bulk fetch), got {count}"
        );
    }

    #[test]
    fn mesh_runtime_error_exits_2() {
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file("src/code.rs", "fn a() {}\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [code](/src/code.rs#L1-L1)."),
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
        let code = run(&[], false, &wiki_root, repo.path(), None, false, false, crate::index::DocSource::WorkingTree).expect("run");
        // SAFETY: PATH_MUTEX is held.
        unsafe { std::env::set_var("PATH", &original_path) };

        assert_eq!(code, 2, "git-mesh non-zero exit must produce exit code 2 (runtime error)");
    }

    /// Finding 2 regression: `--source=index` must validate the staged
    /// content, not the worktree.  Worktree is clean, but the index has a
    /// broken wikilink — `wiki check --source=index` must report it.
    #[test]
    fn check_source_index_validates_staged_broken_when_worktree_clean() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        // Commit a clean baseline.
        repo.create_file("wiki/page.md", &make_wiki_page("Page", "No links."));
        repo.commit("clean baseline");

        // Stage a broken edit, then restore the worktree to the clean version.
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [[Nonexistent]]."),
        );
        repo.git(&["add", "wiki/page.md"]);
        repo.create_file("wiki/page.md", &make_wiki_page("Page", "No links."));

        // --source=worktree: clean.
        let diags_wt = collect_with_config(
            &[],
            &wiki_root,
            repo.path(),
            None,
            crate::index::DocSource::WorkingTree,
        )
        .expect("collect wt");
        assert!(
            diags_wt.iter().all(|d| d.kind == "alias_resolve"),
            "worktree should be clean, got: {:?}",
            diags_wt
        );

        // --source=index: should see the broken wikilink staged.
        let diags_idx = collect_with_config(
            &[],
            &wiki_root,
            repo.path(),
            None,
            crate::index::DocSource::Index,
        )
        .expect("collect idx");
        assert!(
            diags_idx.iter().any(|d| d.kind == "broken_wikilink"),
            "index should see staged broken wikilink, got: {:?}",
            diags_idx
        );
    }

    /// Finding 3 regression: under `--source=index|head`, glob discovery
    /// must filter the source's path list — never walk the worktree.  A
    /// worktree-only `.md` matched by the glob must NOT appear.
    #[test]
    fn check_source_index_glob_does_not_read_worktree() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file("wiki/staged.md", &make_wiki_page("Staged", "ok."));
        repo.git(&["add", "wiki/staged.md"]);
        // Worktree-only file matches the glob but is not in the index.
        repo.create_file("wiki/worktree_only.md", &make_wiki_page("Worktree", "ok."));

        let globs = vec!["wiki/**/*.md".to_string()];
        let diags = collect_with_config(
            &globs,
            &wiki_root,
            repo.path(),
            None,
            crate::index::DocSource::Index,
        )
        .expect("collect idx");
        assert!(
            diags
                .iter()
                .all(|d| !d.message.contains("worktree_only.md")
                    && !d.file.contains("worktree_only.md")),
            "--source=index glob discovery must not surface worktree-only files: {diags:?}"
        );
    }

    /// Finding 4 + 5 regression: discovery errors under `--source=head` on
    /// an unborn HEAD must propagate rather than being silently substituted
    /// with the worktree candidate set or masked into an empty cache key.
    #[test]
    fn check_source_head_unborn_propagates_error() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        // Worktree has files but HEAD is unborn (no commits).
        repo.create_file("wiki/page.md", &make_wiki_page("Page", "ok."));

        let result = collect_with_config(
            &[],
            &wiki_root,
            repo.path(),
            None,
            crate::index::DocSource::Head,
        );
        assert!(
            result.is_err(),
            "unborn HEAD under --source=head must surface as an error"
        );
    }

    /// Finding 2 regression (inverse): `--source=head` must ignore worktree
    /// edits.  The worktree has a broken wikilink, but HEAD is clean —
    /// `wiki check --source=head` must report no errors.
    #[test]
    fn check_source_head_ignores_worktree_only_breakage() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file("wiki/page.md", &make_wiki_page("Page", "No links."));
        repo.commit("clean HEAD");

        // Worktree: introduce a broken link without staging it.
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [[Nonexistent]]."),
        );

        let diags_head = collect_with_config(
            &[],
            &wiki_root,
            repo.path(),
            None,
            crate::index::DocSource::Head,
        )
        .expect("collect head");
        assert!(
            diags_head.iter().all(|d| d.kind == "alias_resolve"),
            "HEAD should be clean, got: {:?}",
            diags_head
        );
    }
}
