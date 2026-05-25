//! `wiki scaffold` end-to-end pipeline.
//!
//! Discover wiki files, parse their fragment links, and create git meshes
//! covering those links via `git mesh add`. Use `--dry-run` to preview the
//! plan as markdown without mutating `.mesh/`. Use `--format json` for a
//! structured non-mutating output.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use miette::{IntoDiagnostic, Result};
use regex::Regex;
use serde::Serialize;

use crate::commands::{discover_files, resolve_link_path};
use crate::index::DocSource;
use crate::parser::{LinkKind, parse_fragment_links};

/// Read `path` from the chosen [`DocSource`], routing non-worktree reads
/// through [`DocSource::read`] so the content snapshot matches the discovery
/// snapshot.
fn read_via_source(path: &Path, repo_root: &Path, source: DocSource) -> std::io::Result<String> {
    match source {
        DocSource::WorkingTree => fs::read_to_string(path),
        DocSource::Index | DocSource::Head => {
            let path_rel = path
                .strip_prefix(repo_root)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| path.to_string_lossy().into_owned());
            match source.read(repo_root, &path_rel) {
                Ok(Some(s)) => Ok(s),
                Ok(None) => Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("{path_rel} not present in source {source:?}"),
                )),
                Err(e) => Err(std::io::Error::other(e.to_string())),
            }
        }
    }
}

// ── Parse-error types ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub(crate) enum ParseErrorKind {
    /// `title:` present, value empty/whitespace.
    EmptyTitle,
    /// IO error or invalid UTF-8 — message captured.
    Unreadable(String),
    /// Starts with `---` but regex rejected it (BOM, CRLF, no closing fence, etc.).
    Malformed,
}

#[derive(Debug, Clone)]
pub(crate) struct ParseError {
    pub(crate) path: String,
    pub(crate) kind: ParseErrorKind,
}

// ── JSON output types ─────────────────────────────────────────────────────────

/// Why a draft was dropped pre-apply.
///
/// Both variants are *fixable wiki conditions* surfaced as advisories — they
/// must NOT escalate to a non-zero exit, otherwise a routine drifted wiki link
/// would lock the repository via the fail-closed pre-commit hook.
#[derive(Debug, Clone)]
pub(crate) enum DropReason {
    /// An anchor's code target file is absent in the active source.
    MissingPath { path: String },
    /// An anchor's code target is gitignored — a build artifact that is never
    /// committed. `git mesh add` refuses such a path (it resolves content
    /// through git and cannot track a path git never sees), so anchoring it
    /// would either fail the add or produce a permanently-`deleted` anchor.
    /// Skipped with an advisory rather than escalated. Distinct from
    /// untracked-but-not-ignored targets, which resolve once committed and are
    /// left to anchor normally.
    IgnoredPath { path: String },
    /// An anchor is statically invalid for git-mesh: line range exceeds the
    /// target file's line count, start > end, or start < 1.
    InvalidAnchor { anchor: String, detail: String },
    /// The generated slug cannot be created because a pre-existing mesh
    /// occupies a conflicting path (an ancestor file, or a directory at the
    /// slug path). `existing` is the conflicting path relative to the mesh
    /// dir, forward slashes.
    SlugPathCollision { existing: String },
}

/// A mesh that was dropped pre-apply (missing target or invalid anchor).
#[derive(Debug, Clone)]
pub(crate) struct DroppedMesh {
    pub(crate) slug: String,
    pub(crate) reason: DropReason,
    pub(crate) page: String,
}

/// Machine-stable dropped-mesh category tags.
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum DroppedMeshCategory {
    MissingPath,
    IgnoredPath,
    InvalidAnchor,
    SlugPathCollision,
}

/// JSON representation of a dropped mesh.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DroppedMeshJson {
    slug: String,
    category: DroppedMeshCategory,
    /// The missing path (missing-path drops) or the offending anchor
    /// (invalid-anchor drops).
    anchor: String,
    /// Human-readable reason; for missing paths this is empty.
    detail: String,
    page: String,
}

/// Top-level JSON output for `wiki scaffold --format json`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ScaffoldOutput {
    schema_version: u32,
    parse_errors: Vec<ParseErrorJson>,
    dropped_meshes: Vec<DroppedMeshJson>,
    pages: Vec<PageJson>,
}

/// JSON representation of a parse error.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ParseErrorJson {
    path: String,
    category: ParseErrorCategory,
    message: String,
}

/// Machine-stable parse-error category tags.
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum ParseErrorCategory {
    EmptyTitle,
    Unreadable,
    MalformedFrontmatter,
}

/// JSON representation of a per-page section.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PageJson {
    path: String,
    title: String,
    meshes: Vec<MeshJson>,
}

/// JSON representation of one mesh entry.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MeshJson {
    slug: String,
    heading_chain: Vec<String>,
    anchors: Vec<AnchorJson>,
}

/// JSON representation of a structured anchor.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AnchorJson {
    path: String,
    start_line: u32,
    end_line: u32,
}

impl ParseErrorCategory {
    fn from_kind(kind: &ParseErrorKind) -> Self {
        match kind {
            ParseErrorKind::EmptyTitle => ParseErrorCategory::EmptyTitle,
            ParseErrorKind::Unreadable(_) => ParseErrorCategory::Unreadable,
            ParseErrorKind::Malformed => ParseErrorCategory::MalformedFrontmatter,
        }
    }
}

impl ParseErrorKind {
    pub(crate) fn reason(&self) -> String {
        match self {
            ParseErrorKind::EmptyTitle => "frontmatter present but `title:` is empty".to_string(),
            ParseErrorKind::Unreadable(msg) => format!("file could not be read: {msg}"),
            ParseErrorKind::Malformed => {
                "malformed frontmatter — could not parse `title`".to_string()
            }
        }
    }
}

use super::augment::{AugmentedLink, augment};
use super::draft::{self, MeshDraft};
use super::group;
use super::render;

