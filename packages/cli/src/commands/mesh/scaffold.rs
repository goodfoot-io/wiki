//! Mesh-coverage engine.
//!
//! Discover wiki files, parse their fragment links, and create `.wiki/<slug>`
//! mesh files in-process. Driven by `wiki check --fix` (the "Fix #4" slot):
//! the pipeline produces consolidated mesh drafts and applies them best-effort.
//! Output is owned by `check`; this module only returns a [`MeshCoverageOutcome`].

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use miette::Result;
use regex::Regex;

use crate::commands::resolve_link_path;
use crate::git::GitReader;
use crate::index::DocSource;
use crate::parser::{LinkKind, parse_fragment_links};

/// Read `path` from the chosen [`DocSource`], routing non-worktree reads
/// through the [`GitReader`] when available so that a single `gix::Repository`
/// handle is reused across all blob reads in the pipeline.
fn read_via_source(
    path: &Path,
    repo_root: &Path,
    source: DocSource,
    git_reader: Option<&GitReader>,
) -> std::io::Result<String> {
    match source {
        DocSource::WorkingTree => fs::read_to_string(path),
        DocSource::Index | DocSource::Head => {
            let path_rel = path
                .strip_prefix(repo_root)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| path.to_string_lossy().into_owned());
            let result = if let Some(gr) = git_reader {
                gr.read_blob(source, &path_rel)
            } else {
                source.read(repo_root, &path_rel)
            };
            match result {
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

// ── Drop reasons ──────────────────────────────────────────────────────────────

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
    /// committed. A path git never tracks cannot be anchored and would produce a
    /// permanently-stale anchor. Skipped with an advisory rather than escalated.
    /// Distinct from untracked-but-not-ignored targets, which resolve once
    /// committed and are left to anchor normally.
    IgnoredPath { path: String },
    /// An anchor is statically invalid: line range exceeds the target file's
    /// line count, start > end, or start < 1.
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

use git_mesh_core::AnchorExtent;
use git_mesh_core::mesh_file::MeshFile;

use super::augment::{AugmentedLink, augment};
use super::draft::{self, MeshDraft};
use super::group;
use super::render;
use super::store;

/// A mesh draft that the in-process store failed to apply. The failure reason was
/// already emitted to stderr; we only carry the slug so the caller can name what
/// could not be created.
#[derive(Debug, Clone)]
pub(crate) struct MeshFailure {
    pub(crate) slug: String,
}

/// The result of a mesh-coverage pass.
///
/// `check` owns all output: it prints `applied`/`planned` paths under
/// `--print-applied`, and routes `advisories` (parse errors, dropped meshes,
/// performed/planned renames) and per-draft `failures` to stderr.
#[derive(Debug, Default)]
pub(crate) struct MeshCoverageOutcome {
    /// Repo-relative `.wiki/<slug>` paths created or extended this run.
    pub(crate) applied: Vec<String>,
    /// Drafts that could not be applied (best-effort: the rest still ran).
    pub(crate) failures: Vec<MeshFailure>,
    /// Rendered advisory block (parse errors + dropped meshes + renames).
    pub(crate) advisories: String,
    /// Repo-relative `.wiki/<slug>` paths that WOULD be created (dry-run preview).
    pub(crate) planned: Vec<String>,
    /// Repo-relative `.wiki/<slug>` paths deleted this run (scaffold-orphan cleanup).
    pub(crate) deleted: Vec<String>,
    /// Repo-relative `.wiki/<slug>` paths that WOULD be deleted (dry-run preview).
    pub(crate) planned_deletions: Vec<String>,
}

/// Build mesh coverage for `files` and apply it best-effort.
///
/// Runs the full discovery → group → collision → gitignore/missing/invalid-anchor
/// pipeline (byte-identical mesh set to the former `scaffold` command), then:
/// - `dry_run = true`: mutates nothing; `planned` lists the meshes that would
///   be created (and renamed-blocker NEW paths).
/// - `dry_run = false`: applies every draft via the in-process store, accumulating
///   per-draft failures and continuing past them.
pub(crate) fn create_mesh_coverage(
    files: &[PathBuf],
    repo_root: &Path,
    source: crate::index::DocSource,
    dry_run: bool,
    mesh_index: Option<&crate::commands::mesh_coverage::MeshIndex>,
    git_reader: Option<&GitReader>,
) -> Result<MeshCoverageOutcome> {
    // Re-apply the `/tests/fixtures/` exclusion so the produced mesh set is
    // byte-identical to the former `scaffold` command.
    let files: Vec<PathBuf> = files
        .iter()
        .filter(|f| {
            let s = f.to_string_lossy();
            !s.contains("/tests/fixtures/") && !s.contains("\\tests\\fixtures\\")
        })
        .cloned()
        .collect();

    // Coverage index: filter out fragment links already covered by a mesh in
    // the repo. When the caller supplies a pre-built index (e.g. from
    // `run_fix_pass` which already built one for link rewriting), reuse it
    // to avoid re-walking and re-parsing the entire `.wiki/` store.
    let owned_index;
    let mesh_index: &crate::commands::mesh_coverage::MeshIndex = match mesh_index {
        Some(idx) => idx,
        None => {
            owned_index = match crate::commands::mesh_coverage::build_mesh_index(repo_root, &files) {
                Ok(Some(idx)) => idx,
                Ok(None) => return Ok(MeshCoverageOutcome::default()),
                Err(e) => return Err(e),
            };
            &owned_index
        }
    };

    let mut all_inputs: Vec<LinkInput> = Vec::new();
    for file in &files {
        let content = match read_via_source(file, repo_root, source, git_reader) {
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
        let (meta, err_kind) = classify_frontmatter(f, repo_root, source, git_reader);
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
    apply_section_extension(&mut consolidated, mesh_index);

    // Resolve slug collisions across both already-assigned slugs in this run
    // and any meshes that already live in the repo. Extension drafts opt out:
    // they reuse the existing mesh's slug verbatim and skip the probe entirely.
    let mesh_dir = store::wiki_dir(repo_root);
    let probe =
        |slug: &str| store::exists(repo_root, slug) || mesh_fs_prefix_collision(&mesh_dir, slug);
    resolve_slug_collisions(&mut consolidated, &page_titles, &probe);

    // Drop meshes whose non-wiki anchors reference paths that don't exist in
    // the active source.
    let source_paths: Option<std::collections::HashSet<String>> = match source {
        crate::index::DocSource::WorkingTree => None, // check filesystem inline
        crate::index::DocSource::Index | crate::index::DocSource::Head => {
            let paths = if let Some(gr) = git_reader {
                gr.list_paths(source).unwrap_or_default()
            } else {
                source.list_paths(repo_root).unwrap_or_default()
            };
            Some(paths.into_iter().collect())
        }
    };
    let mut dropped_meshes: Vec<DroppedMesh> = Vec::new();

    // ── Slug-path-collision pass ──────────────────────────────────────────
    // A pre-existing mesh occupying a conflicting path makes writing a new
    // mesh at a longer slug structurally impossible (the existing file would
    // need to become a directory). When the collision is a strict ANCESTOR FILE
    // (existing shorter mesh `B` blocks new longer slug `B/...`), the remedy
    // is to RENAME the blocker out of the way (apply mode) / report the
    // planned rename (dry-run), keeping the new draft. The blocker is read and
    // its single distinct wiki-page anchor drives a reused-noun leaf for the
    // rename target. Any failure (non-ancestor-file collision, unreadable
    // blocker) FAILS OPEN to the drop-with-advisory path (exit 0) — never
    // fail-closed, never panic.
    //
    // Extension drafts reuse an existing slug verbatim, so they are exempt.
    let section_noun: std::collections::HashMap<(String, u32, u32), String> = consolidated
        .iter()
        .filter_map(|d| {
            d.structured_anchors
                .first()
                .map(|a| ((a.path.clone(), a.start_line, a.end_line), d.noun.clone()))
        })
        .collect();
    let mut planned_renames: Vec<PlannedRename> = Vec::new();
    consolidated.retain(|draft| {
        if draft.extends_existing.is_some() || !mesh_fs_prefix_collision(&mesh_dir, &draft.slug) {
            return true;
        }
        // Try the rename-the-blocker remedy for the ancestor-file case.
        match plan_blocker_rename(&mesh_dir, &draft.slug, &draft.page_path, &section_noun) {
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
            // Fail open: not an ancestor-file case or unreadable blocker →
            // drop-with-advisory, exit 0.
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
    // from mesh coverage exactly as `wiki check` treats it: a path git never
    // sees cannot be anchored, so demanding coverage would be
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
    // Wikiignored anchor targets are stripped alongside gitignored ones: they
    // are invisible at every wiki surface, so scaffolding a mesh that anchors
    // them would resurrect them via the mesh-coverage contract.
    let wiki_ignore =
        crate::wikiignore::WikiIgnore::load(repo_root).map_err(|e| miette::miette!("{e}"))?;

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
            if ignored_anchor_paths.contains(&a.path)
                || wiki_ignore.is_ignored(Path::new(&a.path))
            {
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

    // Per-run file-content cache shared between validation (bounds checking)
    // and hashing (apply_drafts). A file referenced by K anchors across drafts
    // is read once — not up to 2K times.
    let mut content_cache = store::FileContentCache::new();

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
            // against the target file before writing it to the store.
            // An over-range / inverted / zero-start anchor is a drifted wiki
            // link (a fixable wiki condition), NOT a hard build failure: drop
            // it with a named advisory so the link resurfaces as a residual
            // `mesh_uncovered` on the post-fix recheck rather than aborting.
            if let Some(detail) =
                invalid_anchor_detail(repo_root, anchor, source, &source_paths, &mut content_cache, git_reader)
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

    // Advisory block: parse errors + dropped meshes + performed/planned renames.
    // The rename phrasing follows `dry_run` (planned vs performed).
    let mut advisories = String::new();
    if !parse_errors.is_empty() || !dropped_meshes.is_empty() {
        render::render_advisories(&mut advisories, &parse_errors, &dropped_meshes, true);
    }
    render::render_rename_advisories(&mut advisories, &planned_renames, dry_run);

    // Repo-relative path of a wiki-dir-relative name (`.wiki/<slug>`,
    // forward slashes).
    let rel_mesh_path = |name: &str| -> String {
        match mesh_dir.strip_prefix(repo_root) {
            Ok(d) => {
                let d = d.to_string_lossy().replace('\\', "/");
                if d.is_empty() {
                    name.to_string()
                } else {
                    format!("{d}/{name}")
                }
            }
            Err(_) => mesh_dir.join(name).to_string_lossy().replace('\\', "/"),
        }
    };

    // Empty when there were no internal fragment links at all OR when every
    // section already has its links anchored by an existing mesh.
    if all_inputs.is_empty() || consolidated.is_empty() {
        // Blocker renames were already performed on disk by the slug-collision
        // retain pass above; record them as applied so the caller stages them.
        let applied: Vec<String> = planned_renames
            .iter()
            .map(|r| rel_mesh_path(&r.to))
            .collect();
        return Ok(MeshCoverageOutcome {
            applied,
            failures: Vec::new(),
            advisories,
            planned: Vec::new(),
            deleted: Vec::new(),
            planned_deletions: Vec::new(),
        });
    }

    if dry_run {
        // Preview only — never mutate. `planned` lists every mesh that would be
        // created (renamed-blocker NEW paths first, then each draft's slug).
        let mut planned: Vec<String> = Vec::new();
        for r in &planned_renames {
            planned.push(rel_mesh_path(&r.to));
        }
        for draft in &consolidated {
            let slug = draft
                .extends_existing
                .as_deref()
                .unwrap_or(draft.slug.as_str());
            planned.push(rel_mesh_path(slug));
        }
        return Ok(MeshCoverageOutcome {
            applied: Vec::new(),
            failures: Vec::new(),
            advisories,
            planned,
            deleted: Vec::new(),
            planned_deletions: Vec::new(),
        });
    }

    // Apply mode. Renamed-blocker NEW paths were already freed on disk by
    // `run_blocker_rename` above; record them as applied so the caller stages
    // them.
    let mut applied: Vec<String> = planned_renames
        .iter()
        .map(|r| rel_mesh_path(&r.to))
        .collect();
    let (draft_applied, failures) = apply_drafts(&consolidated, repo_root, &mesh_dir, &mut content_cache);
    applied.extend(draft_applied);

    Ok(MeshCoverageOutcome {
        applied,
        failures,
        advisories,
        planned: Vec::new(),
        deleted: Vec::new(),
        planned_deletions: Vec::new(),
    })
}

/// Build the repo-relative path string for a mesh given its mesh-dir-relative
/// `name` (e.g. `wiki/scaffold`).
///
/// Returns `<mesh_dir_rel>/<name>` when `mesh_dir` is inside `repo_root`, or
/// the absolute stringified path as a fallback.
fn mesh_rel_path(repo_root: &Path, mesh_dir: &Path, name: &str) -> String {
    match mesh_dir.strip_prefix(repo_root) {
        Ok(d) => {
            let d = d.to_string_lossy().replace('\\', "/");
            if d.is_empty() {
                name.to_string()
            } else {
                format!("{d}/{name}")
            }
        }
        Err(_) => mesh_dir.join(name).to_string_lossy().replace('\\', "/"),
    }
}

/// True iff `text` contains a non-empty line after the first blank line.
///
/// A scaffold mesh has anchor lines, a blank line, then EOF — no prose. A
/// curated mesh has the `why` sentence after the blank line. This discriminator
/// distinguishes them without needing to parse the anchor block.
fn mesh_has_why(text: &str) -> bool {
    let mut past_blank = false;
    for line in text.lines() {
        if line.trim().is_empty() {
            past_blank = true;
        } else if past_blank {
            return true;
        }
    }
    false
}

/// Outcome of the orphan-mesh cleanup stage.
pub(crate) struct MeshCleanupResult {
    pub(crate) deleted: Vec<String>,
    pub(crate) planned_deletions: Vec<String>,
    pub(crate) advisories: String,
}

/// Delete (or preview deletion of) scaffold meshes whose sole wiki page has
/// been removed from disk.
///
/// Eligibility criteria (ALL must hold):
/// 1. At least one `.md` anchor (wiki-derived mesh).
/// 2. No `why` prose (`!mesh_has_why`).
/// 3. Exactly one distinct `.md` path among anchors.
/// 4. That path's file is absent on disk.
/// 5. The sole `.md` anchor falls within the run's scope (`scan_root`/`globs`).
///
/// Ineligible-but-orphaned meshes (criteria 1+4 but NOT 2 or 3) produce an
/// advisory. Meshes that are fully covered (no missing page), have no `.md`
/// anchor, or whose anchor is out of scope are left silently.
///
/// `scan_root` and `globs` mirror the discovery scope from the calling
/// `check::run`. When `scan_root == repo_root` and `globs` is empty the scope
/// predicate matches everything (whole-repo run — no behavior change).
///
/// Best-effort: a failed `store::delete` is recorded and reported, never
/// aborting the pass. After each delete call, file absence is post-verified —
/// a path is recorded as deleted only when the mesh file is actually gone.
pub(crate) fn cleanup_orphaned_meshes(
    repo_root: &Path,
    scan_root: &Path,
    globs: &[String],
    dry_run: bool,
) -> Result<MeshCleanupResult> {
    let mesh_dir = store::wiki_dir(repo_root);
    let mut deleted: Vec<String> = Vec::new();
    let mut planned_deletions: Vec<String> = Vec::new();
    let mut advisories = String::new();

    if !mesh_dir.is_dir() {
        return Ok(MeshCleanupResult {
            deleted,
            planned_deletions,
            advisories,
        });
    }

    // Build the scope predicate (Finding 3): a mesh's sole .md anchor must
    // fall within this run's scan scope to be eligible for cleanup.
    // Reuse the same helpers that `discover_files` uses so scope matching is
    // byte-identical to discovery.
    let scope_prefix = crate::commands::scan_prefix(scan_root, repo_root);
    // Pre-compile any user globs into a globset using the same
    // `glob_to_repo_relative` transform that `discover_files` uses.
    let glob_set: Option<globset::GlobSet> = if globs.is_empty() {
        None
    } else {
        let mut builder = globset::GlobSetBuilder::new();
        for g in globs {
            let repo_rel =
                crate::commands::glob_to_repo_relative(g, scope_prefix.as_deref(), repo_root);
            if let Ok(pat) = globset::Glob::new(&repo_rel) {
                builder.add(pat);
            }
        }
        builder.build().ok()
    };

    // Returns true when `path_rel` (repo-relative) is within this run's scope.
    let in_scope = |path_rel: &str| -> bool {
        if glob_set.is_none() {
            // No explicit globs — scope is defined purely by scan_root prefix.
            crate::commands::path_under_prefix(path_rel, scope_prefix.as_deref())
        } else {
            // Explicit globs: must match at least one glob AND be under the
            // scan prefix (discovery requires both).
            crate::commands::path_under_prefix(path_rel, scope_prefix.as_deref())
                && glob_set.as_ref().is_some_and(|gs| gs.is_match(path_rel))
        }
    };

    // Walk every regular file under mesh_dir.
    let mut stack: Vec<std::path::PathBuf> = vec![mesh_dir.clone()];
    while let Some(dir) = stack.pop() {
        let rd = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                // Parse the mesh file via MeshFile::parse (replaces local parser).
                let mesh_name = match path.strip_prefix(&mesh_dir) {
                    Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
                    Err(_) => continue,
                };
                let mesh_file = match store::read_one(repo_root, &mesh_name) {
                    Ok(Some(m)) => m,
                    Ok(None) => continue,
                    Err(_) => continue,
                };

                // Extract distinct .md anchor paths from the parsed MeshFile.
                let text_for_why = match std::fs::read_to_string(&path) {
                    Ok(t) => t,
                    Err(_) => continue,
                };
                let mut distinct_paths: Vec<String> = Vec::new();
                for anchor in &mesh_file.anchors {
                    if anchor.path.ends_with(".md") && !distinct_paths.contains(&anchor.path) {
                        distinct_paths.push(anchor.path.clone());
                    }
                }
                if distinct_paths.is_empty() {
                    // No .md anchors — hand-authored, leave silently.
                    continue;
                }

                // Check which distinct paths are missing on disk.
                let missing: Vec<&String> = distinct_paths
                    .iter()
                    .filter(|p| !repo_root.join(p.as_str()).exists())
                    .collect();

                if missing.is_empty() {
                    // All pages present — leave silently.
                    continue;
                }

                let rel_path = mesh_rel_path(repo_root, &mesh_dir, &mesh_name);

                if mesh_has_why(&text_for_why) {
                    // Curated why — advisory only.
                    for m in &missing {
                        advisories.push_str(&format!(
                            "Advisory: mesh `{rel_path}` anchors deleted page `{m}` but has curated why prose — leaving unchanged\n"
                        ));
                    }
                    continue;
                }

                if distinct_paths.len() > 1 {
                    // Multi-page mesh — advisory only.
                    for m in &missing {
                        advisories.push_str(&format!(
                            "Advisory: mesh `{rel_path}` anchors deleted page `{m}` alongside other pages — leaving unchanged\n"
                        ));
                    }
                    continue;
                }

                // Finding 3: scope guard — the sole .md anchor must fall
                // within this run's scan scope. Out-of-scope orphans are
                // left silently; they will be cleaned up by the run that
                // covers their subtree.
                let sole_page = &distinct_paths[0];
                if !in_scope(sole_page) {
                    continue;
                }

                // Eligible: single wiki page, scaffold-style, page gone, in scope.
                if dry_run {
                    planned_deletions.push(rel_path);
                } else {
                    // In-process delete via store::delete (idempotent: missing is ok).
                    match store::delete(repo_root, &mesh_name) {
                        Ok(()) => {
                            // Post-verify: record success only when the file is gone.
                            let mesh_file_gone = !mesh_dir.join(&mesh_name).exists();
                            if mesh_file_gone {
                                deleted.push(rel_path);
                            } else {
                                eprintln!(
                                    "wiki check --fix: mesh `{mesh_name}` delete reported ok but file is still present — skipping"
                                );
                            }
                        }
                        Err(e) => {
                            eprintln!(
                                "wiki check --fix: mesh `{mesh_name}` delete failed: {e} — skipping"
                            );
                        }
                    }
                }
            }
        }
    }

    Ok(MeshCleanupResult {
        deleted,
        planned_deletions,
        advisories,
    })
}

/// Apply each `MeshDraft` by writing a `.wiki/<slug>` mesh file in-process.
///
/// Extension drafts (`extends_existing.is_some()`) re-use the existing slug;
/// the new code anchors are merged into the existing mesh (or a fresh one if
/// the slug doesn't exist yet).
///
/// `cache` avoids re-reading the same target file when multiple anchors across
/// drafts reference it — each file is read once for hashing, not once per anchor.
///
/// Best-effort: every draft is attempted. A write error is recorded as a
/// per-draft [`MeshFailure`] and the loop continues. Returns the repo-relative
/// `.wiki/<slug>` paths that were created and the failures.
fn apply_drafts(
    drafts: &[MeshDraft],
    repo_root: &Path,
    mesh_dir: &Path,
    cache: &mut store::FileContentCache,
) -> (Vec<String>, Vec<MeshFailure>) {
    let mut applied: Vec<String> = Vec::new();
    let mut failures: Vec<MeshFailure> = Vec::new();

    for draft in drafts {
        let slug = draft
            .extends_existing
            .as_deref()
            .unwrap_or(draft.slug.as_str());

        // Build AnchorRecords from the draft's structured anchors.
        // For extension drafts all entries are code anchors; for new-mesh drafts
        // the first entry is the page section anchor. All are hashed the same way.
        let anchor_records_result: Result<Vec<git_mesh_core::mesh_file::AnchorRecord>> = draft
            .structured_anchors
            .iter()
            .map(|a| {
                let extent = if a.start_line == 0 && a.end_line == 0 {
                    AnchorExtent::WholeFile
                } else {
                    AnchorExtent::LineRange {
                        start: a.start_line,
                        end: a.end_line,
                    }
                };
                let content_hash = store::hash_anchor(repo_root, &a.path, extent, cache)
                    .map_err(|e| miette::miette!("failed to hash anchor {}: {e}", a.path))?;
                Ok(store::anchor_record(a.path.clone(), extent, content_hash))
            })
            .collect();

        let new_records = match anchor_records_result {
            Ok(r) => r,
            Err(e) => {
                eprintln!("wiki check --fix: could not create mesh `{slug}`: {e}");
                failures.push(MeshFailure {
                    slug: slug.to_string(),
                });
                continue;
            }
        };

        // For extension drafts: merge new anchors into the existing mesh.
        // For new-mesh drafts: write a fresh mesh with empty why.
        let mesh = if draft.extends_existing.is_some() {
            match store::read_one(repo_root, slug) {
                Ok(Some(mut existing)) => {
                    // Append only anchors not already present (path+range match).
                    for rec in new_records {
                        let already = existing.anchors.iter().any(|a| {
                            a.path == rec.path
                                && a.start_line == rec.start_line
                                && a.end_line == rec.end_line
                        });
                        if !already {
                            existing.anchors.push(rec);
                        }
                    }
                    existing
                }
                Ok(None) => MeshFile {
                    anchors: new_records,
                    why: String::new(),
                },
                Err(e) => {
                    eprintln!("wiki check --fix: could not read mesh `{slug}` for extension: {e}");
                    failures.push(MeshFailure {
                        slug: slug.to_string(),
                    });
                    continue;
                }
            }
        } else {
            MeshFile {
                anchors: new_records,
                why: String::new(),
            }
        };

        if let Err(e) = store::write(repo_root, slug, &mesh) {
            eprintln!("wiki check --fix: could not create mesh `{slug}`: {e}");
            failures.push(MeshFailure {
                slug: slug.to_string(),
            });
            continue;
        }

        applied.push(mesh_rel_path(repo_root, mesh_dir, slug));
    }
    (applied, failures)
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
/// an extension (add to M) with no `why` line.
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
/// Coalesce overlapping or contiguous line-range anchors on the same path into
/// one covering anchor, matching `git-mesh-core`'s coalesce_line_ranges so
/// `wiki check --fix` and `git span drift --fix` settle on a byte-identical
/// fixed point.
///
/// Whole-file anchors (`0-0`) are inert — they are never merged and pass
/// through unchanged. Output preserves first-occurrence path order; within a
/// path, ranges are sorted by `(start, end)` then merged when
/// `next.start <= current.end + 1`. The returned `Vec<String>` and
/// `Vec<StructuredAnchor>` are in lockstep (same set, same order).
fn coalesce_line_ranges(
    anchors: Vec<draft::StructuredAnchor>,
) -> (Vec<String>, Vec<draft::StructuredAnchor>) {
    // Group ranges by path, preserving first-occurrence path order.
    let mut path_order: Vec<String> = Vec::new();
    let mut by_path: std::collections::HashMap<String, Vec<draft::StructuredAnchor>> =
        std::collections::HashMap::new();
    for anchor in anchors {
        if !by_path.contains_key(&anchor.path) {
            path_order.push(anchor.path.clone());
        }
        by_path.entry(anchor.path.clone()).or_default().push(anchor);
    }

    let mut structured: Vec<draft::StructuredAnchor> = Vec::new();
    for path in &path_order {
        let mut ranges = by_path.remove(path).expect("path tracked in order");
        // Whole-file anchors are inert; never merge them.
        let (whole_file, mut line_ranges): (Vec<_>, Vec<_>) = ranges
            .drain(..)
            .partition(|a| a.start_line == 0 && a.end_line == 0);
        line_ranges.sort_by_key(|a| (a.start_line, a.end_line));

        let mut merged: Vec<draft::StructuredAnchor> = Vec::new();
        for range in line_ranges {
            match merged.last_mut() {
                Some(current) if range.start_line <= current.end_line.saturating_add(1) => {
                    current.end_line = current.end_line.max(range.end_line);
                    current.start_line = current.start_line.min(range.start_line);
                }
                _ => merged.push(range),
            }
        }
        structured.extend(merged);
        structured.extend(whole_file);
    }

    let rendered: Vec<String> = structured
        .iter()
        .map(|a| format!("{}#L{}-L{}", a.path, a.start_line, a.end_line))
        .collect();
    (rendered, structured)
}

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
                structured_targets.push(draft::StructuredAnchor {
                    path: anchor_rel,
                    start_line: start,
                    end_line: end,
                });
            }
            let (target_anchors, structured_targets) = coalesce_line_ranges(structured_targets);
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
    // into the applied stage list whenever consolidation prunes a duplicate
    // the dedup already suffixed.
    let mut consolidated: Vec<MeshDraft> = Vec::new();
    for (start, end) in page_spans {
        let page_drafts: Vec<MeshDraft> = all_drafts[start..end].to_vec();
        consolidated.extend(group::consolidate_within_page(page_drafts));
    }

    consolidated
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
        long.starts_with(short) && long.as_bytes().get(short.len()) == Some(&b'/')
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
/// Returns `Some(target)` with the chosen mesh-dir-relative target `B/<leaf>`.
/// Falls back to `B/index` (then `B/index-2`, … capped at 99) when the leaf
/// is empty, equals `B`'s last segment, or `B/<leaf>` would itself collide.
/// Returns `None` when all 99 numeric index slots are occupied.
fn derive_rename_target(
    mesh_dir: &Path,
    blocker: &str,
    wiki_anchors: &[(String, u32, u32)],
    section_noun: &std::collections::HashMap<(String, u32, u32), String>,
) -> Option<String> {
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
        if !mesh_dir.join(&cand).is_file() && !mesh_fs_prefix_collision(mesh_dir, &cand) {
            return Some(cand);
        }
    }

    // Numeric `index` fallback.
    let base = format!("{blocker}/index");
    if !mesh_dir.join(&base).is_file() && !mesh_fs_prefix_collision(mesh_dir, &base) {
        return Some(base);
    }
    for n in 2..=99 {
        let cand = format!("{blocker}/index-{n}");
        if !mesh_dir.join(&cand).is_file() && !mesh_fs_prefix_collision(mesh_dir, &cand) {
            return Some(cand);
        }
    }
    if !mesh_dir.join(&base).is_file() { Some(base) } else { None }
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
    // repo_root is the parent of wiki_dir: wiki_dir = repo_root/.wiki
    let repo_root = mesh_dir.parent()?;
    let mesh_file = store::read_one(repo_root, &blocker).ok()??;
    // Extract distinct .md anchor triples from the parsed MeshFile.
    let wiki_anchors: Vec<(String, u32, u32)> = {
        let mut out: Vec<(String, u32, u32)> = Vec::new();
        for anchor in &mesh_file.anchors {
            if anchor.path.ends_with(".md") {
                let triple = (anchor.path.clone(), anchor.start_line, anchor.end_line);
                if !out.contains(&triple) {
                    out.push(triple);
                }
            }
        }
        out
    };
    let target = derive_rename_target(mesh_dir, &blocker, &wiki_anchors, section_noun)?;
    Some(PlannedRename {
        from: blocker,
        to: target,
        for_slug: slug.to_string(),
        page: page.to_string(),
    })
}

/// Execute a planned blocker rename in-process.
///
/// Reads the blocker mesh, writes it to `plan.to`, and deletes the old slug.
/// If either the write or delete fails, the operation fails open and the caller
/// drops the draft with a SlugPathCollision advisory.
///
/// Returns `true` only when the blocker ends up at `plan.to`.
fn run_blocker_rename(repo_root: &Path, _mesh_dir: &Path, plan: &PlannedRename) -> bool {
    // Read the blocker.
    let mesh = match store::read_one(repo_root, &plan.from) {
        Ok(Some(m)) => m,
        _ => return false,
    };
    // The rename target may need `plan.from`'s path as a directory component
    // (e.g. renaming `wiki/foo` to `wiki/foo/index` requires `wiki/foo` to
    // become a directory). Delete the old location first so `create_dir_all`
    // can succeed, then write the new location.
    if store::delete(repo_root, &plan.from).is_err() {
        return false;
    }
    // Write to the new location. If it fails, attempt to restore the original
    // mesh at `plan.from`. `store::write` is atomic (temp file + persist), so
    // the mesh content is intact in memory and can be safely re-written.
    if store::write(repo_root, &plan.to, &mesh).is_err() {
        // Rollback: restore the original mesh at its old location.
        if let Err(e) = store::write(repo_root, &plan.from, &mesh) {
            eprintln!(
                "failed to restore original mesh `{}` after write to `{}` failed: {e}",
                plan.from, plan.to,
            );
        }
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
    git_reader: Option<&GitReader>,
) -> (FileMeta, Option<ParseErrorKind>) {
    let text = match read_via_source(path, repo_root, source, git_reader) {
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
    let title = parse_frontmatter_field(&text);
    if title.is_none() {
        return (FileMeta::default(), Some(ParseErrorKind::Malformed));
    }

    let meta = FileMeta { title };
    (meta, None)
}

/// Extract the `title` field value from YAML frontmatter.
///
/// The frontmatter block is located by finding the opening `---` fence at the
/// start of the content and the next `\n---` closing fence.  The title regex
/// is compiled once in a [`OnceLock`] and reused across all calls.
fn parse_frontmatter_field(content: &str) -> Option<String> {
    // ── Locate the frontmatter block ──────────────────────────────────────
    let after_open = content.strip_prefix("---")?;
    // Consume optional horizontal whitespace between `---` and the newline
    // (mirrors the `\s*` in the original `\A---\s*\n` anchor).
    let after_open = after_open.trim_start_matches([' ', '\t', '\r']);
    let after_open = after_open.strip_prefix('\n')?;

    // Find the closing `---` fence so the regex only sees the frontmatter
    // block — it cannot leak past the fence into body text.
    let fm_end = after_open.find("\n---")?;
    let frontmatter = &after_open[..fm_end];

    // ── Extract the title field ───────────────────────────────────────────
    static TITLE_RE: OnceLock<Regex> = OnceLock::new();
    let re = TITLE_RE.get_or_init(|| Regex::new(r"(?m)^title:\s*(.+?)\s*$").unwrap());
    let cap = re.captures(frontmatter)?;
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
/// file before writing it to the store.
///
/// `cache` avoids re-reading the same target file when multiple anchors across
/// drafts reference it — each file is read once for bounds validation, not
/// once per anchor.
///
/// Returns `Some(detail)` describing why the anchor is invalid, or `None`
/// when the anchor is acceptable. `None` here means "let it through" — a
/// genuine store write failure on an otherwise-valid anchor still fails
/// closed downstream.
fn invalid_anchor_detail(
    repo_root: &Path,
    anchor: &draft::StructuredAnchor,
    source: DocSource,
    source_paths: &Option<std::collections::HashSet<String>>,
    cache: &mut store::FileContentCache,
    git_reader: Option<&GitReader>,
) -> Option<String> {
    let (start, end) = (anchor.start_line, anchor.end_line);
    // start < 1 (anchors use 1-based inclusive line numbers).
    if start < 1 {
        return Some(format!("start line {start} is below 1"));
    }
    // Inverted / zero-length range.
    if start > end {
        return Some(format!("start line {start} exceeds end line {end}"));
    }
    // Over-range: end beyond the file's line count.
    let abs = repo_root.join(&anchor.path);
    let owned_content;
    let content: &str = if source_paths.is_none() {
        // WorkingTree — read from disk via the per-run cache.
        let cached = match cache.get_or_read(&abs) {
            Ok(c) => c,
            Err(e) => {
                return Some(format!("cannot read {}: {e}", abs.display()));
            }
        };
        match cached.utf8() {
            Ok(s) => s,
            Err(e) => return Some(format!("{e}")),
        }
    } else {
        // Index / Head — read the git object snapshot so the line count matches
        // discovery. Insert into the cache so hash_anchor (called later in
        // apply_drafts) uses the same git-sourced content, not the worktree.
        owned_content = read_via_source(&abs, repo_root, source, git_reader).ok()?;
        cache.insert(abs, owned_content.as_bytes().to_vec());
        &owned_content
    };
    let line_count = count_lines(content);
    if u64::from(end) > line_count {
        return Some(format!("end exceeds file line count {line_count}"));
    }
    None
}

/// Count the lines in `content` using inclusive line-count semantics: a trailing
/// newline does not introduce a phantom final line, and empty content has zero lines.
fn count_lines(content: &str) -> u64 {
    if content.is_empty() {
        return 0;
    }
    let mut n = content.lines().count() as u64;
    // `str::lines` already drops a single trailing newline's empty segment, so
    // "a\n" → 1 line, "a\nb" → 2 lines, "a\nb\n" → 2 lines.
    if n == 0 {
        n = 1;
    }
    n
}

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
    fn derive_rename_target_reuses_section_noun() {
        let tmp = tempfile::tempdir().unwrap();
        let md = tmp.path();
        let mut nouns = std::collections::HashMap::new();
        nouns.insert(
            ("wiki/arch/page.md".to_string(), 5, 12),
            "helper".to_string(),
        );
        let target = derive_rename_target(
            md,
            "wiki/arch/scaff",
            &[("wiki/arch/page.md".to_string(), 5, 12)],
            &nouns,
        );
        assert_eq!(target, Some("wiki/arch/scaff/helper".to_string()));
    }

    #[test]
    fn derive_rename_target_index_when_no_unique_anchor() {
        let tmp = tempfile::tempdir().unwrap();
        let md = tmp.path();
        let nouns = std::collections::HashMap::new();
        assert_eq!(
            derive_rename_target(md, "wiki/b", &[], &nouns),
            Some("wiki/b/index".to_string())
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
        assert_eq!(target, Some("wiki/b/index".to_string()));
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
            Some("wiki/b/index-2".to_string())
        );
    }

    #[test]
    fn derive_rename_target_falls_back_when_leaf_equals_blocker_last() {
        let tmp = tempfile::tempdir().unwrap();
        let md = tmp.path();
        let mut nouns = std::collections::HashMap::new();
        nouns.insert(("p.md".to_string(), 1, 1), "scaff".to_string());
        // Leaf would equal blocker's last segment `scaff` → use `index`.
        let target = derive_rename_target(md, "wiki/scaff", &[("p.md".to_string(), 1, 1)], &nouns);
        assert_eq!(target, Some("wiki/scaff/index".to_string()));
    }

    #[test]
    fn derive_rename_target_exhausted_slots_returns_occupied_path() {
        let tmp = tempfile::tempdir().unwrap();
        let md = tmp.path();
        std::fs::create_dir_all(md.join("wiki/b")).unwrap();
        // Fill all 99 numeric index slots: index, index-2 .. index-99.
        std::fs::write(md.join("wiki/b/index"), "x").unwrap();
        for n in 2..=99 {
            std::fs::write(md.join(format!("wiki/b/index-{n}")), "x").unwrap();
        }
        let nouns = std::collections::HashMap::new();
        let result = derive_rename_target(md, "wiki/b", &[], &nouns);
        // All 99 numeric index slots are occupied — the function must return
        // `None` so the caller (plan_blocker_rename) propagates it as a
        // fail-open drop-with-advisory.
        assert!(
            result.is_none(),
            "expected None when all 99 index slots are occupied, got `{result:?}`"
        );
    }

    #[test]
    fn plan_blocker_rename_ok_for_ancestor_file() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Store a blocker mesh at slug "wiki/arch/scaff" under root/.wiki/.
        super::store::write(
            root,
            "wiki/arch/scaff",
            &git_mesh_core::mesh_file::MeshFile {
                anchors: vec![super::store::anchor_record(
                    "src/lib.rs".to_string(),
                    git_mesh_core::AnchorExtent::LineRange { start: 1, end: 1 },
                    "abc".to_string(),
                )],
                why: "why".to_string(),
            },
        )
        .unwrap();
        let mesh_dir = super::store::wiki_dir(root);
        let nouns = std::collections::HashMap::new();
        let plan =
            plan_blocker_rename(&mesh_dir, "wiki/arch/scaff/helper", "p.md", &nouns).unwrap();
        assert_eq!(plan.from, "wiki/arch/scaff");
        assert_eq!(plan.to, "wiki/arch/scaff/index");
        assert_eq!(plan.for_slug, "wiki/arch/scaff/helper");
    }

    #[test]
    fn plan_blocker_rename_none_for_dir_at_slug_path() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let mesh_dir = super::store::wiki_dir(root);
        std::fs::create_dir_all(mesh_dir.join("wiki/scaffold/child")).unwrap();
        let nouns = std::collections::HashMap::new();
        // `wiki/scaffold` is a directory, not an ancestor file → fail open.
        assert!(plan_blocker_rename(&mesh_dir, "wiki/scaffold", "p.md", &nouns).is_none());
    }

    #[test]
    fn plan_blocker_rename_none_for_unreadable_blocker() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let mesh_dir = super::store::wiki_dir(root);
        std::fs::create_dir_all(mesh_dir.join("wiki")).unwrap();
        // Non-UTF8 payload → `MeshFile::parse` fails → fail open.
        std::fs::write(mesh_dir.join("wiki/b"), [0xff, 0xfe, 0x00]).unwrap();
        let nouns = std::collections::HashMap::new();
        assert!(plan_blocker_rename(&mesh_dir, "wiki/b/leaf", "p.md", &nouns).is_none());
    }

    #[test]
    fn run_blocker_rename_preserves_from_on_write_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Create a mesh at the `from` slug.
        let mesh = git_mesh_core::mesh_file::MeshFile {
            anchors: vec![],
            why: "curated why".to_string(),
        };
        super::store::write(root, "wiki/a/file", &mesh).unwrap();
        // Create the target parent directory and make it read-only so that
        // NamedTempFile::new_in(parent) fails when store::write tries to
        // create a temp file there.
        let to_parent = super::store::wiki_dir(root).join("wiki/a/dir");
        std::fs::create_dir_all(&to_parent).unwrap();
        let mut perms = std::fs::metadata(&to_parent).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&to_parent, perms).unwrap();
        let plan = PlannedRename {
            from: "wiki/a/file".to_string(),
            to: "wiki/a/dir/file".to_string(),
            for_slug: "wiki/a/dir".to_string(),
            page: "p.md".to_string(),
        };
        let mesh_dir = super::store::wiki_dir(root);
        assert!(
            !run_blocker_rename(root, &mesh_dir, &plan),
            "rename should fail when target parent is read-only"
        );
        // Bug: the original mesh was deleted before the write attempt and is
        // never restored. This assertion FAILS against current code.
        let restored = super::store::read_one(root, "wiki/a/file").unwrap();
        assert!(
            restored.is_some(),
            "original mesh must survive a failed blocker rename"
        );
        assert_eq!(restored.unwrap().why, "curated why");
    }

    #[test]
    fn parse_frontmatter_extracts_title() {
        let c = "---\ntitle: Hello World\nsummary: A page summary.\n---\nbody";
        assert_eq!(
            parse_frontmatter_field(c),
            Some("Hello World".into())
        );
    }

    #[test]
    fn parse_frontmatter_handles_quoted_values() {
        let c = "---\ntitle: \"Quoted Title\"\n---\n";
        assert_eq!(
            parse_frontmatter_field(c),
            Some("Quoted Title".into())
        );
    }

    #[test]
    fn parse_frontmatter_returns_none_when_absent() {
        assert!(parse_frontmatter_field("no frontmatter here").is_none());
    }

    #[test]
    fn parse_frontmatter_ignores_thematic_break_in_body() {
        // Body contains a `---` separator followed by a `title:` line — must NOT match.
        let c = "# Heading\n\nbody text\n\n---\ntitle: Spurious\n\nmore body\n";
        assert_eq!(parse_frontmatter_field(c), None);
    }

    #[test]
    fn parse_frontmatter_does_not_cross_closing_fence() {
        // The regex must not leak past a closing `---` fence to match a
        // `title:` line in a later frontmatter block.
        let c = "---\nsummary: No title here\n---\ntitle: Evil\n";
        assert_eq!(parse_frontmatter_field(c), None);
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
        let (_, kind) = classify_frontmatter(tmp.path(), &dir, DocSource::WorkingTree, None);
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
            classify_frontmatter(tmp.path(), &std::env::temp_dir(), DocSource::WorkingTree, None);
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
        assert_eq!(drafts[0].slug, "charge-handler");
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
            ["charge-handler".to_string()].into_iter().collect();
        let exists = |slug: &str| existing.contains(slug);
        resolve_slug_collisions(&mut drafts, &titles, &exists);
        assert_eq!(drafts[0].slug, "checkout/charge-handler");
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
            "charge-handler".to_string(),
            "checkout/charge-handler".to_string(),
        ]
        .into_iter()
        .collect();
        let exists = |slug: &str| existing.contains(slug);
        resolve_slug_collisions(&mut drafts, &titles, &exists);
        assert_eq!(drafts[0].slug, "payments/checkout/charge-handler");
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
            "charge-handler".to_string(),
            "checkout/charge-handler".to_string(),
        ]
        .into_iter()
        .collect();
        let exists = |slug: &str| existing.contains(slug);
        resolve_slug_collisions(&mut drafts, &titles, &exists);
        assert_eq!(drafts[0].slug, "billing-service/checkout/charge-handler");
    }

    #[test]
    fn collision_resolver_falls_back_to_digit_suffix() {
        let mut drafts = vec![make_draft("wiki/billing.md", "charge-handler", "", vec![])];
        let titles: std::collections::HashMap<String, Option<String>> =
            std::collections::HashMap::new();
        let existing: std::collections::HashSet<String> =
            ["charge-handler".to_string()].into_iter().collect();
        let exists = |slug: &str| existing.contains(slug);
        resolve_slug_collisions(&mut drafts, &titles, &exists);
        // No chain, no title — only the base slug is a unique candidate,
        // so the digit fallback runs against it.
        assert_eq!(drafts[0].slug, "charge-handler-2");
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
        assert_eq!(drafts[0].slug, "foo");
        assert_eq!(drafts[1].slug, "foo-2");
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
        assert_eq!(drafts[0].slug, "foo");
        // Second clashes only with the first draft (in-run); the parent
        // heading "Second" resolves it without needing a digit.
        assert_eq!(drafts[1].slug, "second/foo");
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
            ["mesh/leaf".to_string()].into_iter().collect();
        let exists = |slug: &str| existing.contains(slug);
        resolve_slug_collisions(&mut drafts, &titles, &exists);
        assert_eq!(drafts[0].slug, "mesh/outer/leaf");
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
        let detail = invalid_anchor_detail(
            repo_root,
            &anchor,
            DocSource::WorkingTree,
            &None,
            &mut store::FileContentCache::new(),
            None,
        );
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
            invalid_anchor_detail(
                repo_root,
                &anchor,
                DocSource::WorkingTree,
                &None,
                &mut store::FileContentCache::new(),
                None,
            ),
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
            invalid_anchor_detail(
                repo_root,
                &anchor,
                DocSource::WorkingTree,
                &None,
                &mut store::FileContentCache::new(),
                None,
            ),
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

    // ── mesh_has_why ──────────────────────────────────────────────────────────

    #[test]
    fn mesh_has_why_false_for_scaffold_shape() {
        // Anchor lines, one blank line, EOF — no prose.
        let text = "wiki/page.md#L1-L5 sha256:abc\n\n";
        assert!(!mesh_has_why(text));
    }

    #[test]
    fn mesh_has_why_false_for_no_blank() {
        // No blank line at all — definitely no why.
        let text = "wiki/page.md#L1-L5 sha256:abc\n";
        assert!(!mesh_has_why(text));
    }

    #[test]
    fn mesh_has_why_true_for_curated_shape() {
        // Anchor lines, blank line, then why sentence.
        let text = "wiki/page.md#L1-L5 sha256:abc\n\nFlow that carries a charge from browser.\n";
        assert!(mesh_has_why(text));
    }

    // ── cleanup_orphaned_meshes classification ────────────────────────────────

    #[test]
    fn cleanup_eligible_when_single_page_gone() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Meshes live under .wiki/ (the in-process store location).
        super::store::write(
            root,
            "wiki-page",
            &git_mesh_core::mesh_file::MeshFile {
                anchors: vec![super::store::anchor_record(
                    "wiki/page.md".to_string(),
                    git_mesh_core::AnchorExtent::LineRange { start: 1, end: 5 },
                    "abc".to_string(),
                )],
                why: String::new(),
            },
        )
        .unwrap();

        // Page does NOT exist on disk — eligible for deletion.
        let result = cleanup_orphaned_meshes(root, root, &[], true).unwrap();
        assert_eq!(result.planned_deletions, vec![".wiki/wiki-page"]);
        assert!(result.advisories.is_empty());
    }

    #[test]
    fn cleanup_advisory_when_has_why() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Curated mesh — has why after blank line.
        super::store::write(
            root,
            "wiki-curated",
            &git_mesh_core::mesh_file::MeshFile {
                anchors: vec![super::store::anchor_record(
                    "wiki/page.md".to_string(),
                    git_mesh_core::AnchorExtent::LineRange { start: 1, end: 5 },
                    "abc".to_string(),
                )],
                why: "Curated reason here.".to_string(),
            },
        )
        .unwrap();

        // Page does NOT exist — but ineligible due to why.
        let result = cleanup_orphaned_meshes(root, root, &[], true).unwrap();
        assert!(result.planned_deletions.is_empty());
        assert!(
            result.advisories.contains("curated why prose"),
            "expected curated-why advisory; got: {}",
            result.advisories
        );
    }

    #[test]
    fn cleanup_advisory_when_multi_page_one_gone() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Multi-page mesh: two distinct .md paths, one gone.
        super::store::write(
            root,
            "shared",
            &git_mesh_core::mesh_file::MeshFile {
                anchors: vec![
                    super::store::anchor_record(
                        "wiki/a.md".to_string(),
                        git_mesh_core::AnchorExtent::LineRange { start: 1, end: 3 },
                        "abc".to_string(),
                    ),
                    super::store::anchor_record(
                        "wiki/b.md".to_string(),
                        git_mesh_core::AnchorExtent::LineRange { start: 2, end: 4 },
                        "def".to_string(),
                    ),
                ],
                why: String::new(),
            },
        )
        .unwrap();
        // wiki/a.md is absent; wiki/b.md present.
        std::fs::create_dir_all(root.join("wiki")).unwrap();
        std::fs::write(root.join("wiki/b.md"), "present\n").unwrap();

        let result = cleanup_orphaned_meshes(root, root, &[], true).unwrap();
        assert!(result.planned_deletions.is_empty());
        assert!(
            result.advisories.contains("other pages"),
            "expected multi-page advisory; got: {}",
            result.advisories
        );
    }

    #[test]
    fn cleanup_leave_when_page_present() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Scaffold mesh — page IS present on disk.
        std::fs::create_dir_all(root.join("wiki")).unwrap();
        std::fs::write(root.join("wiki/page.md"), "# Present\n").unwrap();
        super::store::write(
            root,
            "wiki-page",
            &git_mesh_core::mesh_file::MeshFile {
                anchors: vec![super::store::anchor_record(
                    "wiki/page.md".to_string(),
                    git_mesh_core::AnchorExtent::LineRange { start: 1, end: 3 },
                    "abc".to_string(),
                )],
                why: String::new(),
            },
        )
        .unwrap();

        let result = cleanup_orphaned_meshes(root, root, &[], true).unwrap();
        assert!(result.planned_deletions.is_empty());
        assert!(result.advisories.is_empty());
    }

    #[test]
    fn cleanup_leave_silently_when_no_md_anchor() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Hand-authored mesh with only non-.md anchors.
        super::store::write(
            root,
            "hand-authored",
            &git_mesh_core::mesh_file::MeshFile {
                anchors: vec![super::store::anchor_record(
                    "src/lib.rs".to_string(),
                    git_mesh_core::AnchorExtent::LineRange { start: 1, end: 5 },
                    "abc".to_string(),
                )],
                why: String::new(),
            },
        )
        .unwrap();

        let result = cleanup_orphaned_meshes(root, root, &[], true).unwrap();
        assert!(result.planned_deletions.is_empty());
        assert!(result.advisories.is_empty());
    }

    // ── range coalescing (oscillation-prevention) ─────────────────────────────

    /// `build_meshes` must coalesce overlapping/contiguous fragment-link ranges
    /// on the same path within a section — matching `git-mesh-core`'s rule (merge
    /// when `next.start <= current.end + 1`). Without coalescing, a section linking
    /// `card.ts#L69-L95`, `card.ts#L75-L75`, `card.ts#L81-L81` emits three
    /// separate anchors that `git span drift --fix` re-collapses into one
    /// `card.ts#L69-L95`, so the two tools oscillate forever.
    #[test]
    fn build_meshes_coalesces_overlapping_ranges_per_path() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        // Real target file so anchor-path resolution keeps `card.ts`.
        std::fs::write(root.join("card.ts"), "// card\n".repeat(100)).unwrap();

        // Wiki page: one section with three fragment links into overlapping /
        // contiguous ranges of the same file.
        let page_rel = "page.md";
        let content = "\
## Section heading