/// Run the `wiki scaffold` subcommand.
pub fn run(
    globs: &[String],
    json: bool,
    dry_run: bool,
    repo_root: &Path,
    source: crate::index::DocSource,
    print_applied: bool,
) -> Result<i32> {
    let files = match discover_files(globs, repo_root, source) {
        Ok(v) => v
            .into_iter()
            .filter(|f| {
                let s = f.to_string_lossy();
                !s.contains("/tests/fixtures/") && !s.contains("\\tests\\fixtures\\")
            })
            .collect::<Vec<_>>(),
        Err(e) => {
            if e.to_string().contains("no wiki pages found") {
                Vec::new()
            } else {
                return Err(e);
            }
        }
    };

    // Coverage index: filter out fragment links already covered by a mesh in
    // the repo. Fail closed when `git-mesh` is unavailable — emitting unfiltered
    // `git mesh add` blocks would silently regenerate meshes that already exist.
    let mesh_index = match crate::commands::mesh_coverage::build_mesh_index(repo_root, &files) {
        Ok(Some(idx)) => idx,
        Ok(None) => {
            eprintln!(
                "wiki scaffold: mesh_unavailable — `git-mesh` is not on PATH, refusing to scaffold without coverage data. Install git-mesh and retry; see https://github.com/goodfoot-io/git-mesh."
            );
            return Ok(1);
        }
        Err(e) => return Err(e),
    };

    let mut all_inputs: Vec<LinkInput> = Vec::new();
    for file in &files {
        let content = match read_via_source(file, repo_root, source) {
            Ok(s) => s,
            Err(_) => {
                // Unreadable files are surfaced via parse_errors (classify_frontmatter
                // records ParseErrorKind::Unreadable independently). Skip from the
                // link pipeline.
                continue;
            }
        };
        let raw_links = parse_fragment_links(&content);
        let augmented = augment(&raw_links, &content);
        // Filter to internal links with a parsed line range — mirrors the JS
        // which skips URL-scheme links and links lacking `#`.
        for aug in augmented {
            if aug.link.kind != LinkKind::Internal {
                continue;
            }
            if aug.link.start_line.is_none() {
                continue;
            }
            all_inputs.push(LinkInput {
                wiki_file: file.clone(),
                augmented: aug,
            });
        }
    }

    // Build per-source frontmatter map (title) keyed by absolute path,
    // and accumulate parse errors for source files.
    let mut wiki_meta_cache: std::collections::HashMap<PathBuf, FileMeta> =
        std::collections::HashMap::new();
    let mut parse_errors: Vec<ParseError> = Vec::new();
    for f in &files {
        let (meta, err_kind) = classify_frontmatter(f, repo_root, source);
        if let Some(kind) = err_kind {
            let rel = path_relative_to(f, repo_root);
            parse_errors.push(ParseError { path: rel, kind });
        }
        wiki_meta_cache.insert(f.clone(), meta);
    }
    parse_errors.sort_by(|a, b| a.path.cmp(&b.path));

    // Build the page-title lookup keyed by repo-root-relative path strings.
    let mut page_titles: std::collections::HashMap<String, Option<String>> =
        std::collections::HashMap::new();
    // Parallel map: per-page slug subdir (directory of page relative to repo_root,
    // forward slashes).
    let mut page_subdirs: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for f in &files {
        let rel = path_relative_to(f, repo_root);
        let title = wiki_meta_cache.get(f).and_then(|m| m.title.clone());
        page_titles.insert(rel.clone(), title);
        let subdir = resolve_page_subdir(f, repo_root);
        page_subdirs.insert(rel, subdir);
    }

    // Collect parse-error paths for exclusion from pages output.
    let parse_error_paths: std::collections::HashSet<String> =
        parse_errors.iter().map(|e| e.path.clone()).collect();

    // ── Unified build/group pipeline (both modes) ─────────────────────────
    // Trim heading chains once here so both renderers consume pre-trimmed data.
    let mut consolidated = build_meshes(&all_inputs, repo_root, &page_subdirs);
    trim_chains_in_place(&mut consolidated, &page_titles);

    // Section-extension pass: when a wiki section already has an associated
    // mesh (some mesh M anchors the exact `(page_path, section_start,
    // section_end)` triple), every new code link added in that section becomes
    // an extension of M instead of a brand-new mesh. Drafts whose new code
    // anchors are *all* already in M are dropped; drafts with remaining new
    // anchors switch to `extends_existing = Some(M)`.
    apply_section_extension(&mut consolidated, &mesh_index);

    // Resolve slug collisions across both already-assigned slugs in this run
    // and any meshes that already live in the repo. Extension drafts opt out:
    // they reuse the existing mesh's slug verbatim and skip the probe entirely.
    let mesh_dir = resolve_mesh_dir(repo_root);
    let probe = |slug: &str| {
        mesh_exists(repo_root, slug) || mesh_fs_prefix_collision(&mesh_dir, slug)
    };
    resolve_slug_collisions(&mut consolidated, &page_titles, &probe);

    // Drop meshes whose non-wiki anchors reference paths that don't exist in
    // the active source.
    let source_paths: Option<std::collections::HashSet<String>> = match source {
        crate::index::DocSource::WorkingTree => None, // check filesystem inline
        crate::index::DocSource::Index | crate::index::DocSource::Head => Some(
            source
                .list_paths(repo_root)
                .unwrap_or_default()
                .into_iter()
                .collect(),
        ),
    };
    let mut dropped_meshes: Vec<DroppedMesh> = Vec::new();

    // ── Slug-path-collision pass ──────────────────────────────────────────
    // A pre-existing mesh occupying a conflicting path makes `git mesh add`
    // structurally impossible. When the collision is a strict ANCESTOR FILE
    // (existing shorter mesh `B` blocks new longer slug `B/...`), the remedy
    // is to RENAME the blocker out of the way (apply mode) / report the
    // planned rename (dry-run), keeping the new draft. The blocker is read and
    // its single distinct wiki-page anchor drives a reused-noun leaf for the
    // rename target. Any failure (non-ancestor-file collision, unreadable
    // blocker, `git mesh move` non-zero) FAILS OPEN to the legacy
    // drop-with-advisory path (exit 0) — never fail-closed, never panic.
    //
    // Extension drafts reuse an existing slug verbatim, so they are exempt.
    let section_noun: std::collections::HashMap<(String, u32, u32), String> =
        consolidated
            .iter()
            .filter_map(|d| {
                d.structured_anchors.first().map(|a| {
                    (
                        (a.path.clone(), a.start_line, a.end_line),
                        d.noun.clone(),
                    )
                })
            })
            .collect();
    let mut planned_renames: Vec<PlannedRename> = Vec::new();
    consolidated.retain(|draft| {
        if draft.extends_existing.is_some()
            || !mesh_fs_prefix_collision(&mesh_dir, &draft.slug)
        {
            return true;
        }
        // Try the rename-the-blocker remedy for the ancestor-file case.
        match plan_blocker_rename(
            &mesh_dir,
            &draft.slug,
            &draft.page_path,
            &section_noun,
        ) {
            Some(plan) if dry_run => {
                // Preview only — never mutate. Keep the draft.
                planned_renames.push(plan);
                true
            }
            Some(plan) if run_blocker_rename(repo_root, &mesh_dir, &plan) => {
                // Path freed — keep the draft; `apply_drafts` will create it.
                planned_renames.push(plan);
                true
            }
            // Fail open: not an ancestor-file case, unreadable blocker, or
            // `git mesh move` failed → drop-with-advisory, exit 0.
            _ => {
                dropped_meshes.push(DroppedMesh {
                    slug: draft.slug.clone(),
                    reason: DropReason::SlugPathCollision {
                        existing: mesh_fs_collision_path(&mesh_dir, &draft.slug),
                    },
                    page: draft.page_path.clone(),
                });
                false
            }
        }
    });

    // A gitignored fragment-link target (a generated build artifact) is exempt
    // from mesh coverage exactly as `wiki check` treats it: git-mesh refuses to
    // anchor a path git never sees, so demanding coverage would be
    // unsatisfiable. Strip ONLY the gitignored anchors from each draft (with a
    // per-anchor advisory), keeping the section's co-cited *tracked* anchors
    // covered; drop a draft entirely only when no code anchor remains. An
    // untracked-but-not-ignored target is NOT stripped — it resolves once
    // committed. Index/Head sources already exclude ignored paths via
    // `source_paths`; we still consult `git check-ignore` in all modes so the
    // advisory can name the gitignore cause.
    //
    // `extends_existing` drafts have already had their leading page-section
    // anchor removed by `apply_section_extension`, so for them every entry is a
    // code anchor (`code_start == 0`); new-mesh drafts keep the page section
    // anchor at index 0 (`code_start == 1`).
    fn code_anchor_start(draft: &super::draft::MeshDraft) -> usize {
        usize::from(draft.extends_existing.is_none())
    }
    let candidate_anchor_paths: Vec<String> = consolidated
        .iter()
        .flat_map(|d| d.structured_anchors.iter().skip(code_anchor_start(d)))
        .map(|a| a.path.clone())
        .collect();
    let ignored_anchor_paths = crate::git::ignored_paths(repo_root, &candidate_anchor_paths)?;

    consolidated.retain_mut(|draft| {
        let code_start = code_anchor_start(draft);
        // Preserve the page-section prefix (none for extension drafts), then
        // copy across every code anchor that is not gitignored.
        let mut kept_anchors = draft.anchors[..code_start].to_vec();
        let mut kept_struct = draft.structured_anchors[..code_start].to_vec();
        let mut stripped = false;
        for (a_str, a) in draft
            .anchors
            .iter()
            .skip(code_start)
            .zip(draft.structured_anchors.iter().skip(code_start))
        {
            if ignored_anchor_paths.contains(&a.path) {
                dropped_meshes.push(DroppedMesh {
                    slug: draft.slug.clone(),
                    reason: DropReason::IgnoredPath {
                        path: a.path.clone(),
                    },
                    page: draft.page_path.clone(),
                });
                stripped = true;
            } else {
                kept_anchors.push(a_str.clone());
                kept_struct.push(a.clone());
            }
        }
        if stripped {
            draft.anchors = kept_anchors;
            draft.structured_anchors = kept_struct;
        }
        // Drop the draft only when stripping left it with no code anchor.
        draft.structured_anchors.len() > code_start
    });

    consolidated.retain(|draft| {
        let code_start = code_anchor_start(draft);
        // Check every code anchor (skip the page section anchor on new drafts).
        for anchor in draft.structured_anchors.iter().skip(code_start) {
            let missing = match &source_paths {
                None => {
                    let abs = repo_root.join(&anchor.path);
                    !abs.is_file()
                }
                Some(paths) => !paths.contains(&anchor.path),
            };
            if missing {
                dropped_meshes.push(DroppedMesh {
                    slug: draft.slug.clone(),
                    reason: DropReason::MissingPath {
                        path: anchor.path.clone(),
                    },
                    page: draft.page_path.clone(),
                });
                return false;
            }
            // The path exists — statically validate the anchor's line range
            // against the target file before it ever reaches `git mesh add`.
            // An over-range / inverted / zero-start anchor is a drifted wiki
            // link (a fixable wiki condition), NOT a hard build failure: drop
            // it with a named advisory and let scaffold exit 0 so the
            // fail-closed pre-commit hook does not lock the whole repository.
            if let Some(detail) =
                invalid_anchor_detail(repo_root, anchor, source, &source_paths)
            {
                dropped_meshes.push(DroppedMesh {
                    slug: draft.slug.clone(),
                    reason: DropReason::InvalidAnchor {
                        anchor: format!(
                            "{}#L{}-L{}",
                            anchor.path, anchor.start_line, anchor.end_line
                        ),
                        detail,
                    },
                    page: draft.page_path.clone(),
                });
                return false;
            }
        }
        true
    });

    if json {
        let parse_errors_json: Vec<ParseErrorJson> = parse_errors
            .iter()
            .map(|e| ParseErrorJson {
                path: e.path.clone(),
                category: ParseErrorCategory::from_kind(&e.kind),
                message: e.kind.reason(),
            })
            .collect();
        let dropped_meshes_json: Vec<DroppedMeshJson> = dropped_meshes
            .iter()
            .map(|d| match &d.reason {
                DropReason::MissingPath { path } => DroppedMeshJson {
                    slug: d.slug.clone(),
                    category: DroppedMeshCategory::MissingPath,
                    anchor: path.clone(),
                    detail: String::new(),
                    page: d.page.clone(),
                },
                DropReason::IgnoredPath { path } => DroppedMeshJson {
                    slug: d.slug.clone(),
                    category: DroppedMeshCategory::IgnoredPath,
                    anchor: path.clone(),
                    detail: "anchor target is gitignored".to_string(),
                    page: d.page.clone(),
                },
                DropReason::InvalidAnchor { anchor, detail } => DroppedMeshJson {
                    slug: d.slug.clone(),
                    category: DroppedMeshCategory::InvalidAnchor,
                    anchor: anchor.clone(),
                    detail: detail.clone(),
                    page: d.page.clone(),
                },
                DropReason::SlugPathCollision { existing } => DroppedMeshJson {
                    slug: d.slug.clone(),
                    category: DroppedMeshCategory::SlugPathCollision,
                    anchor: existing.clone(),
                    detail: format!(
                        "slug path collides with existing mesh `{existing}`"
                    ),
                    page: d.page.clone(),
                },
            })
            .collect();
        let pages = build_pages_json(&consolidated, &page_titles, &parse_error_paths);
        let output = ScaffoldOutput {
            schema_version: 1,
            parse_errors: parse_errors_json,
            dropped_meshes: dropped_meshes_json,
            pages,
        };
        let s = serde_json::to_string_pretty(&output).into_diagnostic()?;
        println!("{s}");
        return Ok(0);
    }

    // ── Dry-run / apply mode ──────────────────────────────────────────────
    // Empty when there were no internal fragment links at all OR when every
    // section already has its links anchored by an existing mesh.
    if all_inputs.is_empty() || consolidated.is_empty() {
        let mut out = render::render_empty_markdown(&parse_errors, &dropped_meshes);
        render::render_rename_advisories(&mut out, &planned_renames, dry_run);
        // `--print-applied` contracts stdout to be exactly the list of
        // repo-relative applied mesh paths. With nothing applied, stdout must
        // be empty; the human-readable advisory belongs on stderr.
        if print_applied {
            eprint!("{out}");
        } else {
            print!("{out}");
        }
        return Ok(0);
    }

    if dry_run {
        // The planned-rename block is part of the non-mutating preview; it
        // precedes the mesh plan like other advisories.
        let mut rename_block = String::new();
        render::render_rename_advisories(&mut rename_block, &planned_renames, true);
        let rendered = render::render_markdown(
            &consolidated,
            &page_titles,
            &parse_errors,
            &parse_error_paths,
            &dropped_meshes,
        );
        print!("{rename_block}{rendered}");
        return Ok(0);
    }

    // Default: apply each draft via `git mesh add`.
    // Print advisories (parse errors, dropped meshes, performed renames)
    // before applying so the caller can see what changed, even when not in
    // dry-run mode.
    if !parse_errors.is_empty()
        || !dropped_meshes.is_empty()
        || !planned_renames.is_empty()
    {
        let mut advisory = String::new();
        render::render_advisories(&mut advisory, &parse_errors, &dropped_meshes, true);
        render::render_rename_advisories(&mut advisory, &planned_renames, false);
        if print_applied {
            // Advisories must not pollute stdout when the caller is consuming
            // stdout as an exact stage list.
            eprint!("{advisory}");
        } else {
            print!("{advisory}");
        }
    }

    // When `print_applied`, every mesh file this run touched must be emitted
    // on stdout so the pre-commit hook stages it — including each renamed
    // blocker's NEW path.
    if print_applied {
        for r in &planned_renames {
            let rel = match mesh_dir.strip_prefix(repo_root) {
                Ok(d) => {
                    let d = d.to_string_lossy().replace('\\', "/");
                    if d.is_empty() {
                        r.to.clone()
                    } else {
                        format!("{d}/{}", r.to)
                    }
                }
                Err(_) => mesh_dir.join(&r.to).to_string_lossy().replace('\\', "/"),
            };
            println!("{rel}");
        }
    }

    apply_drafts(&consolidated, repo_root, &mesh_dir, print_applied)
}

/// Apply each `MeshDraft` by invoking `git mesh add <slug> <anchors…>`.
///
/// Extension drafts (`extends_existing.is_some()`) re-use the existing slug;
/// git-mesh 1.0.80 appends anchors idempotently.
///
/// Fail-closed: stops on the first non-zero exit and returns `Ok(1)`.
/// On failure, reports the slugs already applied in this run so the partial
/// mutation is disclosed rather than silent.
///
/// When `print_applied` is set, each successfully applied mesh's repo-relative
/// path (`.mesh/<slug>`, forward slashes) is printed to stdout, one per line.
/// This is an additional fail-closed guard: if the deterministic mesh file is
/// absent on disk after a successful `git mesh add`, that is treated as a hard
/// failure (error to stderr, returns `Ok(1)`) rather than silently emitting an
/// incomplete stage list to the caller.
fn apply_drafts(
    drafts: &[MeshDraft],
    repo_root: &Path,
    mesh_dir: &Path,
    print_applied: bool,
) -> Result<i32> {
    let mut applied: Vec<String> = Vec::new();

    for draft in drafts {
        let slug = draft
            .extends_existing
            .as_deref()
            .unwrap_or(draft.slug.as_str());

        let mut args: Vec<&str> = vec!["mesh", "add", slug];
        let anchor_strs: Vec<String> = draft.anchors.clone();
        for a in &anchor_strs {
            args.push(a.as_str());
        }

        // Inherit stderr so git-mesh's failure reason — and GIT_MESH_PERF=1
        // timing lines — print directly rather than being buffered here.
        let status = Command::new("git")
            .args(&args)
            .current_dir(repo_root)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .status()
            .into_diagnostic()?;

        if !status.success() {
            // git-mesh already printed its reason to the inherited stderr; the
            // partial-mutation disclosure is emitted on its own line so it
            // never runs together with that reason.
            if applied.is_empty() {
                eprintln!("wiki scaffold: `git mesh add {slug}` failed");
            } else {
                let already = applied.join(", ");
                eprintln!(
                    "wiki scaffold: `git mesh add {slug}` failed\n\
                     already created this run: {already}; aborted at {slug}"
                );
            }
            return Ok(1);
        }

        if print_applied {
            let mesh_path = mesh_dir.join(slug);
            let rel = match mesh_dir.strip_prefix(repo_root) {
                Ok(d) => {
                    let d = d.to_string_lossy().replace('\\', "/");
                    if d.is_empty() {
                        slug.to_string()
                    } else {
                        format!("{d}/{slug}")
                    }
                }
                Err(_) => mesh_path.to_string_lossy().replace('\\', "/"),
            };
            if !mesh_path.is_file() {
                eprintln!(
                    "wiki scaffold: `git mesh add {slug}` reported success but expected mesh file {rel} is absent — refusing to emit an incomplete stage list"
                );
                return Ok(1);
            }
            println!("{rel}");
        }

        applied.push(slug.to_string());
    }
    Ok(0)
}

/// Build the JSON page list from consolidated drafts, excluding pages whose
/// paths appear in `parse_error_paths` (schema must be disjoint).
fn build_pages_json(
    drafts: &[MeshDraft],
    page_titles: &std::collections::HashMap<String, Option<String>>,
    parse_error_paths: &std::collections::HashSet<String>,
) -> Vec<PageJson> {
    // Group by page in first-occurrence order.
    let mut page_order: Vec<String> = Vec::new();
    let mut by_page: std::collections::HashMap<String, Vec<&MeshDraft>> =
        std::collections::HashMap::new();
    for d in drafts {
        if parse_error_paths.contains(&d.page_path) {
            continue;
        }
        if !by_page.contains_key(&d.page_path) {
            page_order.push(d.page_path.clone());
        }
        by_page.entry(d.page_path.clone()).or_default().push(d);
    }

    page_order
        .into_iter()
        .map(|page_path| {
            let title = page_titles
                .get(&page_path)
                .and_then(|t| t.clone())
                .unwrap_or_default();
            let page_drafts = by_page.get(&page_path).expect("tracked");
            let meshes = page_drafts
                .iter()
                .map(|d| {
                    // heading_chain was already trimmed once in trim_chains_in_place.
                    MeshJson {
                        slug: d.slug.clone(),
                        heading_chain: d.heading_chain.clone(),
                        anchors: d
                            .structured_anchors
                            .iter()
                            .map(|a| AnchorJson {
                                path: a.path.clone(),
                                start_line: a.start_line,
                                end_line: a.end_line,
                            })
                            .collect(),
                    }
                })
                .collect();
            PageJson {
                path: page_path,
                title,
                meshes,
            }
        })
        .collect()
}

/// Walk consolidated drafts in place and rewrite each one whose wiki section
/// anchor is already carried by an existing mesh into an *extension* of that
/// mesh.
///
/// For each draft, the leading entry in `structured_anchors` is the page
/// section anchor `(page_path, section_start, section_end)`. When some mesh M
/// anchors that exact triple, the draft is converted: code anchors already in
/// M are dropped, the section anchor itself is dropped, `slug` is overwritten
/// with M's name, and `extends_existing = Some(M)` flags the renderer to emit
/// `git mesh add M ...` with no `git mesh why` line.
///
/// Drafts left with no remaining code anchors are filtered out entirely —
/// nothing new for the user to commit.
fn apply_section_extension(
    drafts: &mut Vec<MeshDraft>,
    mesh_index: &crate::commands::mesh_coverage::MeshIndex,
) {
    drafts.retain_mut(|d| {
        // The page-section anchor is the leading structured anchor by
        // construction in `draft::build`. If a draft somehow lacks one, leave
        // it as a normal new-mesh draft.
        let Some(section_anchor) = d.structured_anchors.first().cloned() else {
            return true;
        };
        let owning = mesh_index
            .owning_mesh_for_exact(
                Path::new(&section_anchor.path),
                section_anchor.start_line,
                section_anchor.end_line,
            )
            .map(|s| s.to_string());
        let Some(mesh_name) = owning else {
            return true;
        };

        // Pair the parallel `anchors` strings with `structured_anchors` and
        // drop (a) the leading section anchor itself, and (b) any code anchor
        // already carried by the owning mesh.
        let paired: Vec<(String, super::draft::StructuredAnchor)> = d
            .anchors
            .iter()
            .cloned()
            .zip(d.structured_anchors.iter().cloned())
            .collect();
        let kept: Vec<(String, super::draft::StructuredAnchor)> = paired
            .into_iter()
            .enumerate()
            .filter_map(|(idx, (a_str, a_struct))| {
                if idx == 0 {
                    return None; // section anchor
                }
                if mesh_index.mesh_contains_anchor(
                    &mesh_name,
                    Path::new(&a_struct.path),
                    a_struct.start_line,
                    a_struct.end_line,
                ) {
                    return None;
                }
                Some((a_str, a_struct))
            })
            .collect();

        if kept.is_empty() {
            return false; // nothing new; drop the draft
        }

        let (new_anchors, new_struct): (Vec<String>, Vec<super::draft::StructuredAnchor>) =
            kept.into_iter().unzip();
        d.anchors = new_anchors;
        d.structured_anchors = new_struct;
        d.slug = mesh_name.clone();
        d.extends_existing = Some(mesh_name);
        true
    });
}

/// Trim heading chains on all drafts in place. The leading chain entry is
/// dropped when it matches the page's frontmatter title after normalization.
/// This runs once after `build_meshes` so both renderers consume pre-trimmed data.
fn trim_chains_in_place(
    drafts: &mut [MeshDraft],
    page_titles: &std::collections::HashMap<String, Option<String>>,
) {
    for d in drafts.iter_mut() {
        let title = page_titles
            .get(&d.page_path)
            .and_then(|t| t.as_deref())
            .unwrap_or("");
        d.heading_chain = trim_heading_chain(&d.heading_chain, title);
    }
}

/// Trim the leading entry of `heading_chain` when it matches the page's
/// frontmatter `title` after normalization (strip inline markup, collapse
/// whitespace, case-insensitive compare). Returns the trimmed chain.
pub(crate) fn trim_heading_chain(chain: &[String], page_title: &str) -> Vec<String> {
    if chain.is_empty() {
        return Vec::new();
    }
    let normalized_title = normalize_heading_text(page_title);
    let normalized_first = normalize_heading_text(&chain[0]);
    if !normalized_title.is_empty() && normalized_first.eq_ignore_ascii_case(&normalized_title) {
        chain[1..].to_vec()
    } else {
        chain.to_vec()
    }
}