See [a](./card.ts#L69-L95), [b](./card.ts#L75-L75), [c](./card.ts#L81-L81).
";
        let page_abs = root.join(page_rel);
        std::fs::write(&page_abs, content).unwrap();

        // Build LinkInputs exactly as `run` does: parse + augment, keep internal
        // links with a parsed start line.
        let raw_links = parse_fragment_links(content);
        let augmented = augment(&raw_links, content);
        let mut inputs: Vec<LinkInput> = Vec::new();
        for aug in augmented {
            if aug.link.kind != LinkKind::Internal {
                continue;
            }
            if aug.link.start_line.is_none() {
                continue;
            }
            inputs.push(LinkInput {
                wiki_file: page_abs.clone(),
                augmented: aug,
            });
        }
        assert_eq!(inputs.len(), 3, "fixture should yield three fragment links");

        let mut page_subdirs: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        page_subdirs.insert(page_rel.to_string(), String::new());

        let drafts = build_meshes(&inputs, root, &page_subdirs);
        assert_eq!(drafts.len(), 1, "expected a single section draft");

        // Code anchors follow the leading page-section anchor.
        let code_anchors: Vec<&str> = drafts[0]
            .anchors
            .iter()
            .skip(1)
            .map(|s| s.as_str())
            .collect();

        // 69-95 covers 75 and 81 (overlap / containment), so the three ranges
        // must collapse to one covering anchor.
        assert_eq!(
            code_anchors,
            vec!["card.ts#L69-L95"],
            "overlapping/contiguous ranges on the same path must coalesce into one anchor"
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

        // BUG: result is Some("src/lib.rs") because the suffix
        // loop checks in-repo existence without verifying the
        // candidate is a meaningful suffix of the original path
        // that also remains within the repo.
        assert_eq!(
            result,
            None,
            "locate_existing_suffix must not match in-repo files \
             for paths that resolve outside the repo"
        );
    }

    #[test]
    fn test_scaffold_strips_wikiignored_anchors() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo_root = tmp.path();
        std::process::Command::new("git")
            .arg("init")
            .arg("-q")
            .current_dir(repo_root)
            .status()
            .unwrap();
        std::fs::create_dir_all(repo_root.join("src")).unwrap();
        std::fs::write(repo_root.join("src/foo.rs"), "fn a() {}\nfn b() {}\n").unwrap();
        std::fs::create_dir_all(repo_root.join("wiki")).unwrap();
        std::fs::write(
            repo_root.join("wiki/page.md"),
            "---\ntitle: Page\nsummary: P.\n---\n\n[foo](../src/foo.rs#L1-L2)\n",
        )
        .unwrap();
        std::fs::create_dir_all(repo_root.join(".wiki")).unwrap();
        std::fs::write(repo_root.join(".wiki/.wikiignore"), "src/foo.rs\n").unwrap();

        let files = vec![repo_root.join("wiki/page.md")];
        let outcome = create_mesh_coverage(
            &files,
            repo_root,
            DocSource::WorkingTree,
            true, // dry_run
            None,
            None,
        )
        .expect("create_mesh_coverage");

        assert!(
            outcome.planned.is_empty(),
            "no mesh should be planned for a wikiignored anchor target: {:?}",
            outcome.planned
        );
    }
}