/// Normalize heading or title text for comparison: strip inline markup chars
/// (`*`, `_`, `` ` ``, `[`, `]`), collapse whitespace.
pub(crate) fn normalize_heading_text(s: &str) -> String {
    let stripped: String = s.chars().filter(|c| !"`*_[]".contains(*c)).collect();
    stripped.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Three-stage build/group/annotate pipeline that produces the final list of
/// meshes (in per-page declaration order) ready for shell rendering.
fn build_meshes(
    inputs: &[LinkInput],
    repo_root: &Path,
    page_subdirs: &std::collections::HashMap<String, String>,
) -> Vec<MeshDraft> {
    // Group inputs by source page (preserving discovery order).
    let mut page_order: Vec<PathBuf> = Vec::new();
    let mut by_page: std::collections::HashMap<PathBuf, Vec<&LinkInput>> =
        std::collections::HashMap::new();
    for input in inputs {
        if !by_page.contains_key(&input.wiki_file) {
            page_order.push(input.wiki_file.clone());
        }
        by_page
            .entry(input.wiki_file.clone())
            .or_default()
            .push(input);
    }

    // Stage 1: per-page section grouping → one draft per section.
    let mut all_drafts: Vec<MeshDraft> = Vec::new();
    let mut page_spans: Vec<(usize, usize)> = Vec::with_capacity(page_order.len());
    for page in &page_order {
        let entries = by_page.get(page).expect("page tracked in order");
        let page_rel = path_relative_to(page, repo_root);

        // Group entries by (section_start, section_end) preserving first-occurrence order.
        let mut section_order: Vec<(u32, u32)> = Vec::new();
        let mut by_section: std::collections::HashMap<(u32, u32), Vec<&LinkInput>> =
            std::collections::HashMap::new();
        for entry in entries {
            let key = (
                entry.augmented.section_start_line,
                entry.augmented.section_end_line,
            );
            if !by_section.contains_key(&key) {
                section_order.push(key);
            }
            by_section.entry(key).or_default().push(entry);
        }

        type GroupTuple<'a> = (
            &'a AugmentedLink,
            u32,
            u32,
            Vec<String>,
            Vec<draft::StructuredAnchor>,
        );
        let mut groups_storage: Vec<GroupTuple<'_>> = Vec::with_capacity(section_order.len());
        for key in &section_order {
            let section_entries = by_section.get(key).expect("tracked");
            let leader = &section_entries[0].augmented;
            let mut seen: std::collections::HashSet<(String, u32, u32)> =
                std::collections::HashSet::new();
            let mut target_anchors: Vec<String> = Vec::new();
            let mut structured_targets: Vec<draft::StructuredAnchor> = Vec::new();
            for entry in section_entries {
                let link = &entry.augmented.link;
                let resolved = resolve_link_path(&link.path, &entry.wiki_file, repo_root);
                let anchor_rel = path_relative_to(&resolved, repo_root);
                let anchor_rel =
                    locate_existing_suffix(&anchor_rel, repo_root).unwrap_or(anchor_rel);
                let start = link.start_line.unwrap_or(0);
                let end = link.end_line.unwrap_or(start);
                let triple = (anchor_rel.clone(), start, end);
                if !seen.insert(triple) {
                    continue;
                }
                target_anchors.push(format!("{anchor_rel}#L{start}-L{end}"));
                structured_targets.push(draft::StructuredAnchor {
                    path: anchor_rel,
                    start_line: start,
                    end_line: end,
                });
            }
            groups_storage.push((leader, key.0, key.1, target_anchors, structured_targets));
        }
        let groups: Vec<draft::SectionGroup<'_>> = groups_storage
            .iter()
            .map(|(leader, s, e, ta, st)| draft::SectionGroup {
                leader,
                section_start: *s,
                section_end: *e,
                target_anchors: ta.clone(),
                structured_targets: st.clone(),
            })
            .collect();
        // Look up the owning wiki for slug derivation. A page should always
        // be in the map (every discovered file is registered above), but fall
        // back to an empty subdir so a missing entry can never panic — the
        // slug still gets the `wiki/` prefix.
        let page_subdir = page_subdirs.get(&page_rel).cloned().unwrap_or_default();
        let drafts = draft::build(&page_rel, &groups, repo_root, &page_subdir);
        let start = all_drafts.len();
        all_drafts.extend(drafts);
        page_spans.push((start, all_drafts.len()));
    }

    // Stage 2: per-page consolidation. Identical-anchor-set siblings collapse
    // into one survivor; only then does the collision resolver (run from
    // [`run`] after heading-chain trimming) see contiguous slugs. Doing this
    // in the reverse order leaks suffix gaps (`foo`, `foo-3`, no `foo-2`)
    // into the footer's `git mesh commit` lines whenever consolidation
    // prunes a duplicate the dedup already suffixed.
    let mut consolidated: Vec<MeshDraft> = Vec::new();
    for (start, end) in page_spans {
        let page_drafts: Vec<MeshDraft> = all_drafts[start..end].to_vec();
        consolidated.extend(group::consolidate_within_page(page_drafts));
    }

    consolidated
}

/// Resolve the git-mesh storage directory for `repo_root`.
///
/// Precedence (highest first):
/// 1. `GIT_MESH_DIR` environment variable.
/// 2. `git config --get git-mesh.dir` (non-zero exit / empty value = unset).
/// 3. Default `.mesh`.
///
/// A relative resolved value is joined onto `repo_root`; an absolute value is
/// used as-is. `wiki` never passes `--mesh-dir`, so that tier is intentionally
/// not modeled here.
fn resolve_mesh_dir(repo_root: &Path) -> PathBuf {
    let configured = std::env::var("GIT_MESH_DIR")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| {
            Command::new("git")
                .args(["config", "--get", "git-mesh.dir"])
                .current_dir(repo_root)
                .stderr(Stdio::null())
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| {
                    let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                    if s.is_empty() { None } else { Some(s) }
                })
        });

    match configured {
        Some(v) => {
            let p = PathBuf::from(&v);
            if p.is_absolute() {
                p
            } else {
                repo_root.join(p)
            }
        }
        None => repo_root.join(".mesh"),
    }
}

/// Probe whether a mesh with `slug` already exists in `repo_root`.
///
/// git-mesh stores every mesh as a tracked working-tree file at
/// `<mesh_dir>/<name>`, where `<name>` is the slug with `/` as real nested
/// directory separators. Existence is therefore a pure filesystem check: the
/// mesh exists iff `<mesh_dir>/<slug>` is a regular file. No `git mesh`
/// subprocess is involved.
fn mesh_exists(repo_root: &Path, slug: &str) -> bool {
    resolve_mesh_dir(repo_root).join(slug).is_file()
}

/// True if slugs `a` and `b` cannot both exist as nested mesh files: they are
/// equal, or one is a strict segment-wise path prefix of the other.
///
/// Segment-aware: `wiki/scaffold` and `wiki/scaffold-2` do NOT conflict, but
/// `wiki/scaffold` and `wiki/scaffold/extra` do.
fn slug_paths_conflict(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let starts_with_segs = |long: &str, short: &str| {
        long.starts_with(short)
            && long.as_bytes().get(short.len()) == Some(&b'/')
    };
    starts_with_segs(a, b) || starts_with_segs(b, a)
}

/// True if creating `<mesh_dir>/<slug>` as a regular file is structurally
/// impossible because:
/// - `<mesh_dir>/<slug>` already exists as a directory, OR
/// - any STRICT ancestor `<mesh_dir>/<seg1>/…/<segK>` (K < total segments)
///   exists as a regular file.
///
/// An exact pre-existing file at `<mesh_dir>/<slug>` is NOT this function's
/// concern — that case belongs to [`mesh_exists`].
fn mesh_fs_prefix_collision(mesh_dir: &Path, slug: &str) -> bool {
    let full = mesh_dir.join(slug);
    if full.is_dir() {
        return true;
    }
    let segs: Vec<&str> = slug.split('/').filter(|s| !s.is_empty()).collect();
    let mut cur = mesh_dir.to_path_buf();
    for seg in segs.iter().take(segs.len().saturating_sub(1)) {
        cur = cur.join(seg);
        if cur.is_file() {
            return true;
        }
    }
    false
}

/// Compute the conflicting ancestor mesh path (relative to `mesh_dir`, forward
/// slashes) for a slug that fails [`mesh_fs_prefix_collision`]: the first
/// strict ancestor that exists as a regular file, or `slug` itself if
/// `<mesh_dir>/<slug>` is an existing directory.
fn mesh_fs_collision_path(mesh_dir: &Path, slug: &str) -> String {
    if mesh_dir.join(slug).is_dir() {
        return slug.to_string();
    }
    let segs: Vec<&str> = slug.split('/').filter(|s| !s.is_empty()).collect();
    let mut cur = mesh_dir.to_path_buf();
    let mut acc: Vec<&str> = Vec::new();
    for seg in segs.iter().take(segs.len().saturating_sub(1)) {
        cur = cur.join(seg);
        acc.push(seg);
        if cur.is_file() {
            return acc.join("/");
        }
    }
    slug.to_string()
}

/// The first STRICT ancestor of `slug` that exists as a regular file under
/// `mesh_dir`, returned as a forward-slash mesh-dir-relative name. `None` when
/// the collision (if any) is a directory at the slug path rather than an
/// ancestor file — only the ancestor-file case is eligible for the
/// rename-the-blocker remedy.
fn ancestor_file_blocker(mesh_dir: &Path, slug: &str) -> Option<String> {
    if mesh_dir.join(slug).is_dir() {
        return None;
    }
    let segs: Vec<&str> = slug.split('/').filter(|s| !s.is_empty()).collect();
    let mut cur = mesh_dir.to_path_buf();
    let mut acc: Vec<&str> = Vec::new();
    for seg in segs.iter().take(segs.len().saturating_sub(1)) {
        cur = cur.join(seg);
        acc.push(seg);
        if cur.is_file() {
            return Some(acc.join("/"));
        }
    }
    None
}

/// Parsed view of a tracked mesh file: the distinct wiki-page anchors (RELPATH
/// ends `.md`) it carries, as `(path, start, end)` triples.
fn parse_mesh_wiki_anchors(text: &str) -> Vec<(String, u32, u32)> {
    let mut out: Vec<(String, u32, u32)> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            // Blank line ends the anchor block; the rest is `why` prose.
            break;
        }
        // `RELPATH#L<start>-L<end> sha256:<hex>` or whole-file
        // `RELPATH sha256:<hex>`.
        let first = line.split_whitespace().next().unwrap_or("");
        let (relpath, range) = match first.split_once("#L") {
            Some((p, r)) => (p, Some(r)),
            None => (first, None),
        };
        if !relpath.ends_with(".md") {
            continue;
        }
        let (start, end) = match range {
            None => (0u32, 0u32),
            Some(r) => match r.split_once("-L") {
                Some((s, e)) => match (s.parse::<u32>(), e.parse::<u32>()) {
                    (Ok(s), Ok(e)) => (s, e),
                    _ => continue,
                },
                None => continue,
            },
        };
        let triple = (relpath.to_string(), start, end);
        if !out.contains(&triple) {
            out.push(triple);
        }
    }
    out
}

/// A rename of a blocker mesh that frees a slug path for a new draft.
#[derive(Debug, Clone)]
pub(crate) struct PlannedRename {
    /// Mesh-dir-relative old name of the blocker (e.g. `wiki/arch/scaff`).
    pub(crate) from: String,
    /// Mesh-dir-relative new name (e.g. `wiki/arch/scaff/index`).
    pub(crate) to: String,
    /// Slug of the new draft whose path the rename frees.
    pub(crate) for_slug: String,
    /// Wiki page that motivated `for_slug` (for the advisory).
    pub(crate) page: String,
}

/// Derive the leaf segment for a blocker rename target `B/<leaf>` from the
/// blocker's single distinct wiki-page anchor, reusing the exact noun the
/// scaffold pipeline already derived for that page section (looked up in
/// `section_noun` keyed by `(page, start, end)`).
///
/// Returns the chosen mesh-dir-relative target `B/<leaf>`. Falls back to
/// `B/index` (then `B/index-2`, … capped at 99) when the leaf is empty, equals
/// `B`'s last segment, or `B/<leaf>` would itself collide.
fn derive_rename_target(
    mesh_dir: &Path,
    blocker: &str,
    wiki_anchors: &[(String, u32, u32)],
    section_noun: &std::collections::HashMap<(String, u32, u32), String>,
) -> String {
    let blocker_last = blocker.rsplit('/').next().unwrap_or(blocker);

    let leaf: Option<String> = if wiki_anchors.len() == 1 {
        let key = &wiki_anchors[0];
        section_noun
            .get(key)
            .map(|n| draft::kebab(n))
            .filter(|s| !s.is_empty())
    } else {
        None
    };

    if let Some(leaf) = leaf
        && leaf != blocker_last
    {
        let cand = format!("{blocker}/{leaf}");
        if !mesh_dir.join(&cand).is_file()
            && !mesh_fs_prefix_collision(mesh_dir, &cand)
        {
            return cand;
        }
    }

    // Numeric `index` fallback.
    let base = format!("{blocker}/index");
    if !mesh_dir.join(&base).is_file() && !mesh_fs_prefix_collision(mesh_dir, &base)
    {
        return base;
    }
    for n in 2..=99 {
        let cand = format!("{blocker}/index-{n}");
        if !mesh_dir.join(&cand).is_file() && !mesh_fs_prefix_collision(mesh_dir, &cand)
        {
            return cand;
        }
    }
    base
}

/// Plan a blocker rename for `slug` whose creation is blocked by a strict
/// ancestor mesh FILE. Reads and parses the blocker; returns the planned
/// rename, or `None` to signal fail-open (fall back to drop-with-advisory):
/// the collision is not an ancestor-file case, the blocker is unreadable, or
/// derivation otherwise cannot proceed.
fn plan_blocker_rename(
    mesh_dir: &Path,
    slug: &str,
    page: &str,
    section_noun: &std::collections::HashMap<(String, u32, u32), String>,
) -> Option<PlannedRename> {
    let blocker = ancestor_file_blocker(mesh_dir, slug)?;
    let text = fs::read_to_string(mesh_dir.join(&blocker)).ok()?;
    let wiki_anchors = parse_mesh_wiki_anchors(&text);
    let target = derive_rename_target(mesh_dir, &blocker, &wiki_anchors, section_noun);
    Some(PlannedRename {
        from: blocker,
        to: target,
        for_slug: slug.to_string(),
        page: page.to_string(),
    })
}

/// Run one `git mesh move <old> <new>` with cwd `repo_root`. `true` iff the
/// subprocess spawned and exited zero.
fn git_mesh_move(repo_root: &Path, old: &str, new: &str) -> bool {
    matches!(
        Command::new("git")
            .args(["mesh", "move", old, new])
            .current_dir(repo_root)
            .stdout(Stdio::null())
            // Inherit stderr so git-mesh diagnostics and GIT_MESH_PERF=1 timing
            // lines surface instead of being captured and discarded.
            .stderr(Stdio::inherit())
            .status(),
        Ok(s) if s.success()
    )
}

/// Execute a planned blocker rename.
///
/// The rename target `<B>/<leaf>` is always a strict DESCENDANT of the blocker
/// `<B>`, and `git mesh move` cannot turn the existing file `<B>` directly
/// into the directory `<B>/…` in one step. So the move is performed in two
/// hops through a fresh, collision-free temporary mesh name that is *not*
/// under `<B>`: `B → tmp`, then `tmp → B/target`. If the second hop fails the
/// first is rolled back (`tmp → B`) so the blocker is left exactly as found.
///
/// Returns `true` only when the blocker ends up at `plan.to`. Any spawn /
/// non-zero exit yields `false` (caller fails open to drop-with-advisory) with
/// the blocker restored to its original name.
fn run_blocker_rename(repo_root: &Path, mesh_dir: &Path, plan: &PlannedRename) -> bool {
    // A unique temp name at the mesh root, never under `plan.from`.
    let flat: String = plan
        .from
        .chars()
        .map(|c| if c == '/' { '-' } else { c })
        .collect();
    let mut tmp = format!("wiki-scaffold-tmp-{flat}");
    let mut n = 2;
    while mesh_dir.join(&tmp).exists() || mesh_fs_prefix_collision(mesh_dir, &tmp) {
        tmp = format!("wiki-scaffold-tmp-{flat}-{n}");
        n += 1;
        if n > 99 {
            return false;
        }
    }

    if !git_mesh_move(repo_root, &plan.from, &tmp) {
        return false;
    }
    if !git_mesh_move(repo_root, &tmp, &plan.to) {
        // Roll back so the blocker is byte-identical at its original name.
        let _ = git_mesh_move(repo_root, &tmp, &plan.from);
        return false;
    }
    true
}

/// Resolve slug collisions by progressively prepending semantic qualifiers
/// drawn from the section's heading chain and the page title.
///
/// Each draft starts with its base slug. If that slug collides with either
/// an earlier-assigned slug in this run or a pre-existing mesh (`mesh_exists`
/// returns `true`), the resolver tries successively longer qualifier sets:
///
/// 1. The immediate parent heading (`heading_chain.last()`), then grandparent,
///    great-grandparent, … each new heading prepended outer→inner so the slug
///    reads top-down like a path.
/// 2. After the heading chain is exhausted, the page's frontmatter title
///    (kebab-cased) is prepended ahead of the full chain.
/// 3. Only when *all* semantic qualifiers fail does the resolver fall back to
///    numeric `-2`, `-3`, … suffixes appended to the last unique semantic
///    candidate.
///
/// Duplicate candidate slugs (caused by [`draft::build_slug_with_qualifiers`]
/// dropping a reserved qualifier) are skipped so the search makes monotonic
/// progress instead of looping on the same string.
/// True if `cand` would collide path-wise with any already-assigned slug
/// (exact equality or a strict segment-wise prefix relationship), since two
/// such slugs cannot coexist as nested mesh files.
fn assigned_conflict(assigned: &std::collections::HashSet<String>, cand: &str) -> bool {
    assigned.iter().any(|a| slug_paths_conflict(a, cand))
}

pub(crate) fn resolve_slug_collisions(
    drafts: &mut [MeshDraft],
    page_titles: &std::collections::HashMap<String, Option<String>>,
    mesh_exists: &dyn Fn(&str) -> bool,
) {
    let mut assigned: std::collections::HashSet<String> = std::collections::HashSet::new();
    for d in drafts.iter_mut() {
        // Extension drafts reuse the existing mesh's slug verbatim — they are
        // not new meshes, so they participate in neither the assigned set nor
        // the probe.
        if d.extends_existing.is_some() {
            continue;
        }
        // Inner→outer chain in kebab form, dropping empties and any entry
        // equal to the noun (the deepest entry in `heading_chain` is the
        // section heading itself, which already supplied the noun — using it
        // as a qualifier would just re-emit `wiki/foo/foo`).
        let chain_kebab: Vec<String> = d
            .heading_chain
            .iter()
            .map(|h| draft::kebab(h))
            .filter(|s| !s.is_empty() && s != &d.noun)
            .collect();
        let title_kebab = page_titles
            .get(&d.page_path)
            .and_then(|t| t.as_deref())
            .filter(|t| !t.is_empty())
            .map(draft::kebab)
            .filter(|s| !s.is_empty() && s != &d.noun);

        // Candidate qualifier sets to try, in priority order.
        let mut candidates: Vec<Vec<String>> = Vec::new();
        candidates.push(Vec::new());
        for k in 1..=chain_kebab.len() {
            let slice = &chain_kebab[chain_kebab.len() - k..];
            candidates.push(slice.to_vec());
        }
        if let Some(title) = &title_kebab {
            let mut with_title: Vec<String> = vec![title.clone()];
            with_title.extend(chain_kebab.iter().cloned());
            candidates.push(with_title);
        }

        let mut tried: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut last_unique: Option<String> = None;
        let mut resolved: Option<String> = None;
        for quals in &candidates {
            let slug = draft::build_slug_with_qualifiers(&d.page_subdir, quals, &d.noun);
            if !tried.insert(slug.clone()) {
                continue;
            }
            last_unique = Some(slug.clone());
            if !assigned_conflict(&assigned, &slug) && !mesh_exists(&slug) {
                resolved = Some(slug);
                break;
            }
        }
        let final_slug = resolved.unwrap_or_else(|| {
            let base = last_unique.unwrap_or_else(|| d.slug.clone());
            // Cap iterations: if `base` has a pre-existing ANCESTOR file every
            // `base-N` still prefix-collides forever. When exhausted, return
            // `base` unchanged (still-colliding) — the pre-apply retain pass
            // drops it with a SlugPathCollision advisory.
            (2..=99)
                .map(|n| format!("{base}-{n}"))
                .find(|cand| !assigned_conflict(&assigned, cand) && !mesh_exists(cand))
                .unwrap_or(base)
        });
        assigned.insert(final_slug.clone());
        d.slug = final_slug;
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

struct LinkInput {
    wiki_file: PathBuf,
    augmented: AugmentedLink,
}

#[derive(Debug, Clone, Default)]
struct FileMeta {
    title: Option<String>,
}

/// Resolve a page's directory relative to repo_root (minus the filename).
/// Returns the parent directory as a forward-slash string, or empty string
/// if the page is directly in the repo root.
pub(crate) fn resolve_page_subdir(page_abs: &Path, repo_root: &Path) -> String {
    let rel = page_abs.strip_prefix(repo_root).unwrap_or(page_abs);
    let parent = rel.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    parent.to_string_lossy().replace('\\', "/")
}

/// Classify the frontmatter of a file, returning both the `FileMeta` and an
/// optional `ParseErrorKind` if the file's `title` could not be extracted.
fn classify_frontmatter(
    path: &Path,
    repo_root: &Path,
    source: DocSource,
) -> (FileMeta, Option<ParseErrorKind>) {
    let text = match read_via_source(path, repo_root, source) {
        Ok(s) => s,
        Err(e) => {
            return (
                FileMeta::default(),
                Some(ParseErrorKind::Unreadable(e.to_string())),
            );
        }
    };

    // Step 2: must start with `---\n` or `---\r\n`. A file without a
    // frontmatter block is a valid plain markdown page — it simply has no
    // extractable title, so return the default `FileMeta` with no error.
    if !text.starts_with("---\n") && !text.starts_with("---\r\n") {
        return (FileMeta::default(), None);
    }

    // Step 3: locate closing `---` fence.
    let after_open = text
        .strip_prefix("---\r\n")
        .or_else(|| text.strip_prefix("---\n"))
        .unwrap_or(&text[4..]);
    let has_closing_fence = after_open
        .lines()
        .any(|l| l.trim_end_matches('\r') == "---");
    if !has_closing_fence {
        return (FileMeta::default(), Some(ParseErrorKind::Malformed));
    }

    // Step 4: look for `title:` line inside the fenced block.
    // Collect lines between the two `---` fences.
    let lines: Vec<&str> = after_open.lines().collect();
    let closing_idx = lines.iter().position(|l| l.trim_end_matches('\r') == "---");
    let fm_lines = match closing_idx {
        Some(i) => &lines[..i],
        None => &lines[..],
    };

    let title_line = fm_lines
        .iter()
        .find(|l| l.starts_with("title:") || l.starts_with("title :"));

    if title_line.is_none() {
        // A frontmatter block without a `title:` key is valid — the page
        // simply has no extractable title and is treated as a plain page.
        return (FileMeta::default(), None);
    }

    // Check if the value is empty/whitespace.
    let raw_value = title_line
        .unwrap()
        .split_once(':')
        .map(|(_, v)| v)
        .unwrap_or("")
        .trim();
    let stripped_value = raw_value
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| {
            raw_value
                .strip_prefix('\'')
                .and_then(|s| s.strip_suffix('\''))
        })
        .unwrap_or(raw_value)
        .trim();

    if stripped_value.is_empty() {
        return (FileMeta::default(), Some(ParseErrorKind::EmptyTitle));
    }

    // Step 5: run parse_frontmatter_field — if it returns None despite a
    // non-empty title line, the frontmatter is malformed (BOM, CRLF, etc.).
    let title = parse_frontmatter_field(&text, "title");
    if title.is_none() {
        return (FileMeta::default(), Some(ParseErrorKind::Malformed));
    }

    let meta = FileMeta { title };
    (meta, None)
}

fn parse_frontmatter_field(content: &str, field: &str) -> Option<String> {
    // Only parse if the file starts with `---\n`. JS uses /^---\s*\n(?:.*\n)*?title:\s*(.+?)\s*\n/.
    // Anchor to file start (\A) so a thematic-break `---` later in the body does not
    // match — that was the JS prototype's intent.
    let pat = format!(r"\A---\s*\n(?:.*\n)*?{field}:\s*(.+?)\s*\n");
    let re = Regex::new(&pat).ok()?;
    let cap = re.captures(content)?;
    let raw = cap.get(1)?.as_str().trim();
    let stripped = raw
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| raw.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
        .unwrap_or(raw);
    Some(stripped.trim().to_string())
}

fn path_relative_to(path: &Path, repo_root: &Path) -> String {
    let rel = path.strip_prefix(repo_root).unwrap_or(path);
    rel.to_string_lossy().replace('\\', "/")
}

/// Statically validate a structured anchor's line range against the target
/// file the same way git-mesh would, *before* `git mesh add` is invoked.
///
/// Returns `Some(detail)` describing why the anchor is invalid (mirroring
/// git-mesh's own diagnostics), or `None` when the anchor is acceptable.
/// `None` here means "let it through" — a genuine `git mesh add` failure on
/// an otherwise-valid anchor must still fail closed downstream.
fn invalid_anchor_detail(
    repo_root: &Path,
    anchor: &draft::StructuredAnchor,
    source: DocSource,
    source_paths: &Option<std::collections::HashSet<String>>,
) -> Option<String> {
    let (start, end) = (anchor.start_line, anchor.end_line);
    // start < 1 (git-mesh uses 1-based inclusive line numbers).
    if start < 1 {
        return Some(format!("start line {start} is below 1"));
    }
    // Inverted / zero-length range.
    if start > end {
        return Some(format!("start line {start} exceeds end line {end}"));
    }
    // Over-range: end beyond the file's line count.
    let content = if source_paths.is_none() {
        // WorkingTree — read from disk.
        fs::read_to_string(repo_root.join(&anchor.path)).ok()?
    } else {
        // Index / Head — read the snapshot so the count matches discovery.
        let abs = repo_root.join(&anchor.path);
        read_via_source(&abs, repo_root, source).ok()?
    };
    let line_count = count_lines(&content);
    if u64::from(end) > line_count {
        return Some(format!(
            "end exceeds file line count {line_count}"
        ));
    }
    None
}

/// Count the lines in `content` the way git-mesh does: a trailing newline does
/// not introduce a phantom final line, and empty content has zero lines.
fn count_lines(content: &str) -> u64 {
    if content.is_empty() {
        return 0;
    }
    let mut n = content.lines().count() as u64;
    // `str::lines` already drops a single trailing newline's empty segment, so
    // "a\n" → 1 line, "a\nb" → 2 lines, "a\nb\n" → 2 lines. That matches
    // git-mesh's inclusive line-count semantics.
    if n == 0 {
        n = 1;
    }
    n
}

fn locate_existing_suffix(rel_path: &str, repo_root: &Path) -> Option<String> {
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

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── slug path-collision helpers ───────────────────────────────────────────

    #[test]
    fn slug_paths_conflict_prefix_positive() {
        assert!(slug_paths_conflict(
            "wiki/architecture/wiki-scaffold",
            "wiki/architecture/wiki-scaffold/default-glob"
        ));
        assert!(slug_paths_conflict(
            "wiki/architecture/wiki-scaffold/default-glob",
            "wiki/architecture/wiki-scaffold"
        ));
    }

    #[test]
    fn slug_paths_conflict_equal() {
        assert!(slug_paths_conflict("wiki/scaffold", "wiki/scaffold"));
    }

    #[test]
    fn slug_paths_conflict_sibling_negative() {
        assert!(!slug_paths_conflict("wiki/scaffold", "wiki/scaffold-2"));
        assert!(!slug_paths_conflict("wiki/scaffold-2", "wiki/scaffold"));
        assert!(!slug_paths_conflict("wiki/foo", "wiki/bar"));
    }

    #[test]
    fn mesh_fs_prefix_collision_ancestor_file_true() {
        let tmp = tempfile::tempdir().unwrap();
        let md = tmp.path();
        std::fs::create_dir_all(md.join("wiki/architecture")).unwrap();
        std::fs::write(md.join("wiki/architecture/wiki-scaffold"), "x").unwrap();
        assert!(mesh_fs_prefix_collision(
            md,
            "wiki/architecture/wiki-scaffold/default-glob"
        ));
        assert_eq!(
            mesh_fs_collision_path(md, "wiki/architecture/wiki-scaffold/default-glob"),
            "wiki/architecture/wiki-scaffold"
        );
    }

    #[test]
    fn mesh_fs_prefix_collision_slug_as_dir_true() {
        let tmp = tempfile::tempdir().unwrap();
        let md = tmp.path();
        std::fs::create_dir_all(md.join("wiki/scaffold/child")).unwrap();
        assert!(mesh_fs_prefix_collision(md, "wiki/scaffold"));
        assert_eq!(mesh_fs_collision_path(md, "wiki/scaffold"), "wiki/scaffold");
    }

    #[test]
    fn mesh_fs_prefix_collision_clean_false() {
        let tmp = tempfile::tempdir().unwrap();
        let md = tmp.path();
        std::fs::create_dir_all(md.join("wiki")).unwrap();
        std::fs::write(md.join("wiki/other"), "x").unwrap();
        assert!(!mesh_fs_prefix_collision(md, "wiki/scaffold/default-glob"));
    }

    // ── blocker-rename helpers ────────────────────────────────────────────────

    #[test]
    fn ancestor_file_blocker_finds_strict_ancestor_file() {
        let tmp = tempfile::tempdir().unwrap();
        let md = tmp.path();
        std::fs::create_dir_all(md.join("wiki/arch")).unwrap();
        std::fs::write(md.join("wiki/arch/scaff"), "x").unwrap();
        assert_eq!(
            ancestor_file_blocker(md, "wiki/arch/scaff/helper"),
            Some("wiki/arch/scaff".to_string())
        );
    }

    #[test]
    fn ancestor_file_blocker_none_for_dir_collision() {
        let tmp = tempfile::tempdir().unwrap();
        let md = tmp.path();
        std::fs::create_dir_all(md.join("wiki/scaffold/child")).unwrap();
        // `wiki/scaffold` exists only as a directory — not an ancestor file.
        assert_eq!(ancestor_file_blocker(md, "wiki/scaffold"), None);
    }

    #[test]
    fn parse_mesh_wiki_anchors_collects_only_md_anchors_before_blank() {
        let text = "wiki/arch/page.md#L5-L12 sha256:abc\n\
                     src/lib.rs#L1-L3 sha256:def\n\
                     \n\
                     why prose here\n\
                     wiki/ignored.md#L1-L1 sha256:zzz\n";
        let anchors = parse_mesh_wiki_anchors(text);
        assert_eq!(anchors, vec![("wiki/arch/page.md".to_string(), 5, 12)]);
    }

    #[test]
    fn parse_mesh_wiki_anchors_handles_whole_file_md_anchor() {
        let text = "wiki/whole.md sha256:abc\n\nwhy\n";
        assert_eq!(
            parse_mesh_wiki_anchors(text),
            vec![("wiki/whole.md".to_string(), 0, 0)]
        );
    }

    #[test]
    fn derive_rename_target_reuses_section_noun() {
        let tmp = tempfile::tempdir().unwrap();
        let md = tmp.path();
        let mut nouns = std::collections::HashMap::new();
        nouns.insert(("wiki/arch/page.md".to_string(), 5, 12), "helper".to_string());
        let target = derive_rename_target(
            md,
            "wiki/arch/scaff",
            &[("wiki/arch/page.md".to_string(), 5, 12)],
            &nouns,
        );
        assert_eq!(target, "wiki/arch/scaff/helper");
    }

    #[test]
    fn derive_rename_target_index_when_no_unique_anchor() {
        let tmp = tempfile::tempdir().unwrap();
        let md = tmp.path();
        let nouns = std::collections::HashMap::new();
        assert_eq!(
            derive_rename_target(md, "wiki/b", &[], &nouns),
            "wiki/b/index"
        );
    }

    #[test]
    fn derive_rename_target_index_when_two_distinct_anchors() {
        let tmp = tempfile::tempdir().unwrap();
        let md = tmp.path();
        let nouns = std::collections::HashMap::new();
        let target = derive_rename_target(
            md,
            "wiki/b",
            &[
                ("wiki/a.md".to_string(), 1, 2),
                ("wiki/c.md".to_string(), 3, 4),
            ],
            &nouns,
        );
        assert_eq!(target, "wiki/b/index");
    }

    #[test]
    fn derive_rename_target_numeric_index_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        let md = tmp.path();
        std::fs::create_dir_all(md.join("wiki/b")).unwrap();
        std::fs::write(md.join("wiki/b/index"), "x").unwrap();
        let nouns = std::collections::HashMap::new();
        assert_eq!(
            derive_rename_target(md, "wiki/b", &[], &nouns),
            "wiki/b/index-2"
        );
    }

    #[test]
    fn derive_rename_target_falls_back_when_leaf_equals_blocker_last() {
        let tmp = tempfile::tempdir().unwrap();
        let md = tmp.path();
        let mut nouns = std::collections::HashMap::new();
        nouns.insert(("p.md".to_string(), 1, 1), "scaff".to_string());
        // Leaf would equal blocker's last segment `scaff` → use `index`.
        let target = derive_rename_target(
            md,
            "wiki/scaff",
            &[("p.md".to_string(), 1, 1)],
            &nouns,
        );
        assert_eq!(target, "wiki/scaff/index");
    }

    #[test]
    fn plan_blocker_rename_ok_for_ancestor_file() {
        let tmp = tempfile::tempdir().unwrap();
        let md = tmp.path();
        std::fs::create_dir_all(md.join("wiki/arch")).unwrap();
        std::fs::write(
            md.join("wiki/arch/scaff"),
            "src/lib.rs#L1-L1 sha256:abc\n\nwhy\n",
        )
        .unwrap();
        let nouns = std::collections::HashMap::new();
        let plan =
            plan_blocker_rename(md, "wiki/arch/scaff/helper", "p.md", &nouns).unwrap();
        assert_eq!(plan.from, "wiki/arch/scaff");
        assert_eq!(plan.to, "wiki/arch/scaff/index");
        assert_eq!(plan.for_slug, "wiki/arch/scaff/helper");
    }

    #[test]
    fn plan_blocker_rename_none_for_dir_at_slug_path() {
        let tmp = tempfile::tempdir().unwrap();
        let md = tmp.path();
        std::fs::create_dir_all(md.join("wiki/scaffold/child")).unwrap();
        let nouns = std::collections::HashMap::new();
        // `wiki/scaffold` is a directory, not an ancestor file → fail open.
        assert!(plan_blocker_rename(md, "wiki/scaffold", "p.md", &nouns).is_none());
    }

    #[test]
    fn plan_blocker_rename_none_for_unreadable_blocker() {
        let tmp = tempfile::tempdir().unwrap();
        let md = tmp.path();
        std::fs::create_dir_all(md.join("wiki")).unwrap();
        // Non-UTF8 payload → `read_to_string` fails → fail open.
        std::fs::write(md.join("wiki/b"), [0xff, 0xfe, 0x00]).unwrap();
        let nouns = std::collections::HashMap::new();
        assert!(plan_blocker_rename(md, "wiki/b/leaf", "p.md", &nouns).is_none());
    }

    #[test]
    fn parse_frontmatter_extracts_title_and_summary() {
        let c = "---\ntitle: Hello World\nsummary: A page summary.\n---\nbody";
        assert_eq!(
            parse_frontmatter_field(c, "title"),
            Some("Hello World".into())
        );
        assert_eq!(
            parse_frontmatter_field(c, "summary"),
            Some("A page summary.".into())
        );
    }

    #[test]
    fn parse_frontmatter_handles_quoted_values() {
        let c = "---\ntitle: \"Quoted Title\"\n---\n";
        assert_eq!(
            parse_frontmatter_field(c, "title"),
            Some("Quoted Title".into())
        );
    }

    #[test]
    fn parse_frontmatter_returns_none_when_absent() {
        assert!(parse_frontmatter_field("no frontmatter here", "title").is_none());
    }

    #[test]
    fn parse_frontmatter_ignores_thematic_break_in_body() {
        // Body contains a `---` separator followed by a `title:` line — must NOT match.
        let c = "# Heading\n\nbody text\n\n---\ntitle: Spurious\n\nmore body\n";
        assert_eq!(parse_frontmatter_field(c, "title"), None);
    }

    // ── heading-chain trim ────────────────────────────────────────────────────

    #[test]
    fn trim_chain_drops_leading_when_equals_page_title() {
        let chain = vec!["Billing".to_string(), "Charge handler".to_string()];
        let trimmed = trim_heading_chain(&chain, "Billing");
        assert_eq!(trimmed, vec!["Charge handler"]);
    }

    #[test]
    fn trim_chain_keeps_chain_when_top_differs() {
        let chain = vec!["Charge handler".to_string()];
        let trimmed = trim_heading_chain(&chain, "Billing");
        assert_eq!(trimmed, vec!["Charge handler"]);
    }

    #[test]
    fn trim_chain_empties_to_nothing_when_single_equals_title() {
        let chain = vec!["Incremental indexing".to_string()];
        let trimmed = trim_heading_chain(&chain, "Incremental indexing");
        assert!(trimmed.is_empty());
    }

    // ── classify_frontmatter unit tests ──────────────────────────────────────

    fn classify_str(text: &str) -> Option<ParseErrorKind> {
        // Write to a tempfile and run the classifier.
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let dir = std::env::temp_dir();
        std::fs::write(tmp.path(), text.as_bytes()).unwrap();
        let (_, kind) = classify_frontmatter(tmp.path(), &dir, DocSource::WorkingTree);
        kind
    }

    #[test]
    fn classify_no_frontmatter_is_not_an_error() {
        // A plain markdown file without a frontmatter block is valid — it has
        // no extractable title but does not produce a parse error.
        let kind = classify_str("# Just a body.\n");
        assert!(kind.is_none(), "expected None, got {kind:?}");
    }

    #[test]
    fn classify_missing_title_is_not_an_error() {
        // Frontmatter without a `title:` key is valid — the page has no
        // extractable title but does not produce a parse error.
        let kind = classify_str("---\nsummary: x\n---\n\nbody\n");
        assert!(kind.is_none(), "expected None, got {kind:?}");
    }

    #[test]
    fn classify_empty_title() {
        let kind = classify_str("---\ntitle:\nsummary: x\n---\n\nbody\n");
        assert!(
            matches!(kind, Some(ParseErrorKind::EmptyTitle)),
            "expected EmptyTitle, got {kind:?}"
        );
    }

    #[test]
    fn classify_malformed_bom() {
        // BOM-prefixed frontmatter — the file does not start with `---\n`, so
        // it is treated as a plain markdown page (no frontmatter, no error).
        let kind = classify_str("\u{FEFF}---\ntitle: x\nsummary: y\n---\n");
        assert!(kind.is_none(), "expected None, got {kind:?}");
    }

    #[test]
    fn classify_unreadable_non_utf8() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), [0xFF_u8, 0xFE, 0x00]).unwrap();
        let (_, kind) =
            classify_frontmatter(tmp.path(), &std::env::temp_dir(), DocSource::WorkingTree);
        assert!(
            matches!(kind, Some(ParseErrorKind::Unreadable(_))),
            "expected Unreadable, got {kind:?}"
        );
    }

    #[test]
    fn classify_clean_file() {
        let kind = classify_str("---\ntitle: Hello\nsummary: World\n---\n\nbody\n");
        assert!(kind.is_none(), "expected no error, got {kind:?}");
    }

    // ── resolve_slug_collisions ──────────────────────────────────────────────

    use super::super::draft::StructuredAnchor;

    fn make_draft(
        page_path: &str,
        noun: &str,
        page_subdir: &str,
        heading_chain: Vec<&str>,
    ) -> MeshDraft {
        let slug = super::super::draft::build_slug(page_subdir, noun);
        MeshDraft {
            page_path: page_path.to_string(),
            slug,
            anchors: Vec::new(),
            structured_anchors: vec![StructuredAnchor {
                path: page_path.to_string(),
                start_line: 1,
                end_line: 1,
            }],
            heading_chain: heading_chain.iter().map(|s| s.to_string()).collect(),
            consolidated_count: 1,
            noun: noun.to_string(),
            page_subdir: page_subdir.to_string(),
            extends_existing: None,
        }
    }

    #[test]
    fn collision_resolver_keeps_unique_slugs() {
        let mut drafts = vec![make_draft("wiki/billing.md", "charge-handler", "", vec![])];
        let titles: std::collections::HashMap<String, Option<String>> =
            std::collections::HashMap::new();
        let exists = |_slug: &str| false;
        resolve_slug_collisions(&mut drafts, &titles, &exists);
        assert_eq!(drafts[0].slug, "wiki/charge-handler");
    }

    #[test]
    fn collision_resolver_adds_parent_heading_on_clash() {
        let mut drafts = vec![make_draft(
            "wiki/billing.md",
            "charge-handler",
            "",
            vec!["Checkout"],
        )];
        let titles: std::collections::HashMap<String, Option<String>> =
            std::collections::HashMap::new();
        let existing: std::collections::HashSet<String> =
            ["wiki/charge-handler".to_string()].into_iter().collect();
        let exists = |slug: &str| existing.contains(slug);
        resolve_slug_collisions(&mut drafts, &titles, &exists);
        assert_eq!(drafts[0].slug, "wiki/checkout/charge-handler");
    }

    #[test]
    fn collision_resolver_walks_chain_outer_to_inner() {
        // chain is outer→inner. Base slug clashes; +inner-most ("Checkout")
        // also clashes; the resolver must try adding the next ancestor up
        // ("Payments") before falling back further.
        let mut drafts = vec![make_draft(
            "wiki/billing.md",
            "charge-handler",
            "",
            vec!["Payments", "Checkout"],
        )];
        let titles: std::collections::HashMap<String, Option<String>> =
            std::collections::HashMap::new();
        let existing: std::collections::HashSet<String> = [
            "wiki/charge-handler".to_string(),
            "wiki/checkout/charge-handler".to_string(),
        ]
        .into_iter()
        .collect();
        let exists = |slug: &str| existing.contains(slug);
        resolve_slug_collisions(&mut drafts, &titles, &exists);
        assert_eq!(drafts[0].slug, "wiki/payments/checkout/charge-handler");
    }

    #[test]
    fn collision_resolver_uses_page_title_after_chain_exhausted() {
        let mut drafts = vec![make_draft(
            "wiki/billing.md",
            "charge-handler",
            "",
            vec!["Checkout"],
        )];
        let mut titles: std::collections::HashMap<String, Option<String>> =
            std::collections::HashMap::new();
        titles.insert(
            "wiki/billing.md".to_string(),
            Some("Billing Service".to_string()),
        );
        let existing: std::collections::HashSet<String> = [
            "wiki/charge-handler".to_string(),
            "wiki/checkout/charge-handler".to_string(),
        ]
        .into_iter()
        .collect();
        let exists = |slug: &str| existing.contains(slug);
        resolve_slug_collisions(&mut drafts, &titles, &exists);
        assert_eq!(
            drafts[0].slug,
            "wiki/billing-service/checkout/charge-handler"
        );
    }

    #[test]
    fn collision_resolver_falls_back_to_digit_suffix() {
        let mut drafts = vec![make_draft("wiki/billing.md", "charge-handler", "", vec![])];
        let titles: std::collections::HashMap<String, Option<String>> =
            std::collections::HashMap::new();
        let existing: std::collections::HashSet<String> =
            ["wiki/charge-handler".to_string()].into_iter().collect();
        let exists = |slug: &str| existing.contains(slug);
        resolve_slug_collisions(&mut drafts, &titles, &exists);
        // No chain, no title — only the base slug is a unique candidate,
        // so the digit fallback runs against it.
        assert_eq!(drafts[0].slug, "wiki/charge-handler-2");
    }

    #[test]
    fn collision_resolver_dedups_within_run() {
        // Two drafts with the same base slug and no semantic disambiguators
        // available; the second must get a digit suffix.
        let mut drafts = vec![
            make_draft("wiki/a.md", "foo", "", vec![]),
            make_draft("wiki/b.md", "foo", "", vec![]),
        ];
        let titles: std::collections::HashMap<String, Option<String>> =
            std::collections::HashMap::new();
        let exists = |_slug: &str| false;
        resolve_slug_collisions(&mut drafts, &titles, &exists);
        assert_eq!(drafts[0].slug, "wiki/foo");
        assert_eq!(drafts[1].slug, "wiki/foo-2");
    }

    #[test]
    fn collision_resolver_intra_run_uses_semantic_qualifier_first() {
        let mut drafts = vec![
            make_draft("wiki/a.md", "foo", "", vec!["First"]),
            make_draft("wiki/b.md", "foo", "", vec!["Second"]),
        ];
        let titles: std::collections::HashMap<String, Option<String>> =
            std::collections::HashMap::new();
        let exists = |_slug: &str| false;
        resolve_slug_collisions(&mut drafts, &titles, &exists);
        assert_eq!(drafts[0].slug, "wiki/foo");
        // Second clashes only with the first draft (in-run); the parent
        // heading "Second" resolves it without needing a digit.
        assert_eq!(drafts[1].slug, "wiki/second/foo");
    }

    #[test]
    fn collision_resolver_skips_duplicate_candidates_from_reserved_drop() {
        // Page lives at subdir "mesh"; parent heading is also "mesh",
        // so the +parent candidate collapses back to the base slug. The resolver
        // must skip that duplicate and try the next strategy (page title).
        let mut drafts = vec![make_draft("mesh/page.md", "leaf", "mesh", vec!["Mesh"])];
        let mut titles: std::collections::HashMap<String, Option<String>> =
            std::collections::HashMap::new();
        titles.insert("mesh/page.md".to_string(), Some("Outer".to_string()));
        let existing: std::collections::HashSet<String> =
            ["wiki/mesh/leaf".to_string()].into_iter().collect();
        let exists = |slug: &str| existing.contains(slug);
        resolve_slug_collisions(&mut drafts, &titles, &exists);
        assert_eq!(drafts[0].slug, "wiki/mesh/outer/leaf");
    }

    // ── resolve_page_subdir ──────────────────────────────────────────────────

    #[test]
    fn resolve_page_subdir_returns_parent_relative_to_repo_root() {
        let repo_root = std::path::Path::new("/repo");
        let page = std::path::Path::new("/repo/wiki/billing.md");
        assert_eq!(resolve_page_subdir(page, repo_root), "wiki");
    }

    #[test]
    fn resolve_page_subdir_nested_dir() {
        let repo_root = std::path::Path::new("/repo");
        let page = std::path::Path::new("/repo/wiki/payments/charge.md");
        assert_eq!(resolve_page_subdir(page, repo_root), "wiki/payments");
    }

    #[test]
    fn resolve_page_subdir_at_repo_root() {
        let repo_root = std::path::Path::new("/repo");
        let page = std::path::Path::new("/repo/readme.md");
        assert_eq!(resolve_page_subdir(page, repo_root), "");
    }

    // ── missing-anchor filtering ─────────────────────────────────────────────

    #[test]
    fn missing_anchor_drops_mesh_and_records_dropped_entry() {
        // Build two drafts: one whose second anchor exists on disk and one
        // whose second anchor does not.
        let tmp = tempfile::TempDir::new().unwrap();
        let repo_root = tmp.path();

        // Create the file that the first draft references.
        let present = repo_root.join("src/present.rs");
        std::fs::create_dir_all(present.parent().unwrap()).unwrap();
        std::fs::write(&present, "// present\n").unwrap();

        let make = |page: &str, anchor_path: &str| {
            let slug = super::super::draft::build_slug(
                "wiki",
                page.split('/').next_back().unwrap().trim_end_matches(".md"),
            );
            super::super::draft::MeshDraft {
                page_path: page.to_string(),
                slug,
                anchors: vec![format!("{page}#L1-L5"), format!("{anchor_path}#L1-L5")],
                structured_anchors: vec![
                    StructuredAnchor {
                        path: page.to_string(),
                        start_line: 1,
                        end_line: 5,
                    },
                    StructuredAnchor {
                        path: anchor_path.to_string(),
                        start_line: 1,
                        end_line: 5,
                    },
                ],
                heading_chain: vec![],
                consolidated_count: 1,
                noun: "test".to_string(),
                page_subdir: "wiki".to_string(),
                extends_existing: None,
            }
        };

        let mut drafts = vec![
            make("wiki/a.md", "src/present.rs"),
            make("wiki/b.md", "src/missing.rs"),
        ];

        let mut dropped: Vec<DroppedMesh> = Vec::new();
        drafts.retain(|draft| {
            for anchor in draft.structured_anchors.iter().skip(1) {
                let abs = repo_root.join(&anchor.path);
                if !abs.is_file() {
                    dropped.push(DroppedMesh {
                        slug: draft.slug.clone(),
                        reason: DropReason::MissingPath {
                            path: anchor.path.clone(),
                        },
                        page: draft.page_path.clone(),
                    });
                    return false;
                }
            }
            true
        });

        assert_eq!(
            drafts.len(),
            1,
            "only the draft with present anchor should survive"
        );
        assert_eq!(drafts[0].page_path, "wiki/a.md");
        assert_eq!(dropped.len(), 1, "one mesh should be dropped");
        assert_eq!(dropped[0].page, "wiki/b.md");
        match &dropped[0].reason {
            DropReason::MissingPath { path } => assert_eq!(path, "src/missing.rs"),
            other => panic!("expected MissingPath, got {other:?}"),
        }
    }

    // ── invalid-anchor static validation ─────────────────────────────────────

    #[test]
    fn invalid_anchor_detail_flags_over_range() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo_root = tmp.path();
        std::fs::create_dir_all(repo_root.join("src")).unwrap();
        std::fs::write(repo_root.join("src/lib.rs"), "// l1\n// l2\n").unwrap();
        let anchor = StructuredAnchor {
            path: "src/lib.rs".to_string(),
            start_line: 1,
            end_line: 999,
        };
        let detail =
            invalid_anchor_detail(repo_root, &anchor, DocSource::WorkingTree, &None);
        assert_eq!(
            detail,
            Some("end exceeds file line count 2".to_string()),
            "over-range anchor must be flagged"
        );
    }

    #[test]
    fn invalid_anchor_detail_accepts_valid_range() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo_root = tmp.path();
        std::fs::create_dir_all(repo_root.join("src")).unwrap();
        std::fs::write(repo_root.join("src/lib.rs"), "// l1\n// l2\n// l3\n").unwrap();
        let anchor = StructuredAnchor {
            path: "src/lib.rs".to_string(),
            start_line: 1,
            end_line: 3,
        };
        assert_eq!(
            invalid_anchor_detail(repo_root, &anchor, DocSource::WorkingTree, &None),
            None,
            "valid anchor must pass through (preserve fail-closed downstream)"
        );
    }

    #[test]
    fn invalid_anchor_detail_flags_inverted_range() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo_root = tmp.path();
        std::fs::create_dir_all(repo_root.join("src")).unwrap();
        std::fs::write(repo_root.join("src/lib.rs"), "// l1\n// l2\n").unwrap();
        let anchor = StructuredAnchor {
            path: "src/lib.rs".to_string(),
            start_line: 2,
            end_line: 1,
        };
        assert_eq!(
            invalid_anchor_detail(repo_root, &anchor, DocSource::WorkingTree, &None),
            Some("start line 2 exceeds end line 1".to_string())
        );
    }

    #[test]
    fn count_lines_matches_git_mesh_semantics() {
        assert_eq!(count_lines(""), 0);
        assert_eq!(count_lines("a\n"), 1);
        assert_eq!(count_lines("a\nb"), 2);
        assert_eq!(count_lines("a\nb\n"), 2);
    }
}
