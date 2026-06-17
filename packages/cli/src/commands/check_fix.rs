use std::cmp::Reverse;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use miette::Result;
use serde::Serialize;

use git_mesh_core::{
    AnchorExtent, LineIndex, cheap_fingerprint_indexed, rk64_from_hex, scan_indexed_rk64,
};
use git_mesh_core::mesh_file::{
    has_conflict_markers, merge_mesh_files, AnchorRecord, MeshFile, UnresolvedAnchor,
};

use crate::commands::mesh::store;
use crate::commands::mesh_coverage::build_mesh_index;
use crate::frontmatter::parse_frontmatter;
use crate::git::GitReader;
use super::check::ContentCache;
use crate::headings::{extract_headings, github_slug, resolve_heading, Heading};
use crate::index::DocSource;
use crate::parser::{LinkKind, parse_fragment_links};

// ── Types ─────────────────────────────────────────────────────────────────────

/// What kind of rewrite the fix performs.
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub enum FixKind {
    /// Fix 1: rewrite a broken link whose target was renamed.
    BrokenLinkRename,
    /// Fix 2: update a line-range anchor that drifted due to line insertions/deletions.
    MeshAnchorShift,
    /// Fix 3: rewrite an alias href to the canonical slug.
    AliasToCanonical,
    /// Fix 5: update a heading anchor that was renamed in-place (same position).
    HeadingRename,
    /// Fix 6: refresh a hash that drifted due to whitespace-only in-place edit.
    AnchorContentRefresh,
}

/// Classification of a stale anchor in [`plan_mesh_follows`].
#[derive(Debug)]
pub enum AnchorDriftKind {
    /// Content changed in place; whitespace-only diff → auto-re-anchored by --fix.
    Changed { whitespace_only: bool },
    /// Content changed in place, but the pre-edit bytes could not be recovered
    /// from history (shallow clone, or the edit is older than
    /// [`HISTORY_SCAN_LIMIT`] commits), so whitespace-equivalence cannot be
    /// proven. Fail-closed: NOT auto-settled, but distinguished from a confirmed
    /// meaning change so the diagnostic is honest about the cause.
    Unrecoverable,
    /// File deleted or line range out of bounds.
    Deleted,
}

/// One anchor whose stored hash no longer matches its current content and which
/// did not relocate to a unique new position (so it is not a [`MeshMovePlan`]).
#[derive(Debug)]
pub struct AnchorDrift {
    pub mesh_name: String,
    /// Repo-relative path the anchor names.
    pub path: String,
    /// 1-based start line (0 for whole-file).
    pub start: u32,
    /// 1-based end line (0 for whole-file).
    pub end: u32,
    pub kind: AnchorDriftKind,
}

/// How confident the fixer is that the proposed rewrite is correct.
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub enum Confidence {
    /// One unambiguous rename; safe to apply automatically.
    High,
    /// Plausible; one replacement at the same structural position.
    Medium,
}

/// A rewrite that the fixer determined is safe to apply.
#[derive(Debug, Serialize)]
pub struct Fix {
    /// Repo-relative path to the file that will be rewritten.
    pub file: String,
    /// 1-based line number of the link in the source file.
    pub line: usize,
    /// The kind of fix being applied.
    pub kind: FixKind,
    /// Absolute byte offset in file content where the old href begins.
    pub byte_start: usize,
    /// Absolute byte offset in file content where the old href ends.
    pub byte_end: usize,
    /// The old href text (as it appears in the source).
    pub old_href: String,
    /// The new href text that replaces it.
    pub new_href: String,
    /// Human-readable explanation of why this fix was applied.
    pub reason: String,
    /// How confident the fixer is.
    pub confidence: Confidence,
}

/// A fix that was skipped because it could not be applied safely.
#[derive(Debug, Serialize)]
pub struct SkippedFix {
    /// Repo-relative path to the file that would have been rewritten.
    pub file: String,
    /// 1-based line number of the link in the source file.
    pub line: usize,
    /// The kind of fix that was attempted.
    pub kind: FixKind,
    /// Human-readable explanation of why the fix was skipped.
    pub reason: String,
}

/// The result of a fix pass: what was applied and what was skipped.
///
/// `mesh` carries the outcome of Fix #4 (mesh coverage creation), which runs
/// after the link/anchor patches are materialized to disk. `check` owns its
/// output (applied/planned paths, advisories, failures).
#[derive(Debug)]
pub struct FixPlan {
    pub fixes: Vec<Fix>,
    pub skipped: Vec<SkippedFix>,
    pub(crate) mesh: super::mesh::scaffold::MeshCoverageOutcome,
    /// Number of ambiguous (≥ 2-match) move-follow conflicts surfaced during the
    /// pass. Each is also recorded in `skipped` with an actionable message; this
    /// count drives a non-zero exit so the conflict fails pre-commit/CI (the
    /// `wiki check --fix` hot path). Fail-closed: a conflict never auto-rewrites.
    pub mesh_conflicts: usize,
    /// Number of `Changed { whitespace_only: false }` + `Deleted` anchors that
    /// `--fix` cannot auto-settle. Each is also recorded in `skipped` with an
    /// actionable re-anchor command; this count drives a non-zero exit so the
    /// drift fails pre-commit/CI. Per the `mesh-conflict-exit-gate` mesh, this
    /// new fail-closed category must be wired into both fix-mode exit gates by
    /// hand (the field can be ignored without any compiler error).
    pub anchor_stale_count: usize,
}

// ── Conflict resolution ─────────────────────────────────────────────────────────

/// The outcome of a mesh conflict-resolution pass.
#[derive(Debug)]
pub struct ConflictResolutionReport {
    /// Slugs of meshes that were fully resolved (merged cleanly).
    pub resolved: Vec<String>,
    /// Slugs of meshes that were partially resolved (residue remains).
    pub partial: Vec<String>,
    /// Slugs + reason for meshes that could not be resolved at all.
    pub skipped: Vec<(String, String)>,
}

/// Walk `.wiki/` and resolve every mesh file that carries git conflict markers.
///
/// For each conflicted mesh:
///
/// 1. Split the raw text into ours/theirs content by parsing three-way conflict
///    marker blocks (`<<<<<<<` / `=======` / `>>>>>>>`). Diff3 base markers
///    (`|||||||`) are discarded.
///
/// 2. Parse each side with `MeshFile::parse()`. If either side fails, skip.
///
/// 3. Collect every unique `anchor.path` from both parsed meshes, read each
///    source file from the worktree, and check `has_conflict_markers()` on its
///    content. If any source file has markers, skip (the source is itself
///    conflicted — can't authoritatively re-hash).
///
/// 4. Call `merge_mesh_files(None, &ours, &theirs, &source_files)` with the
///    clean worktree file bytes.
///
/// 5. Partition unresolved:
///    - `u.path.is_empty()` → `--why` sentinel → residue
///    - Non-empty path → same-anchor divergence → try re-hashing the worktree
///      file directly; if it matches one side, accept that side; otherwise,
///      keep as residue.
///
/// 6. Fully resolved meshes are written via `store::write()` and `git add`-ed.
///    Partial meshes are written with resolved anchors clean and only the
///    residue wrapped in conflict markers (NOT staged).
///
/// A missing `.wiki/` directory returns a report with all-empty fields.
pub fn resolve_conflicted_meshes(repo_root: &Path) -> Result<ConflictResolutionReport> {
    let conflicted = store::find_conflicted_meshes(repo_root)?;
    if conflicted.is_empty() {
        return Ok(ConflictResolutionReport {
            resolved: Vec::new(),
            partial: Vec::new(),
            skipped: Vec::new(),
        });
    }

    let mut report = ConflictResolutionReport {
        resolved: Vec::new(),
        partial: Vec::new(),
        skipped: Vec::new(),
    };

    'next_mesh: for (slug, raw_text) in &conflicted {
        // Step 1: Split markers into ours/theirs content.
        let (ours_text, theirs_text) = match split_conflict_blocks(raw_text) {
            Ok(pair) => pair,
            Err(e) => {
                report
                    .skipped
                    .push((slug.clone(), format!("failed to split markers: {e}")));
                continue;
            }
        };

        // Step 2: Parse each side.
        let ours = match MeshFile::parse(&ours_text) {
            Ok(m) => m,
            Err(e) => {
                report
                    .skipped
                    .push((slug.clone(), format!("ours side failed to parse: {e}")));
                continue;
            }
        };
        let theirs = match MeshFile::parse(&theirs_text) {
            Ok(m) => m,
            Err(e) => {
                report
                    .skipped
                    .push((slug.clone(), format!("theirs side failed to parse: {e}")));
                continue;
            }
        };

        // Step 3: Clean-source precondition — read every source file named by
        // both sides and verify none are themselves conflicted.
        let mut seen_paths: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let mut source_paths: Vec<String> = Vec::new();
        for anchor in ours.anchors.iter().chain(theirs.anchors.iter()) {
            if seen_paths.insert(&anchor.path) {
                source_paths.push(anchor.path.clone());
            }
        }

        let mut source_files: Vec<(String, Vec<u8>)> = Vec::new();
        for sp in &source_paths {
            let abs = repo_root.join(sp);
            let bytes = match std::fs::read(&abs) {
                Ok(b) => b,
                Err(_) => {
                    report.skipped.push((
                        slug.clone(),
                        format!("source file `{sp}` not found in worktree"),
                    ));
                    continue 'next_mesh;
                }
            };
            let text = match String::from_utf8(bytes) {
                Ok(t) => t,
                Err(_) => {
                    report.skipped.push((
                        slug.clone(),
                        format!("source file `{sp}` is not valid UTF-8"),
                    ));
                    continue 'next_mesh;
                }
            };
            if has_conflict_markers(&text) {
                report.skipped.push((
                    slug.clone(),
                    format!("source file `{sp}` has its own conflict markers"),
                ));
                continue 'next_mesh;
            }
            source_files.push((sp.clone(), text.into_bytes()));
        }

        // Step 4: Merge.
        let result = merge_mesh_files(None, &ours, &theirs, &source_files);

        // Step 5: Partition unresolved.
        let (unresolved_why, unresolved_anchors): (Vec<_>, Vec<_>) = result
            .unresolved
            .into_iter()
            .partition(|u| u.path.is_empty());

        let mut residual = unresolved_anchors;

        // For same-anchor divergence, try re-hashing the worktree file to see
        // if one side's hash matches the actual content.
        let mut resolved_extra: Vec<AnchorRecord> = Vec::new();
        residual.retain(|u| {
            let abs = repo_root.join(&u.path);
            let bytes = match std::fs::read(&abs) {
                Ok(b) => b,
                Err(_) => return true, // keep as unresolved
            };
            let extent = if u.start_line == 0 && u.end_line == 0 {
                git_mesh_core::AnchorExtent::WholeFile
            } else {
                git_mesh_core::AnchorExtent::LineRange {
                    start: u.start_line,
                    end: u.end_line,
                }
            };
            let computed =
                git_mesh_core::rk64_to_hex(git_mesh_core::cheap_fingerprint_with_extent(
                    &bytes,
                    &extent,
                ));

            // If the worktree hash matches one side, accept that side.
            if computed == u.ours.content_hash {
                resolved_extra.push(u.ours.clone());
                false // removed from residual
            } else if computed == u.theirs.content_hash {
                resolved_extra.push(u.theirs.clone());
                false
            } else {
                true // keep as unresolved
            }
        });

        // Build the final merged mesh with all resolved anchors.
        let mut final_anchors = result.merged.anchors;
        final_anchors.extend(resolved_extra);
        final_anchors.sort_by(|a, b| {
            a.path
                .cmp(&b.path)
                .then(a.start_line.cmp(&b.start_line))
                .then(a.end_line.cmp(&b.end_line))
        });
        final_anchors.dedup_by(|a, b| {
            a.path == b.path && a.start_line == b.start_line && a.end_line == b.end_line
        });

        // Step 6: Write.
        let fully_resolved = residual.is_empty() && unresolved_why.is_empty();
        if fully_resolved {
            let merged = MeshFile {
                anchors: final_anchors,
                why: result.merged.why,
            };
            store::write(repo_root, slug, &merged)?;
            // git add the mesh file.
            let mesh_path = store::wiki_dir(repo_root).join(slug);
            let status = std::process::Command::new("git")
                .current_dir(repo_root)
                .args(["add", "--"])
                .arg(&mesh_path)
                .status()
                .map_err(|e| miette::miette!("git add failed for mesh `{slug}`: {e}"))?;
            if !status.success() {
                eprintln!(
                    "warning: failed to stage mesh `{slug}` (git add exited non-zero)"
                );
                report.skipped.push((
                    slug.clone(),
                    "failed to stage: git add returned non-zero".to_string(),
                ));
                continue 'next_mesh;
            }
            report.resolved.push(slug.clone());
        } else {
            // Partial: write resolved anchors clean, wrap residue in markers.
            // Pass both sides' why texts so divergent rationales appear as
            // conflict markers the user can reconcile manually.
            let partial_output = build_partial_output(
                &final_anchors,
                &result.merged.why,
                &residual,
                &unresolved_why,
                &ours.why,
                &theirs.why,
            );
            let mesh_path = store::slug_path(repo_root, slug);
            if let Some(parent) = mesh_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    miette::miette!("failed to create {}: {e}", parent.display())
                })?;
            }
            // Write atomically via temp file + persist.
            let mut tmp = tempfile::NamedTempFile::new_in(
                mesh_path.parent().ok_or_else(|| {
                    miette::miette!("mesh slug `{slug}` has no parent directory")
                })?,
            )
            .map_err(|e| miette::miette!("failed to create temp file: {e}"))?;
            use std::io::Write as _;
            tmp.write_all(partial_output.as_bytes())
                .map_err(|e| miette::miette!("failed to write partial mesh `{slug}`: {e}"))?;
            tmp.persist(&mesh_path)
                .map_err(|e| miette::miette!("failed to persist mesh `{slug}`: {e}"))?;
            // Do NOT git add — partial meshes have conflict markers.
            report.partial.push(slug.clone());
        }
    }

    Ok(report)
}

/// Split the raw text of a mesh file with git conflict markers into
/// (ours_content, theirs_content) by parsing three-way conflict blocks.
///
/// Lines outside any conflict block appear in both outputs. Diff3 base markers
/// (`|||||||`) are discarded: content between `|||||||` and `=======` is
/// omitted from both sides.
///
/// Returns an error on unterminated marker blocks.
fn split_conflict_blocks(text: &str) -> Result<(String, String)> {
    let mut ours = String::new();
    let mut theirs = String::new();

    #[derive(Debug)]
    enum State {
        Outside,
        InOurs,
        InBase,
        InTheirs,
    }
    let mut state = State::Outside;

    for line in text.lines() {
        match state {
            State::Outside => {
                if line.starts_with("<<<<<<< ") {
                    state = State::InOurs;
                } else if line.starts_with("=======")
                    || line.starts_with(">>>>>>> ")
                    || line.starts_with("|||||||")
                {
                    // Stray marker line outside a block — skip it silently
                    // (malformed but recoverable).
                } else {
                    ours.push_str(line);
                    ours.push('\n');
                    theirs.push_str(line);
                    theirs.push('\n');
                }
            }
            State::InOurs => {
                if line.starts_with("|||||||") {
                    state = State::InBase;
                } else if line.starts_with("=======") {
                    state = State::InTheirs;
                } else if line.starts_with(">>>>>>> ") {
                    // Empty theirs block — edge case from a resolved conflict;
                    // treat as a closed block and return to outside.
                    state = State::Outside;
                } else if line.starts_with("<<<<<<< ") {
                    // Nested conflict — malformed; treat as new ours block.
                    // Push nothing, start a new ours block.
                    state = State::InOurs;
                } else {
                    ours.push_str(line);
                    ours.push('\n');
                }
            }
            State::InBase => {
                if line.starts_with("=======") {
                    state = State::InTheirs;
                }
                // Otherwise discard diff3 base content.
            }
            State::InTheirs => {
                if line.starts_with(">>>>>>> ") {
                    state = State::Outside;
                } else if line.starts_with("<<<<<<< ") {
                    // Malformed — treat as new block; flush partial theirs
                    // state then restart.
                    state = State::InOurs;
                } else {
                    theirs.push_str(line);
                    theirs.push('\n');
                }
            }
        }
    }

    if !matches!(state, State::Outside) {
        return Err(miette::miette!(
            "unterminated conflict marker block (ended in state {state:?})"
        ));
    }

    Ok((ours, theirs))
}

/// Build the text output for a partially-resolved mesh: resolved anchors
/// written cleanly, followed by conflict-marker-wrapped blocks for each
/// residual anchor, then the why text.
fn build_partial_output(
    resolved_anchors: &[AnchorRecord],
    why: &str,
    residual: &[UnresolvedAnchor],
    unresolved_why: &[UnresolvedAnchor],
    why_ours: &str,
    why_theirs: &str,
) -> String {
    let mut out = String::new();

    // Write resolved anchors in canonical order.
    for anchor in resolved_anchors {
        out.push_str(&anchor.to_string());
        out.push('\n');
    }

    // Write residual (same-anchor divergence) wrapped in conflict markers.
    for u in residual {
        out.push_str("<<<<<<< ours\n");
        out.push_str(&format!("{}\n", u.ours));
        out.push_str("=======\n");
        out.push_str(&format!("{}\n", u.theirs));
        out.push_str(">>>>>>> theirs\n");
    }

    // Blank-line separator between anchor block and why text.
    if !out.is_empty() || !why.is_empty() || !unresolved_why.is_empty() {
        out.push('\n');
    }

    // For why conflicts: wrap both sides' why texts in conflict markers so
    // the user can reconcile them manually. When there is no why conflict,
    // write the resolved `why` text cleanly.
    if unresolved_why.is_empty() {
        if !why.is_empty() {
            out.push_str(why);
            out.push('\n');
        }
    } else {
        out.push_str("<<<<<<< ours\n");
        out.push_str(why_ours);
        out.push('\n');
        out.push_str("=======\n");
        out.push_str(why_theirs);
        out.push('\n');
        out.push_str(">>>>>>> theirs\n");
    }

    out
}

// ── Rename map ────────────────────────────────────────────────────────────────

/// The result of looking up a path in the rename map.
#[derive(Debug)]
pub enum SuccessorResult {
    /// No rename recorded for this path.
    None,
    /// Exactly one rename destination found.
    Unique(PathBuf),
    /// Multiple possible rename destinations (ambiguous).
    Ambiguous(Vec<PathBuf>),
}

/// A map from old repo-relative paths to their rename successors.
///
/// Chains multi-step renames: A→B and B→C means A→C.
pub struct RenameMap {
    /// old path → new path (repo-relative strings)
    map: HashMap<String, Vec<String>>,
    /// Cache for on-demand `git log --follow` lookups.
    log_cache: HashMap<String, Vec<String>>,
    repo_root: PathBuf,
}

impl RenameMap {
    /// Build the rename map from:
    /// 1. `git diff --diff-filter=R --name-status` (worktree↔index renames)
    /// 2. `git diff --cached --diff-filter=R --name-status` (index↔HEAD renames)
    pub fn build(repo_root: &Path) -> Result<Self> {
        let mut map: HashMap<String, Vec<String>> = HashMap::new();

        // Layer 1: worktree ↔ index
        let pairs1 = run_diff_renames(repo_root, false)?;
        for (old, new) in pairs1 {
            map.entry(old).or_default().push(new);
        }

        // Layer 2: index ↔ HEAD
        let pairs2 = run_diff_renames(repo_root, true)?;
        for (old, new) in pairs2 {
            map.entry(old).or_default().push(new);
        }

        // Chain multi-step renames: if A→B and B→C, add A→C.
        // Repeat until stable (handle chains of any length).
        loop {
            let mut changed = false;
            let keys: Vec<String> = map.keys().cloned().collect();
            for key in keys {
                let destinations: Vec<String> = map[&key].clone();
                for dest in &destinations {
                    if let Some(further) = map.get(dest).cloned() {
                        for f in further {
                            let entry = map.entry(key.clone()).or_default();
                            if !entry.contains(&f) {
                                entry.push(f);
                                changed = true;
                            }
                        }
                        // Remove the intermediate dest — it chains further so
                        // it is not a terminal destination.
                        let entry = map.entry(key.clone()).or_default();
                        let before = entry.len();
                        entry.retain(|d| d != dest);
                        if entry.len() != before {
                            changed = true;
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }

        Ok(RenameMap {
            map,
            log_cache: HashMap::new(),
            repo_root: repo_root.to_path_buf(),
        })
    }

    /// Look up the rename successor(s) for `old_path` (repo-relative).
    ///
    /// On miss, performs an on-demand `git log --diff-filter=R --follow` lookup
    /// and caches the result.
    pub fn successor(&mut self, old_path: &Path) -> SuccessorResult {
        let key = old_path.to_string_lossy().into_owned();

        // Check in-memory map first.
        if let Some(dests) = self.map.get(&key) {
            let dests = dests.clone();
            return match dests.len() {
                0 => SuccessorResult::None,
                1 => SuccessorResult::Unique(PathBuf::from(&dests[0])),
                _ => SuccessorResult::Ambiguous(dests.into_iter().map(PathBuf::from).collect()),
            };
        }

        // On-demand git log lookup for HEAD history renames.
        if let Some(cached) = self.log_cache.get(&key) {
            let cached = cached.clone();
            return match cached.len() {
                0 => SuccessorResult::None,
                1 => SuccessorResult::Unique(PathBuf::from(&cached[0])),
                _ => SuccessorResult::Ambiguous(cached.into_iter().map(PathBuf::from).collect()),
            };
        }

        let results = git_log_follow_renames(&self.repo_root, old_path).unwrap_or_default();
        self.log_cache.insert(key.clone(), results.clone());

        // Also populate main map for chain resolution.
        if !results.is_empty() {
            self.map.insert(key, results.clone());
        }

        match results.len() {
            0 => SuccessorResult::None,
            1 => SuccessorResult::Unique(PathBuf::from(&results[0])),
            _ => SuccessorResult::Ambiguous(results.into_iter().map(PathBuf::from).collect()),
        }
    }
}

/// Parse `git diff [--cached] --diff-filter=R --name-status` output into (old, new) pairs.
fn run_diff_renames(repo_root: &Path, cached: bool) -> Result<Vec<(String, String)>> {
    let mut cmd = Command::new("git");
    cmd.current_dir(repo_root).arg("diff");
    if cached {
        cmd.arg("--cached");
    }
    cmd.args(["--diff-filter=R", "--name-status"]);

    let output = cmd
        .output()
        .map_err(|e| miette::miette!("git diff failed: {e}"))?;

    if !output.status.success() {
        // If there's no HEAD (empty repo), cached diff fails — treat as empty.
        return Ok(vec![]);
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut pairs = Vec::new();
    for line in text.lines() {
        // Format: R<score>\t<old>\t<new>  or R\t<old>\t<new>
        if !line.starts_with('R') {
            continue;
        }
        let parts: Vec<&str> = line.splitn(3, '\t').collect();
        if parts.len() == 3 {
            pairs.push((parts[1].to_string(), parts[2].to_string()));
        }
    }
    Ok(pairs)
}

/// Run `git log --diff-filter=R --follow --name-status -- <path>` and return the most
/// recent rename destination (if any) for `path`.
fn git_log_follow_renames(repo_root: &Path, old_path: &Path) -> Result<Vec<String>> {
    let path_str = old_path.to_string_lossy();
    let output = Command::new("git")
        .current_dir(repo_root)
        .args([
            "log",
            "--diff-filter=R",
            "--follow",
            "--name-status",
            "--format=",
            "--",
            &path_str,
        ])
        .output()
        .map_err(|e| miette::miette!("git log failed: {e}"))?;

    if !output.status.success() {
        return Ok(vec![]);
    }

    let text = String::from_utf8_lossy(&output.stdout);
    // Lines look like: R<score>\t<old>\t<new>
    // The first occurrence (most recent) is the rename we care about.
    // We want the *destination* of the most recent rename for this path.
    for line in text.lines() {
        if !line.starts_with('R') {
            continue;
        }
        let parts: Vec<&str> = line.splitn(3, '\t').collect();
        if parts.len() == 3 {
            return Ok(vec![parts[2].to_string()]);
        }
    }
    Ok(vec![])
}

// ── Path helpers ──────────────────────────────────────────────────────────────

/// Given a link's href (which may be repo-relative `/foo/bar.md`, relative
/// `./bar.md`, or bare `bar.md`) and the rename pair (old_rel, new_rel as
/// repo-relative paths), compute the replacement href that preserves the same
/// addressing style.
///
/// - Repo-relative (`/…`): return `/new_rel`.
/// - Relative (`./`, `../`, or bare): compute path from the wiki file's directory
///   to `new_rel` and prefix with `./` if needed.
fn rewrite_href(
    original_href: &str,
    fragment: Option<&str>,
    new_rel: &Path,
    wiki_file: &Path,
    repo_root: &Path,
) -> String {
    // Strip any fragment from the href for path comparison.
    let path_part = match original_href.find('#') {
        Some(idx) => &original_href[..idx],
        None => original_href,
    };

    let new_path_str = if path_part.starts_with('/') {
        // Repo-relative addressing: keep as `/new_rel`.
        format!("/{}", new_rel.display())
    } else {
        // Relative addressing: compute relative path from wiki file's directory.
        let wiki_dir = wiki_file.parent().unwrap_or(Path::new("."));
        let abs_new = repo_root.join(new_rel);
        let rel = diff_paths(&abs_new, wiki_dir);
        let rel_str = rel.to_string_lossy();
        // Ensure we use `./` prefix for same-dir or descending paths.
        if rel_str.starts_with("..") {
            rel_str.into_owned()
        } else {
            format!("./{rel_str}")
        }
    };

    match fragment {
        Some(frag) => format!("{new_path_str}#{frag}"),
        None => new_path_str,
    }
}

/// Compute the relative path from `base` directory to `target` file.
/// Returns a relative `PathBuf` (never absolute).
fn diff_paths(target: &Path, base: &Path) -> PathBuf {
    // Normalize both to remove `.` components.
    let target = normalize_path(target);
    let base = normalize_path(base);

    let mut target_comps: Vec<_> = target.components().collect();
    let mut base_comps: Vec<_> = base.components().collect();

    // Strip common prefix.
    let common = target_comps
        .iter()
        .zip(base_comps.iter())
        .take_while(|(a, b)| a == b)
        .count();

    target_comps.drain(..common);
    base_comps.drain(..common);

    let mut result = PathBuf::new();
    for _ in &base_comps {
        result.push("..");
    }
    for comp in &target_comps {
        result.push(comp);
    }

    if result.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        result
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            c => result.push(c),
        }
    }
    result
}

// ── Fix #5: heading position computation ─────────────────────────────────────

/// Structural position of a heading within a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadingPosition {
    /// ATX depth (number of `#` characters): 1–6.
    pub depth: usize,
    /// Slug of the nearest ancestor heading with strictly smaller depth.
    /// Empty string when there is no ancestor (top-level heading).
    pub parent_slug: String,
    /// 0-based index among same-depth headings sharing the same parent, in document order.
    pub sibling_index: usize,
}

/// Compute `(Heading, HeadingPosition)` for each heading in `content`.
///
/// Uses `extract_headings` for slugs and depth is measured by counting leading `#` on each line.
pub fn heading_positions(content: &str) -> Vec<(crate::headings::Heading, HeadingPosition)> {
    // First, extract headings with their slugs via the canonical algorithm.
    let headings = extract_headings(content);
    if headings.is_empty() {
        return vec![];
    }

    // Compute depth for each heading by re-scanning the content.
    let mut depth_by_line: std::collections::HashMap<usize, usize> =
        std::collections::HashMap::new();
    for line in content.lines().enumerate().map(|(i, l)| (i + 1, l)) {
        let (line_num, text) = line;
        if text.starts_with('#') {
            let depth = text.chars().take_while(|&c| c == '#').count();
            let rest = &text[depth..];
            if rest.starts_with(' ') {
                depth_by_line.insert(line_num, depth);
            }
        }
    }

    // Track the most recent heading slug at each depth for parent computation.
    // depth_stack[d] = slug of the most recent heading at depth d (1-indexed).
    let mut depth_stack: Vec<Option<String>> = vec![None; 7]; // index 1..=6

    // sibling_count[(depth, parent_slug)] = count so far
    let mut sibling_counts: std::collections::HashMap<(usize, String), usize> =
        std::collections::HashMap::new();

    let mut result = Vec::with_capacity(headings.len());
    for h in &headings {
        let depth = *depth_by_line.get(&h.line).unwrap_or(&1);

        // Parent: most recent heading with strictly smaller depth.
        let parent_slug = (1..depth)
            .rev()
            .find_map(|d| depth_stack[d].clone())
            .unwrap_or_default();

        let key = (depth, parent_slug.clone());
        let sibling_index = *sibling_counts.get(&key).unwrap_or(&0);
        sibling_counts.insert(key, sibling_index + 1);

        // Update depth stack: clear all deeper levels when we see this depth.
        if depth <= 6 {
            depth_stack[depth] = Some(h.slug.clone());
            // Clear all strictly deeper depths (they are no longer valid parents).
            for slot in depth_stack.iter_mut().take(7).skip(depth + 1) {
                *slot = None;
            }
        }

        result.push((
            h.clone(),
            HeadingPosition {
                depth,
                parent_slug,
                sibling_index,
            },
        ));
    }

    result
}

/// Find the heading at position `pos` in `content`. Returns the heading's slug if
/// exactly one heading occupies that position, `None` if zero, and signals multiple
/// via the returned `Vec` length.
fn headings_at_position(content: &str, pos: &HeadingPosition) -> Vec<String> {
    heading_positions(content)
        .into_iter()
        .filter(|(_, p)| p == pos)
        .map(|(h, _)| h.slug)
        .collect()
}

/// Read the HEAD blob for `rel_path` (repo-relative). Returns `Ok(None)` when not
/// found or on any git error.
fn read_head_blob(repo_root: &Path, rel_path: &str) -> Result<Option<String>> {
    let output = Command::new("git")
        .current_dir(repo_root)
        .args(["show", &format!("HEAD:{rel_path}")])
        .output()
        .map_err(|e| miette::miette!("git show failed: {e}"))?;
    if output.status.success()
        && let Ok(s) = String::from_utf8(output.stdout)
    {
        return Ok(Some(s));
    }
    Ok(None)
}

/// Maximum number of historical revisions to inspect when walking `git log
/// --follow` looking for the layer where a now-broken heading slug last
/// resolved. The cap exists to avoid pathological cases on long-lived files
/// where the slug never appears; 100 commits is large enough to cover any
/// realistic refactor window while still bounding the walk.
const HEADING_HISTORY_DEPTH_CAP: usize = 100;

/// Walk a target file's content across layers — HEAD first, then HEAD history
/// via `git log --follow` — and return the first content where `anchor_slug`
/// resolves as a heading. Bounded by [`HEADING_HISTORY_DEPTH_CAP`].
///
/// The worktree and index layers are intentionally not consulted here: callers
/// already inspect the *current* (patched) content separately. A "baseline" for
/// Fix #5 is by definition older than the broken state, so we only walk
/// committed history.
fn find_baseline_with_slug(
    repo_root: &Path,
    rel_path: &str,
    anchor_slug: &str,
) -> Result<Option<String>> {
    // Layer: HEAD
    if let Some(content) = read_head_blob(repo_root, rel_path)? {
        let headings = extract_headings(&content);
        if resolve_heading(anchor_slug, &headings) {
            return Ok(Some(content));
        }
    }

    // Layer: HEAD history via `git log --follow`. Newest-first; stop at the
    // first revision whose blob contains the slug.
    let output = Command::new("git")
        .current_dir(repo_root)
        .args([
            "log",
            "--follow",
            "--format=%H",
            "--name-status",
            "--",
            rel_path,
        ])
        .output()
        .map_err(|e| miette::miette!("git log failed: {e}"))?;

    if !output.status.success() {
        return Ok(None);
    }

    let text = String::from_utf8_lossy(&output.stdout);

    // `--name-status --format=%H` interleaves SHAs and rename/path lines.
    // We need (sha, path-at-that-revision) pairs. The current path follows the
    // sha; if a rename is encountered the path changes for older revisions.
    let mut current_path = rel_path.to_string();
    let mut seen = 0usize;
    let mut last_sha: Option<String> = None;

    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        // SHAs are 40 hex chars with no tab.
        if !line.contains('\t') && line.len() >= 7 && line.chars().all(|c| c.is_ascii_hexdigit()) {
            // Process the previous (sha, path) pair before advancing.
            if let Some(sha) = last_sha.take() {
                seen += 1;
                if seen > HEADING_HISTORY_DEPTH_CAP {
                    return Ok(None);
                }
                if let Some(content) = read_blob_at(repo_root, &sha, &current_path)?
                    && resolve_heading(anchor_slug, &extract_headings(&content))
                {
                    return Ok(Some(content));
                }
            }
            last_sha = Some(line.to_string());
            continue;
        }

        // Name-status line. Formats:
        //   M\tpath
        //   A\tpath
        //   D\tpath
        //   R<score>\told\tnew
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.is_empty() {
            continue;
        }
        let status = parts[0];
        if status.starts_with('R') && parts.len() == 3 {
            // For an R record on commit X with old=O new=N, parents of X had
            // the file at O. We want the path at the *commit being examined*
            // (the sha we just read). After processing this commit, the path
            // for older revisions becomes `old`.
            // Examine current commit at `new`:
            if let Some(sha) = last_sha.take() {
                seen += 1;
                if seen > HEADING_HISTORY_DEPTH_CAP {
                    return Ok(None);
                }
                if let Some(content) = read_blob_at(repo_root, &sha, parts[2])?
                    && resolve_heading(anchor_slug, &extract_headings(&content))
                {
                    return Ok(Some(content));
                }
            }
            current_path = parts[1].to_string();
        } else if parts.len() >= 2 {
            // Non-rename: path is parts[1], unchanged.
            current_path = parts[1].to_string();
        }
    }

    // Drain the trailing sha (last commit had no rename line follow-up before
    // EOF, which means we haven't read it yet).
    if let Some(sha) = last_sha.take() {
        seen += 1;
        if seen <= HEADING_HISTORY_DEPTH_CAP
            && let Some(content) = read_blob_at(repo_root, &sha, &current_path)?
            && resolve_heading(anchor_slug, &extract_headings(&content))
        {
            return Ok(Some(content));
        }
    }

    Ok(None)
}

/// Read `git show <sha>:<path>` as a UTF-8 string. Returns `Ok(None)` on any
/// git error or non-UTF-8 blob.
pub(crate) fn read_blob_at(repo_root: &Path, sha: &str, path: &str) -> Result<Option<String>> {
    let output = Command::new("git")
        .current_dir(repo_root)
        .args(["show", &format!("{sha}:{path}")])
        .output()
        .map_err(|e| miette::miette!("git show failed: {e}"))?;
    if !output.status.success() {
        return Ok(None);
    }
    match String::from_utf8(output.stdout) {
        Ok(s) => Ok(Some(s)),
        Err(_) => Ok(None),
    }
}

// ── Fix #2: mesh auto-follow ──────────────────────────────────────────────────

/// A stale anchor that `git-mesh-core` relocated to a unique new position.
///
/// Carries both the anchored (old) coordinates and the destination (new)
/// coordinates so callers can rewrite wiki link fragments. Computed in-process
/// from the `.wiki/` mesh store: re-hash the anchored extent, and when it no
/// longer matches the stored hash, find where the verbatim content moved.
#[derive(Debug)]
pub struct MeshMovePlan {
    pub mesh_name: String,
    pub old_path: PathBuf,
    pub old_start: u32,
    pub old_end: u32,
    #[allow(dead_code)]
    pub new_path: PathBuf,
    pub new_start: u32,
    pub new_end: u32,
}

/// A stale anchor whose content was found in two or more candidate locations.
///
/// Fail-closed: the move-follow refuses to guess which location the link should
/// point at, so it rewrites nothing and surfaces the conflict for a human to
/// resolve. Carries enough context to name the anchor and the candidate count
/// in a user-visible report.
#[derive(Debug)]
pub struct MeshConflict {
    /// The mesh slug the conflicting anchor belongs to.
    pub mesh_name: String,
    /// Repo-relative path the anchor names today.
    pub path: PathBuf,
    /// 1-based start line of the anchor (0 for whole-file).
    pub start: u32,
    /// 1-based end line of the anchor (0 for whole-file).
    pub end: u32,
    /// Number of candidate locations the content was found in (≥ 2).
    pub candidate_count: usize,
}

/// The outcome of a move-follow pass: unique relocations and ambiguous conflicts.
#[derive(Debug, Default)]
pub struct MeshFollowOutcome {
    /// Anchors whose content moved to a single unambiguous new location.
    pub plans: Vec<MeshMovePlan>,
    /// Anchors whose content matched ≥ 2 candidate locations (fail-closed).
    pub conflicts: Vec<MeshConflict>,
    /// Stale anchors with zero unambiguous relocation, classified as in-place
    /// `Changed` (whitespace-only or meaning-changing) or `Deleted`.
    pub drifted: Vec<AnchorDrift>,
}

/// Maximum git commits to inspect when recovering old anchor bytes.
const HISTORY_SCAN_LIMIT: usize = 50;

/// True when `bytes` decode to a wiki page: markdown with YAML frontmatter
/// carrying both a non-empty `title` and a non-empty `summary`.
fn is_wiki_page_bytes(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    matches!(
        parse_frontmatter(text, Path::new("page.md")),
        Ok(Some(fm)) if !fm.title.is_empty() && !fm.summary.is_empty()
    )
}

/// True when a wiki-page line-range anchor is the scaffold-managed *self-section*
/// anchor of the mesh that owns it — the case the [`is_wiki_page_bytes`]
/// exemption was always meant to cover, and the ONLY case it may safely cover.
///
/// The coverage scaffold (`mesh::draft`) names a mesh `<page_subdir>/<noun>`,
/// where `page_subdir` is the OWNING page's directory and `<noun>` is the
/// kebab-cased heading of the section the mesh covers (see
/// `draft::build_slug` / `draft::derive_noun`). It anchors that owning page's
/// OWN section as a line-range `.md` anchor and re-anchors it on every `--fix`,
/// so the self-section anchor must not fail closed as meaning-change drift.
///
/// A naive directory-prefix test is NOT enough: a line-range citation INTO a
/// *same-directory sibling* wiki page shares the owning page's directory and was
/// wrongly exempted (F-FM2). The robust owning-page signal is to **reconstruct
/// the slug from the candidate page itself**: take the page's directory as the
/// subdir, kebab the heading enclosing the anchored range as the noun, and
/// rebuild `build_slug(subdir, noun)`. The exemption fires only when that
/// reconstruction reproduces the mesh slug — i.e. the candidate page IS the page
/// the mesh was scaffolded for. A citation into another page (sibling, ancestor,
/// or elsewhere) reconstructs a different slug and is therefore staleness-checked.
///
/// `repo_root` lets us derive the page subdir relative to the repo; `anchor_path`
/// is the page file (repo-relative), `start` the anchor's 1-based start line.
fn is_scaffold_self_section_anchor(
    repo_root: &Path,
    slug: &str,
    anchor_path: &str,
    start: u32,
    page_bytes: &[u8],
) -> bool {
    if !is_wiki_page_bytes(page_bytes) {
        return false;
    }
    let Ok(text) = std::str::from_utf8(page_bytes) else {
        return false;
    };

    // Page subdir: the owning page's directory relative to the wiki/repo root,
    // forward slashes (empty for repo-root pages). `build_slug` collapses
    // repeated segments, so we pass the raw directory and let it normalise.
    let abs_page = repo_root.join(anchor_path);
    let page_subdir = crate::commands::mesh::scaffold::resolve_page_subdir(&abs_page, repo_root);

    // The noun is the kebab of the heading enclosing the anchored range — the
    // nearest heading at or above `start`. Match it the way the scaffold derives
    // the section noun (`draft::derive_noun` kebabs the section heading text).
    let headings = extract_headings(text);
    let enclosing = headings
        .iter()
        .filter(|h| h.line as u32 <= start.max(1))
        .max_by_key(|h| h.line);
    let Some(h) = enclosing else {
        // No heading governs the anchored range — the scaffold could not have
        // derived a heading noun for it, so this is not a heading-section
        // self-anchor. Fail-closed: check it.
        return false;
    };
    let noun = super::mesh::draft::kebab(&h.text);
    if noun.is_empty() {
        return false;
    }
    let reconstructed = super::mesh::draft::build_slug(&page_subdir, &noun);
    reconstructed == slug
}

/// True when the file at `path` belongs to a language/format where leading
/// indentation is COSMETIC (insignificant to meaning) — the brace/C-family and
/// similar. For these, an indentation-only reflow (2→4 spaces, tabs↔spaces) is a
/// routine formatter run that carries no meaning change, so leading whitespace
/// may be stripped before whitespace-equivalence comparison.
///
/// Fail-closed: any extension NOT on this list — including indentation-
/// significant formats (`.py`, `.yaml`, `.md`, Makefiles, Haskell/F#, …) and any
/// unknown/extensionless file — returns `false`. There an indentation-only edit
/// can change meaning (e.g. dedenting a statement out of a Python `if` block), so
/// leading whitespace is preserved and such a change is treated as potentially
/// meaning-changing.
fn leading_whitespace_insignificant(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    // Files named Makefile / makefile / *.mk are indentation-significant (tabs
    // are recipe markers) — explicitly excluded (covered by the default `false`,
    // but the `.mk` check below makes the intent unmissable).
    let stem = lower.rsplit('/').next().unwrap_or(&lower);
    if stem == "makefile" || stem.ends_with(".mk") {
        return false;
    }
    let Some((_, ext)) = lower.rsplit_once('.') else {
        // No extension → unknown → fail-closed.
        return false;
    };
    matches!(
        ext,
        // Brace / C-family and similar where leading whitespace is cosmetic.
        "rs" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "c" | "h" | "cpp"
            | "cc" | "hpp" | "hh" | "java" | "go" | "cs" | "php" | "swift" | "kt"
            | "kts" | "scala" | "css" | "scss" | "less" | "json" | "jsonc"
    )
}

/// True when `a` and `b` contain the same non-whitespace content, with leading-
/// indentation handling gated by `path`'s file type.
///
/// Trailing-whitespace trimming and blank-line dropping are ALWAYS applied —
/// those are universally safe across every language. Leading-indentation
/// stripping is applied ONLY when [`leading_whitespace_insignificant`] holds for
/// `path` (the brace/C-family): there a rustfmt/prettier/gofmt reindent is
/// whitespace-equivalent. For indentation-significant or unknown file types,
/// leading whitespace is preserved, so a dedent/reindent that could change
/// meaning is NOT graded whitespace-only (fail-closed). An identifier rename or
/// added parameter changes a non-whitespace token and is NOT equivalent in any
/// language.
fn whitespace_equivalent(a: &str, b: &str, path: &str) -> bool {
    let strip_leading = leading_whitespace_insignificant(path);
    let normalise = |s: &str| -> String {
        s.lines()
            .map(|l| {
                if strip_leading {
                    l.trim()
                } else {
                    l.trim_end()
                }
            })
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    };
    normalise(a) == normalise(b)
}

/// Walk git log for `path`, returning the file bytes from the first commit
/// where the anchor extent hashes to `stored_fp`, or `None` if not found within
/// `limit` commits.
///
/// Used to recover "old content" when HEAD no longer contains it (committed
/// reformat). The search short-circuits on the first match, so recent changes
/// are found in O(1) git invocations.
fn recover_anchor_bytes_from_history(
    repo_root: &Path,
    path: &str,
    extent: &AnchorExtent,
    stored_fp: u64,
    limit: usize,
) -> Option<Vec<u8>> {
    let output = Command::new("git")
        .current_dir(repo_root)
        .args(["log", "--format=%H", "--", path])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for sha in stdout.lines().take(limit) {
        if let Ok(Some(text)) = read_blob_at(repo_root, sha, path) {
            let bytes = text.into_bytes();
            let idx = LineIndex::build(&bytes);
            if cheap_fingerprint_indexed(&idx, extent) == stored_fp {
                return Some(bytes);
            }
        }
    }
    None
}

/// Read the bytes for an anchor's source file from the layer selected by `source`.
///
/// Returns `None` if the file is absent in that layer (→ Deleted).
fn read_anchor_source(repo_root: &Path, source: DocSource, path: &str) -> Option<Vec<u8>> {
    match source {
        DocSource::WorkingTree => std::fs::read(repo_root.join(path)).ok(),
        DocSource::Head => read_blob_at(repo_root, "HEAD", path)
            .ok()
            .flatten()
            .map(|s| s.into_bytes()),
        DocSource::Index => read_blob_at(repo_root, "", path) // git show :path
            .ok()
            .flatten()
            .map(|s| s.into_bytes()),
    }
}

/// Determine whether a stale (non-moved) anchor represents a whitespace-only or
/// meaning-changing edit, covering BOTH committed and uncommitted changes.
///
/// `source` selects the "current" layer. The HEAD fast-path is taken only for
/// `WorkingTree` (where HEAD can serve as the "old" baseline). For `Head`/`Index`
/// sources `current_bytes` already IS HEAD/index content, so reading HEAD again
/// as the "old" comparison target would be circular — the history walk handles
/// those cases.
fn classify_inplace_drift(
    repo_root: &Path,
    anchor: &AnchorRecord,
    extent: &AnchorExtent,
    current_bytes: &[u8],
    stored_fp: u64,
    source: DocSource,
) -> AnchorDriftKind {
    // Fast path (WorkingTree source only): try HEAD as the "old bytes" baseline.
    if source == DocSource::WorkingTree
        && let Ok(Some(head_text)) = read_blob_at(repo_root, "HEAD", &anchor.path)
    {
        let head_bytes = head_text.into_bytes();
        let head_idx = LineIndex::build(&head_bytes);
        if cheap_fingerprint_indexed(&head_idx, extent) == stored_fp {
            let head_slice = store::slice_at_extent(&head_bytes, extent);
            let cur_slice = store::slice_at_extent(current_bytes, extent);
            return AnchorDriftKind::Changed {
                whitespace_only: whitespace_equivalent(&head_slice, &cur_slice, &anchor.path),
            };
        }
    }

    // History-walk fallback: HEAD already moved on (committed change) or we are
    // using a non-WorkingTree source. Walk git log to find the commit whose blob
    // at this extent hashes to stored_fp, then compare old vs current.
    if let Some(old_bytes) = recover_anchor_bytes_from_history(
        repo_root,
        &anchor.path,
        extent,
        stored_fp,
        HISTORY_SCAN_LIMIT,
    ) {
        let old_slice = store::slice_at_extent(&old_bytes, extent);
        let cur_slice = store::slice_at_extent(current_bytes, extent);
        return AnchorDriftKind::Changed {
            whitespace_only: whitespace_equivalent(&old_slice, &cur_slice, &anchor.path),
        };
    }

    // Old bytes not found within HISTORY_SCAN_LIMIT (shallow clone, or the edit
    // is older than the scan limit): fail-closed, but DISTINCT from a confirmed
    // meaning change. We cannot prove whitespace-equivalence, so we never
    // auto-bless; we also do not claim the diff carries meaning. The author
    // re-anchors manually; `wiki mesh add` clears the failure.
    AnchorDriftKind::Unrecoverable
}

/// Compute mesh move plans and in-place drift in-process from the `.wiki/` store.
///
/// Phase-split: a freshness re-hash always runs (no gate); the move-scan (which
/// spawns `git status`) runs lazily only when at least one anchor is stale. On
/// the all-fresh green path only the freshness phase runs.
///
/// Per stale anchor:
/// 1. **Own-file scan.** Search the SAME file for the content at a different
///    line range (no git diff). Detects same-file moves on clean trees.
/// 2. **Change-set scan.** Search files that differ from HEAD in the working
///    tree (`working_tree_change_set`). Detects cross-file moves.
///
/// On the combined hit count: exactly one ⇒ a [`MeshMovePlan`]; two or more ⇒ a
/// [`MeshConflict`] (fail-closed); zero ⇒ in-place drift, classified by
/// [`classify_inplace_drift`] into [`AnchorDrift`].
///
/// `source` selects which layer ([`DocSource`]) the anchor's source bytes are
/// read from (worktree / HEAD / index).
pub fn plan_mesh_follows(repo_root: &Path, source: DocSource) -> Result<MeshFollowOutcome> {
    // Use tolerant reader to skip any meshes that still carry conflict markers
    // (e.g. after a partial conflict resolution). Anchor auto-follow for those
    // meshes is deferred — the next clean `--fix` pass will handle them.
    let (meshes, skipped) = store::read_all_tolerant(repo_root)?;
    if !skipped.is_empty() {
        eprintln!(
            "wiki: skipped {} unparseable mesh(es) during anchor follow",
            skipped.len()
        );
    }
    if meshes.is_empty() {
        return Ok(MeshFollowOutcome::default());
    }

    let wiki_ignore =
        crate::wikiignore::WikiIgnore::load(repo_root).map_err(|e| miette::miette!("{e}"))?;

    // Read every file an anchor names exactly once (a file with K anchors is
    // read once, not K times), so `LineIndex` and the content hashes are
    // computed over a single owned buffer per path. Read from the selected
    // source layer.
    let mut file_bytes: HashMap<String, Option<Vec<u8>>> = HashMap::new();
    for (_, mesh) in &meshes {
        for anchor in &mesh.anchors {
            if wiki_ignore.is_ignored(Path::new(&anchor.path)) {
                continue;
            }
            file_bytes
                .entry(anchor.path.clone())
                .or_insert_with(|| read_anchor_source(repo_root, source, &anchor.path));
        }
    }

    // Phase 1: freshness re-hash (no gate). A stale anchor records (slug, anchor,
    // extent, stored_fp). An anchor whose source file is unreadable in this
    // layer is stale too (will classify to Deleted in phase 2).
    struct Stale<'a> {
        slug: &'a str,
        anchor: &'a AnchorRecord,
        extent: AnchorExtent,
        stored_fp: u64,
    }
    let mut stale_set: Vec<Stale> = Vec::new();

    for (slug, mesh) in &meshes {
        for anchor in &mesh.anchors {
            if wiki_ignore.is_ignored(Path::new(&anchor.path)) {
                continue;
            }
            let extent = if anchor.start_line == 0 && anchor.end_line == 0 {
                AnchorExtent::WholeFile
            } else {
                AnchorExtent::LineRange {
                    start: anchor.start_line,
                    end: anchor.end_line,
                }
            };

            // The stored content identity is an rk64 fingerprint (16 hex). A
            // record that does not parse as rk64 cannot be matched against rk64
            // windows — skip it rather than guess.
            let Some(stored_fp) = rk64_from_hex(&anchor.content_hash) else {
                continue;
            };

            let Some(Some(bytes)) = file_bytes.get(&anchor.path) else {
                // Unreadable / absent in this layer ⇒ stale.
                stale_set.push(Stale { slug, anchor, extent, stored_fp });
                continue;
            };
            let idx = LineIndex::build(bytes);
            if cheap_fingerprint_indexed(&idx, &extent) != stored_fp {
                stale_set.push(Stale { slug, anchor, extent, stored_fp });
            }
            // Fresh ⇒ nothing.
        }
    }

    let mut plans: Vec<MeshMovePlan> = Vec::new();
    let mut conflicts: Vec<MeshConflict> = Vec::new();
    let mut drifted: Vec<AnchorDrift> = Vec::new();

    if stale_set.is_empty() {
        return Ok(MeshFollowOutcome { plans, conflicts, drifted });
    }

    // Phase 2: move-scan (lazy — only when stale anchors exist).
    //
    // Working-tree change set: repo-relative paths that differ from HEAD
    // (staged, unstaged, or untracked). Always worktree content regardless of
    // `source` — `git status` reports the working tree.
    let change_set = working_tree_change_set(repo_root)?;
    let mut change_set_bytes: HashMap<String, Vec<u8>> = HashMap::new();
    for path in &change_set {
        change_set_bytes
            .entry(path.clone())
            .or_insert_with(|| std::fs::read(repo_root.join(path)).unwrap_or_default());
    }
    let change_set_idx: Vec<(String, LineIndex)> = change_set
        .iter()
        .filter_map(|p| {
            let b = change_set_bytes.get(p)?;
            (!b.is_empty()).then(|| (p.clone(), LineIndex::build(b)))
        })
        .collect();

    for s in &stale_set {
        let Stale { slug, anchor, extent, stored_fp } = *s;

        // (a) Own-file scan: look for the anchor content at a different range in
        //     the same file. No git diff required; preserves same-file move
        //     detection on clean trees (committed moves included).
        let own_bytes = file_bytes
            .get(&anchor.path)
            .and_then(|o| o.as_deref())
            .unwrap_or(&[]);
        let own_hits = if own_bytes.is_empty() {
            Vec::new()
        } else {
            let own_pair = [(anchor.path.clone(), LineIndex::build(own_bytes))];
            let near = match extent {
                AnchorExtent::WholeFile => None,
                AnchorExtent::LineRange { start, .. } => Some(start),
            };
            scan_indexed_rk64(&own_pair, stored_fp, extent, near)
        };

        // (b) Change-set scan: look for the anchor content in OTHER files
        //     changed vs HEAD in the working tree. The anchor's own file is
        //     excluded here so it is never double-counted against the own-file
        //     scan above (which already covers it) — counting both would turn a
        //     single in-file shift into a spurious 2-candidate conflict.
        let cross_idx: Vec<(String, LineIndex)> = change_set_idx
            .iter()
            .filter(|(p, _)| p != &anchor.path)
            .map(|(p, idx)| (p.clone(), idx.clone()))
            .collect();
        let cross_hits = scan_indexed_rk64(&cross_idx, stored_fp, extent, None);

        let total = own_hits.len() + cross_hits.len();
        match total {
            1 => {
                let hit = own_hits.first().or_else(|| cross_hits.first()).unwrap();
                push_plan(&mut plans, slug, anchor, hit);
            }
            n if n >= 2 => {
                push_conflict(&mut conflicts, slug, anchor, n);
            }
            _ => {
                // 0 hits: stale, no unambiguous relocation — classify in place.
                //
                // Skip classification for structural page-anchors only: a
                // WholeFile extent targeting a `.md` file. Line-range `.md`
                // citations ARE checked. (Page-anchor move-follow still ran
                // above — only this classification step is skipped.)
                if matches!(extent, AnchorExtent::WholeFile)
                    && (anchor.path.ends_with(".md") || anchor.path.ends_with(".MD"))
                {
                    continue;
                }

                // Skip classification ONLY for the scaffold-managed self-section
                // anchor: a line-range `.md` anchor whose page is the wiki page
                // that owns this mesh (the page citing its own section). The
                // coverage pass (`create_mesh_coverage`) re-anchors those on
                // every `--fix`, so they are bookkeeping, not external citations,
                // and must not fail closed as meaning-change drift.
                //
                // A line-range citation INTO a *different* wiki page is a
                // human-authored cross-page reference and IS checked — narrowing
                // this from the old "any wiki-page source" guard, which silently
                // swallowed cross-page `.md`→`.md` citation drift.
                if (anchor.path.ends_with(".md") || anchor.path.ends_with(".MD"))
                    && file_bytes
                        .get(&anchor.path)
                        .and_then(|o| o.as_deref())
                        .map(|b| {
                            is_scaffold_self_section_anchor(
                                repo_root,
                                slug,
                                &anchor.path,
                                anchor.start_line,
                                b,
                            )
                        })
                        .unwrap_or(false)
                {
                    continue;
                }

                // File missing in this layer → Deleted.
                let Some(Some(current_bytes)) = file_bytes.get(&anchor.path) else {
                    drifted.push(AnchorDrift {
                        mesh_name: slug.to_string(),
                        path: anchor.path.clone(),
                        start: anchor.start_line,
                        end: anchor.end_line,
                        kind: AnchorDriftKind::Deleted,
                    });
                    continue;
                };

                // Out-of-bounds → Deleted. Use LineIndex line count (consistent
                // with cheap_fingerprint_indexed) to avoid off-by-one on files
                // without a trailing newline.
                let idx = LineIndex::build(current_bytes);
                if let AnchorExtent::LineRange { end, .. } = extent
                    && end as usize > idx.line_count()
                {
                    drifted.push(AnchorDrift {
                        mesh_name: slug.to_string(),
                        path: anchor.path.clone(),
                        start: anchor.start_line,
                        end: anchor.end_line,
                        kind: AnchorDriftKind::Deleted,
                    });
                    continue;
                }

                // Content exists but changed — classify with whitespace detection.
                let kind = classify_inplace_drift(
                    repo_root,
                    anchor,
                    &extent,
                    current_bytes,
                    stored_fp,
                    source,
                );
                drifted.push(AnchorDrift {
                    mesh_name: slug.to_string(),
                    path: anchor.path.clone(),
                    start: anchor.start_line,
                    end: anchor.end_line,
                    kind,
                });
            }
        }
    }

    Ok(MeshFollowOutcome { plans, conflicts, drifted })
}

/// Append a [`MeshMovePlan`] for `anchor` relocated to `hit`.
fn push_plan(
    plans: &mut Vec<MeshMovePlan>,
    slug: &str,
    anchor: &git_mesh_core::mesh_file::AnchorRecord,
    hit: &git_mesh_core::Location,
) {
    plans.push(MeshMovePlan {
        mesh_name: slug.to_string(),
        old_path: PathBuf::from(&anchor.path),
        old_start: anchor.start_line,
        old_end: anchor.end_line,
        new_path: PathBuf::from(&hit.path),
        new_start: hit.start_line,
        new_end: hit.end_line,
    });
}

/// Append a [`MeshConflict`] for `anchor` matched in `candidate_count` places.
fn push_conflict(
    conflicts: &mut Vec<MeshConflict>,
    slug: &str,
    anchor: &git_mesh_core::mesh_file::AnchorRecord,
    candidate_count: usize,
) {
    conflicts.push(MeshConflict {
        mesh_name: slug.to_string(),
        path: PathBuf::from(&anchor.path),
        start: anchor.start_line,
        end: anchor.end_line,
        candidate_count,
    });
}

/// Repo-relative paths that differ from HEAD in the working tree: staged,
/// unstaged, and untracked. Parsed from `git status --porcelain -z`.
fn working_tree_change_set(repo_root: &Path) -> Result<Vec<String>> {
    let output = Command::new("git")
        .current_dir(repo_root)
        .args(["status", "--porcelain", "-z", "--no-renames"])
        .output()
        .map_err(|e| miette::miette!("git status failed: {e}"))?;

    if !output.status.success() {
        return Ok(vec![]);
    }

    // `-z` records are NUL-separated; each is `XY <path>` (no rename arrows
    // because `--no-renames` is set, so there is never a second path field).
    let text = String::from_utf8_lossy(&output.stdout);
    let mut paths = Vec::new();
    for record in text.split('\0') {
        if record.len() < 4 {
            continue;
        }
        // Status is two chars + a space, then the path.
        let path = &record[3..];
        if !path.is_empty() {
            paths.push(path.to_string());
        }
    }
    Ok(paths)
}

// ── Fix #1 implementation ─────────────────────────────────────────────────────

/// Scan `files` for `broken_link` diagnostics and attempt to rewrite paths whose
/// targets were renamed in git. Returns a `FixPlan` describing what was (or would
/// be) applied.
///
/// When `dry_run` is false, patched content is written back to disk.
///
/// `scan_root` and `globs` define the scope of the run (as supplied to
/// `check::run`). They are forwarded to `cleanup_orphaned_meshes` so that
/// cleanup only touches orphans whose sole `.md` anchor falls within the
/// same scope that the discovery pass used.
#[allow(clippy::too_many_arguments)]
pub fn run_fix_pass(
    files: &[PathBuf],
    repo_root: &Path,
    scan_root: &Path,
    globs: &[String],
    source: DocSource,
    dry_run: bool,
    content_cache: &mut ContentCache,
    git_reader: Option<&GitReader>,
) -> Result<FixPlan> {
    let mut rename_map = RenameMap::build(repo_root)?;

    let mut fixes: Vec<Fix> = Vec::new();
    let mut skipped: Vec<SkippedFix> = Vec::new();
    // file abs path → patched content
    let mut patches: HashMap<PathBuf, String> = HashMap::new();

    for file in files {
        let content = match content_cache
            .get_or_try_read(file, || std::fs::read_to_string(file))
        {
            Ok(c) => c.to_string(),
            Err(_) => continue,
        };

        let frag_links = parse_fragment_links(&content);
        let mut file_patches: Vec<(usize, usize, String)> = Vec::new(); // (start, end, replacement)

        let file_rel = file
            .strip_prefix(repo_root)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| file.to_string_lossy().into_owned());

        for link in &frag_links {
            if link.kind == LinkKind::External {
                continue;
            }
            if link.original_href.starts_with("mailto:") {
                continue;
            }

            // Only handle links whose target file is missing.
            let resolved = crate::commands::resolve_link_path(&link.path, file, repo_root);
            let abs = repo_root.join(&resolved);
            if abs.exists() || abs.is_dir() {
                continue;
            }

            // Skip bare-path links — they are already flagged with a repo-relative hint.
            let first = Path::new(&link.path).components().next();
            let is_explicit = matches!(first, Some(Component::CurDir) | Some(Component::ParentDir));
            let is_bare = !link.path.starts_with('/') && !is_explicit;
            if is_bare {
                skipped.push(SkippedFix {
                    file: file_rel.clone(),
                    line: link.source_line,
                    kind: FixKind::BrokenLinkRename,
                    reason: "bare path; manual review".to_string(),
                });
                continue;
            }

            // resolved is repo-relative; look it up in the rename map.
            match rename_map.successor(&resolved) {
                SuccessorResult::Unique(new_rel) => {
                    // Only apply if the new file actually exists.
                    let new_abs = repo_root.join(&new_rel);
                    if !new_abs.exists() {
                        skipped.push(SkippedFix {
                            file: file_rel.clone(),
                            line: link.source_line,
                            kind: FixKind::BrokenLinkRename,
                            reason: format!(
                                "target deleted; no successor (rename destination {} missing)",
                                new_rel.display()
                            ),
                        });
                        continue;
                    }

                    let fragment = link
                        .original_href
                        .find('#')
                        .map(|i| &link.original_href[i + 1..]);
                    let new_href =
                        rewrite_href(&link.original_href, fragment, &new_rel, file, repo_root);

                    fixes.push(Fix {
                        file: file_rel.clone(),
                        line: link.source_line,
                        kind: FixKind::BrokenLinkRename,
                        byte_start: link.href_byte_start,
                        byte_end: link.href_byte_end,
                        old_href: link.original_href.clone(),
                        new_href: new_href.clone(),
                        reason: format!("renamed to {}", new_rel.display()),
                        confidence: Confidence::High,
                    });
                    file_patches.push((link.href_byte_start, link.href_byte_end, new_href));
                }
                SuccessorResult::Ambiguous(candidates) => {
                    let names: Vec<String> =
                        candidates.iter().map(|p| p.display().to_string()).collect();
                    skipped.push(SkippedFix {
                        file: file_rel.clone(),
                        line: link.source_line,
                        kind: FixKind::BrokenLinkRename,
                        reason: format!("ambiguous rename: {}", names.join(", ")),
                    });
                }
                SuccessorResult::None => {
                    skipped.push(SkippedFix {
                        file: file_rel.clone(),
                        line: link.source_line,
                        kind: FixKind::BrokenLinkRename,
                        reason: "target deleted; no successor".to_string(),
                    });
                }
            }
        }

        if !file_patches.is_empty() {
            // Apply patches in reverse byte order to preserve offsets.
            file_patches.sort_by_key(|p| Reverse(p.0));
            let mut patched = content.clone();
            for (start, end, replacement) in file_patches {
                patched.replace_range(start..end, &replacement);
            }
            patches.insert(file.clone(), patched);
        }
    }

    // ── Fix #3: alias-driven anchor rewrite ───────────────────────────────────
    //
    // For each in-scope file, parse fragment links. For each non-line-range
    // fragment link whose anchor does NOT resolve against the target's headings,
    // check whether the anchor matches an alias in the target's frontmatter.
    // If so, rewrite the anchor to the target's *canonical* heading slug.
    //
    // Canonical-slug resolution rule (highest confidence first):
    //   1. Slug of the page's frontmatter `title`, if a heading with that slug
    //      exists. Matches the wiki convention "the H1 names the page".
    //   2. Slug of the page's first H1 heading, if any.
    //   3. Slug of the page's first heading at any level.
    //   4. Skip with a reason that names every candidate we tried.
    //
    // The fallback chain exists because not every wiki page repeats its title
    // as the top-level heading: some pages open with a `##` section, some use
    // a leading prose paragraph and only sub-headings, and some title fields
    // intentionally differ from the visible H1 (e.g. "Authentication" titles
    // a page whose H1 is `# Auth & Authorization`). Falling back to a real
    // heading still satisfies the card's intent — give the alias a current
    // canonical destination so the alias entry can be retired without
    // breaking inbound links.

    // Cache: target path → parsed headings, avoids re-parsing the same target
    // file when multiple broken anchors in one source file reference it.
    let mut headings_cache_f3: HashMap<PathBuf, Vec<Heading>> = HashMap::new();
    for file in files {
        // Use the in-memory patched content if Fix #1 rewrote this file.
        let content = if let Some(patched) = patches.get(file) {
            patched.clone()
        } else {
            match content_cache
                .get_or_try_read(file, || std::fs::read_to_string(file))
            {
                Ok(c) => c.to_string(),
                Err(_) => continue,
            }
        };

        let frag_links = parse_fragment_links(&content);
        let mut file_patches: Vec<(usize, usize, String)> = Vec::new();

        let file_rel = file
            .strip_prefix(repo_root)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| file.to_string_lossy().into_owned());

        for link in &frag_links {
            if link.kind == LinkKind::External {
                continue;
            }
            if link.original_href.starts_with("mailto:") {
                continue;
            }

            // Only handle links that have a fragment but no line range.
            let has_fragment = link.original_href.contains('#');
            if !has_fragment || link.start_line.is_some() {
                continue;
            }

            let anchor = match link.original_href.find('#') {
                Some(idx) => &link.original_href[idx + 1..],
                None => continue,
            };
            if anchor.is_empty() {
                continue;
            }

            // Resolve target file — use Fix #1 patched path if the path part changed.
            let resolved = crate::commands::resolve_link_path(&link.path, file, repo_root);
            let target_abs = repo_root.join(&resolved);

            // Read target content: in-memory patch takes priority.
            let target_content = if let Some(patched) = patches.get(&target_abs) {
                patched.clone()
            } else {
                match content_cache
                    .get_or_try_read(&target_abs, || std::fs::read_to_string(&target_abs))
                {
                    Ok(c) => c.to_string(),
                    Err(_) => continue,
                }
            };

            // Use cached headings when available to avoid re-parsing the same
            // target file across multiple links.
            let headings: &[Heading] = match headings_cache_f3.entry(target_abs.clone()) {
                Entry::Occupied(o) => &*o.into_mut(),
                Entry::Vacant(v) => &*v.insert(extract_headings(&target_content)),
            };

            // If anchor resolves correctly, nothing to do.
            if resolve_heading(anchor, headings) {
                continue;
            }

            // Broken anchor — check for alias match.
            let fm = match parse_frontmatter(&target_content, &target_abs) {
                Ok(Some(fm)) => fm,
                // No frontmatter or parse error → fall through (Fix #5 territory).
                _ => continue,
            };

            let anchor_slug = github_slug(anchor);

            // Check if anchor_slug matches any alias.
            let alias_hit = fm.aliases.iter().any(|a| github_slug(a) == anchor_slug);
            if !alias_hit {
                continue;
            }

            // Resolve the canonical heading slug via the documented fallback
            // chain: title-slug → first H1 → first heading.
            let title_slug = github_slug(&fm.title);
            let title_match = resolve_heading(&title_slug, headings);

            let (canonical, source_label) = if title_match {
                (title_slug.clone(), "title")
            } else if let Some(h1) = headings.iter().find(|h| h.level == 1) {
                (h1.slug.clone(), "first H1")
            } else if let Some(first) = headings.first() {
                (first.slug.clone(), "first heading")
            } else {
                skipped.push(SkippedFix {
                    file: file_rel.clone(),
                    line: link.source_line,
                    kind: FixKind::AliasToCanonical,
                    reason: format!(
                        "alias `{}` listed but target has no headings (tried title slug `{}`, first H1, first heading)",
                        anchor, title_slug
                    ),
                });
                continue;
            };

            // Build the new href: replace just the fragment part after `#`.
            let path_part = match link.original_href.find('#') {
                Some(idx) => &link.original_href[..idx],
                None => continue,
            };
            let new_href = format!("{}#{}", path_part, canonical);

            fixes.push(Fix {
                file: file_rel.clone(),
                line: link.source_line,
                kind: FixKind::AliasToCanonical,
                byte_start: link.href_byte_start,
                byte_end: link.href_byte_end,
                old_href: link.original_href.clone(),
                new_href: new_href.clone(),
                reason: format!(
                    "anchor `{}` is an alias for title `{}`; rewriting to canonical slug `{}` ({})",
                    anchor, fm.title, canonical, source_label
                ),
                confidence: Confidence::High,
            });
            file_patches.push((link.href_byte_start, link.href_byte_end, new_href));
        }

        if !file_patches.is_empty() {
            // Apply patches in reverse byte order to preserve offsets.
            file_patches.sort_by_key(|p| Reverse(p.0));
            let base = if let Some(existing) = patches.get(file) {
                existing.clone()
            } else {
                content.clone()
            };
            let mut patched = base;
            for (start, end, replacement) in file_patches {
                patched.replace_range(start..end, &replacement);
            }
            patches.insert(file.clone(), patched);
        }
    }

    // ── Fix #2: mesh auto-follow (line-range anchors) ─────────────────────────
    //
    // Compute move plans in-process from the `.wiki/` store:
    // each stale anchor is relocated to its unique new position, and ambiguity
    // is reported rather than rewritten (fail-closed). Then rewrite wiki page
    // fragment hrefs directly from the planned destination coords.
    //
    // Always run (even in dry_run) to emit SkippedFix records. Working-tree
    // content drives the re-hash, so staged and committed shifts are handled
    // identically.
    let MeshFollowOutcome {
        plans: move_plans,
        conflicts: mesh_conflicts,
        drifted,
    } = plan_mesh_follows(repo_root, source)?;

    // Map (old_path, old_start, old_end) → (slug, new_path, new_start, new_end).
    // The destination path is carried so a cross-file move rewrites the link's
    // path too, not only the line range. The slug is carried so the move-follow
    // can relocate the stored anchor in place once the link is rewritten.
    // Keyed by the old (path, start, end); value is (slug, new_path, new_start,
    // new_end).
    type EligibleKey = (PathBuf, u32, u32);
    type EligibleVal = (String, PathBuf, u32, u32);
    let eligible: HashMap<EligibleKey, EligibleVal> = move_plans
        .iter()
        .map(|p| {
            (
                (p.old_path.clone(), p.old_start, p.old_end),
                (
                    p.mesh_name.clone(),
                    p.new_path.clone(),
                    p.new_start,
                    p.new_end,
                ),
            )
        })
        .collect();

    // Surface ambiguous (≥ 2-match) moves as visible skipped fixes. Fail-closed:
    // the link is left untouched for a human to resolve. These also drive a
    // non-zero exit (see `FixPlan.mesh_conflicts` and `check::run`).
    for c in &mesh_conflicts {
        let anchor_label = if c.start == 0 && c.end == 0 {
            format!("{} (whole file)", c.path.display())
        } else {
            format!("{}#L{}-L{}", c.path.display(), c.start, c.end)
        };
        // Build the anchor arg in the on-disk form expected by `wiki mesh`.
        let anchor_arg = if c.start == 0 && c.end == 0 {
            c.path.display().to_string()
        } else {
            format!("{}#L{}-L{}", c.path.display(), c.start, c.end)
        };
        skipped.push(SkippedFix {
            file: format!(".wiki/{}", c.mesh_name),
            line: 0,
            kind: FixKind::MeshAnchorShift,
            reason: format!(
                "anchor {anchor_label} in mesh {} found in {} candidate locations — \
                 add the anchor at the correct location, then remove the stale one \
                 (add-then-remove keeps the mesh alive so its rationale survives):\n  \
                 wiki mesh add {} <new-anchor>\n  \
                 wiki mesh remove {} {anchor_arg}",
                c.mesh_name, c.candidate_count, c.mesh_name, c.mesh_name
            ),
        });
    }

    // Build the MeshIndex using all in-scope files.
    let mesh_index_opt = build_mesh_index(repo_root, files)?;

    // For each wiki file, rewrite broken line-range links that are now covered
    // by the updated mesh.
    for file in files {
        let content = if let Some(patched) = patches.get(file) {
            patched.clone()
        } else {
            match content_cache
                .get_or_try_read(file, || std::fs::read_to_string(file))
            {
                Ok(c) => c.to_string(),
                Err(_) => continue,
            }
        };

        let frag_links = parse_fragment_links(&content);
        let mut file_patches: Vec<(usize, usize, String)> = Vec::new();

        let file_rel = file
            .strip_prefix(repo_root)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| file.to_string_lossy().into_owned());

        let wiki_rel = file
            .strip_prefix(repo_root)
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|_| file.clone());

        for link in &frag_links {
            if link.kind == LinkKind::External {
                continue;
            }
            if link.original_href.starts_with("mailto:") {
                continue;
            }

            // Only handle links with a line-range fragment.
            let Some(old_start) = link.start_line else {
                continue;
            };
            let old_end = link.end_line.unwrap_or(old_start);

            // Resolve target path.
            let resolved = crate::commands::resolve_link_path(&link.path, file, repo_root);
            let target_abs = repo_root.join(&resolved);

            // Target must exist.
            if !target_abs.exists() {
                continue;
            }

            // The primary signal for Fix #2 is that the mesh reports this anchor
            // as MOVED. Only attempt to rewrite links whose (target, start, end)
            // triple matches a MOVED plan — even if the range is still technically
            // within bounds (the mesh has shifted to a new position).
            let Some((slug, new_path, new_start, new_end)) = eligible
                .get(&(resolved.clone(), old_start, old_end))
                .cloned()
            else {
                continue;
            };

            // Check initial mesh coverage (before auto-follow) to confirm both
            // the wiki page and target path are in the same mesh.
            let is_mesh_covered = mesh_index_opt
                .as_ref()
                .is_some_and(|idx| idx.is_covered(&resolved, old_start, old_end, &wiki_rel));

            if !is_mesh_covered {
                // No mesh covers (wiki, target) together — this is Fix #4 territory (deferred).
                let target_anchor = format!("{}#L{}-L{}", resolved.display(), old_start, old_end);
                skipped.push(SkippedFix {
                    file: file_rel.clone(),
                    line: link.source_line,
                    kind: FixKind::MeshAnchorShift,
                    reason: format!(
                        "no mesh coverage for {target_anchor}; one mesh must anchor BOTH the \
                         wiki page and the code target — create coverage with:\n  \
                         wiki mesh add <slug> {file_rel} {target_anchor} --why \"<rationale>\""
                    ),
                });
                continue;
            }

            let path_changed = new_path != resolved;
            if !path_changed && new_start == old_start && new_end == old_end {
                // No actual change — skip.
                continue;
            }

            // Compute the new path component. For a same-file shift, keep the
            // original addressing verbatim. For a cross-file move, recompute the
            // path in the link's existing addressing style.
            let new_path_part: String = if path_changed {
                let frag_split = link.original_href.find('#');
                let path_only = match frag_split {
                    Some(idx) => &link.original_href[..idx],
                    None => &link.original_href,
                };
                if path_only.starts_with('/') {
                    format!("/{}", new_path.display())
                } else {
                    // Recompute a relative path from the wiki file's directory.
                    rewrite_href(path_only, None, &new_path, file, repo_root)
                }
            } else {
                match link.original_href.find('#') {
                    Some(idx) => link.original_href[..idx].to_string(),
                    None => link.original_href.clone(),
                }
            };
            let new_href = format!("{new_path_part}#L{new_start}-L{new_end}");

            fixes.push(Fix {
                file: file_rel.clone(),
                line: link.source_line,
                kind: FixKind::MeshAnchorShift,
                byte_start: link.href_byte_start,
                byte_end: link.href_byte_end,
                old_href: link.original_href.clone(),
                new_href: new_href.clone(),
                reason: format!(
                    "mesh auto-follow: anchor {resolved:?}#L{old_start}-L{old_end} \
                     moved to #L{new_start}-L{new_end}"
                ),
                confidence: Confidence::High,
            });
            file_patches.push((link.href_byte_start, link.href_byte_end, new_href));

            // Relocate the stored mesh anchor in place so it points at the
            // content's new location with a refreshed hash. Without this the
            // anchor keeps its stale coordinates: it is re-detected as moved on
            // every subsequent run, still claims coverage of the OLD range, and
            // Fix #4's coverage extension appends a duplicate for the new range.
            // Skipped under dry-run (no writes). The new location's bytes are
            // the worktree file, which the move-follow already verified.
            if !dry_run {
                let new_rel = new_path.to_string_lossy().into_owned();
                store::relocate_anchor(
                    repo_root,
                    &slug,
                    &resolved.to_string_lossy(),
                    old_start,
                    old_end,
                    &new_rel,
                    new_start,
                    new_end,
                )?;
            }
        }

        if !file_patches.is_empty() {
            file_patches.sort_by_key(|p| Reverse(p.0));
            let base = if let Some(existing) = patches.get(file) {
                existing.clone()
            } else {
                content.clone()
            };
            let mut patched = base;
            for (start, end, replacement) in file_patches {
                patched.replace_range(start..end, &replacement);
            }
            patches.insert(file.clone(), patched);
        }
    }

    // ── Fix #5: heading-rename anchor rewrite ────────────────────────────────
    //
    // For broken_anchor diagnostics not resolved by Fix #3 (alias), find the
    // slug in the target file's HEAD content, compute its structural position,
    // then check the current (worktree) content for a singleton replacement at
    // the same position. If found, rewrite the anchor.

    // Cache: target path → parsed headings, avoids re-parsing the same target
    // file when multiple broken anchors in one source file reference it.
    let mut headings_cache_f5: HashMap<PathBuf, Vec<Heading>> = HashMap::new();
    // Cache: rel_path → HEAD content
    let mut head_cache: HashMap<String, Option<String>> = HashMap::new();

    for file in files {
        // Use in-memory patched content if prior fixes already rewrote this file.
        let content = if let Some(patched) = patches.get(file) {
            patched.clone()
        } else {
            match content_cache
                .get_or_try_read(file, || std::fs::read_to_string(file))
            {
                Ok(c) => c.to_string(),
                Err(_) => continue,
            }
        };

        let frag_links = parse_fragment_links(&content);
        let mut file_patches: Vec<(usize, usize, String)> = Vec::new();

        let file_rel = file
            .strip_prefix(repo_root)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| file.to_string_lossy().into_owned());

        for link in &frag_links {
            if link.kind == LinkKind::External {
                continue;
            }
            if link.original_href.starts_with("mailto:") {
                continue;
            }

            // Only handle non-line-range fragment links.
            let has_fragment = link.original_href.contains('#');
            if !has_fragment || link.start_line.is_some() {
                continue;
            }

            let anchor = match link.original_href.find('#') {
                Some(idx) => &link.original_href[idx + 1..],
                None => continue,
            };
            if anchor.is_empty() {
                continue;
            }

            // Resolve target file using the in-memory patched view from Fix #1.
            let resolved = crate::commands::resolve_link_path(&link.path, file, repo_root);
            let target_abs = repo_root.join(&resolved);

            // Read current (worktree/patched) content for the target.
            let current_target = if let Some(patched) = patches.get(&target_abs) {
                patched.clone()
            } else {
                match content_cache
                    .get_or_try_read(&target_abs, || std::fs::read_to_string(&target_abs))
                {
                    Ok(c) => c.to_string(),
                    Err(_) => continue,
                }
            };

            // Use cached headings when available to avoid re-parsing the same
            // target file across multiple links.
            let current_headings: &[Heading] = match headings_cache_f5.entry(target_abs.clone()) {
                Entry::Occupied(o) => &*o.into_mut(),
                Entry::Vacant(v) => &*v.insert(extract_headings(&current_target)),
            };

            // If anchor already resolves in current content, nothing to do.
            if resolve_heading(anchor, current_headings) {
                continue;
            }

            // Already handled by Fix #3 if the content still breaks — only
            // attempt Fix #5 for anchors that Fix #3 also could not repair.
            // (Fix #3 runs earlier; if it emitted a Fix, the patch is in
            // `file_patches` for the *source* file. We detect that no fix was
            // applied by the anchor still not resolving.)

            let target_rel = resolved.to_string_lossy().into_owned();

            // Walk layers (HEAD, then HEAD history via `git log --follow`,
            // capped at HEADING_HISTORY_DEPTH_CAP commits) to find the most
            // recent baseline where the broken slug resolves. Newest-first;
            // stops at the first match.
            let anchor_slug = github_slug(anchor);
            let cache_key = format!("{target_rel}#{anchor_slug}");
            let baseline_opt = head_cache.entry(cache_key).or_insert_with(|| {
                find_baseline_with_slug(repo_root, &target_rel, &anchor_slug).unwrap_or(None)
            });

            let Some(baseline_content) = baseline_opt.as_ref() else {
                // Slug not found in HEAD or any historical revision — record
                // an explicit skip so the operator knows Fix #5 declined.
                skipped.push(SkippedFix {
                    file: file_rel.clone(),
                    line: link.source_line,
                    kind: FixKind::HeadingRename,
                    reason: "heading not found in any layer".to_string(),
                });
                continue;
            };

            // Find the structural position of the matching heading in baseline.
            let baseline_positions = heading_positions(baseline_content);
            let Some((_, baseline_pos)) = baseline_positions
                .iter()
                .find(|(h, _)| h.slug == anchor_slug)
                .cloned()
            else {
                continue;
            };

            // Find headings at the same structural position in current content.
            let replacements = headings_at_position(&current_target, &baseline_pos);

            match replacements.len() {
                0 => {
                    skipped.push(SkippedFix {
                        file: file_rel.clone(),
                        line: link.source_line,
                        kind: FixKind::HeadingRename,
                        reason: "heading deleted; no replacement".to_string(),
                    });
                }
                1 => {
                    let new_slug = &replacements[0];
                    if new_slug == &anchor_slug {
                        // Same slug — nothing changed; a different fix is needed.
                        continue;
                    }

                    let path_part = match link.original_href.find('#') {
                        Some(idx) => &link.original_href[..idx],
                        None => continue,
                    };
                    let new_href = format!("{path_part}#{new_slug}");

                    fixes.push(Fix {
                        file: file_rel.clone(),
                        line: link.source_line,
                        kind: FixKind::HeadingRename,
                        byte_start: link.href_byte_start,
                        byte_end: link.href_byte_end,
                        old_href: link.original_href.clone(),
                        new_href: new_href.clone(),
                        reason: format!(
                            "heading `{anchor_slug}` renamed to `{new_slug}` at same structural position"
                        ),
                        confidence: Confidence::Medium,
                    });
                    file_patches.push((link.href_byte_start, link.href_byte_end, new_href));
                }
                n => {
                    skipped.push(SkippedFix {
                        file: file_rel.clone(),
                        line: link.source_line,
                        kind: FixKind::HeadingRename,
                        reason: format!("heading split into {n} replacements"),
                    });
                }
            }
        }

        if !file_patches.is_empty() {
            file_patches.sort_by_key(|p| Reverse(p.0));
            let base = if let Some(existing) = patches.get(file) {
                existing.clone()
            } else {
                content.clone()
            };
            let mut patched = base;
            for (start, end, replacement) in file_patches {
                patched.replace_range(start..end, &replacement);
            }
            patches.insert(file.clone(), patched);
        }
    }

    // Materialize patches to disk unless dry_run.
    if !dry_run {
        for (path, content) in &patches {
            std::fs::write(path, content)
                .map_err(|e| miette::miette!("failed to write {}: {e}", path.display()))?;
        }
    }

    // ── Fix #6: in-place anchor staleness ─────────────────────────────────────
    //
    // For each stale anchor with no unambiguous relocation, auto-re-anchor
    // whitespace-only drift (same coords, refreshed hash) and surface
    // meaning-change / deleted drift fail-closed with a remediation command.
    let mut anchor_stale_count: usize = 0;
    // Repo-relative `.wiki/<slug>` paths whose hash was refreshed — staged via
    // `--print-applied`. Extended into `mesh.applied` once `mesh` exists below.
    let mut anchor_refresh_applied: Vec<String> = Vec::new();

    for d in &drifted {
        let anchor_arg = if d.start == 0 {
            d.path.clone()
        } else {
            format!("{}#L{}-L{}", d.path, d.start, d.end)
        };

        match &d.kind {
            AnchorDriftKind::Changed { whitespace_only: true } => {
                // Auto-re-anchor: same coords, refresh hash to current content.
                if !dry_run {
                    store::relocate_anchor(
                        repo_root,
                        &d.mesh_name,
                        &d.path,
                        d.start,
                        d.end,
                        &d.path,
                        d.start,
                        d.end, // same location → hash-only refresh
                    )?;
                }
                // Emit repo-relative path to --print-applied stdout for staging.
                // Mesh files are extensionless; the hook's `*.md` pattern never
                // matches, so this is the only signal that stages them.
                anchor_refresh_applied.push(format!(".wiki/{}", d.mesh_name));
            }
            AnchorDriftKind::Changed { whitespace_only: false } => {
                anchor_stale_count += 1;
                skipped.push(SkippedFix {
                    file: format!(".wiki/{}", d.mesh_name),
                    line: 0,
                    kind: FixKind::AnchorContentRefresh,
                    reason: format!(
                        "anchor `{anchor_arg}` in mesh `{}` has changed with a meaning-changing \
                         diff; re-anchor to acknowledge:\n  wiki mesh add {} {anchor_arg}",
                        d.mesh_name, d.mesh_name
                    ),
                });
            }
            AnchorDriftKind::Unrecoverable => {
                anchor_stale_count += 1;
                skipped.push(SkippedFix {
                    file: format!(".wiki/{}", d.mesh_name),
                    line: 0,
                    kind: FixKind::AnchorContentRefresh,
                    reason: format!(
                        "anchor `{anchor_arg}` in mesh `{}` drifted, but the pre-edit content \
                         could not be recovered (shallow clone, or the edit is older than {} \
                         commits), so whitespace-equivalence cannot be auto-classified; \
                         re-anchor to acknowledge:\n  wiki mesh add {} {anchor_arg}",
                        d.mesh_name, HISTORY_SCAN_LIMIT, d.mesh_name
                    ),
                });
            }
            AnchorDriftKind::Deleted => {
                anchor_stale_count += 1;
                skipped.push(SkippedFix {
                    file: format!(".wiki/{}", d.mesh_name),
                    line: 0,
                    kind: FixKind::AnchorContentRefresh,
                    reason: format!(
                        "anchor `{anchor_arg}` in mesh `{}` is deleted or out of bounds; \
                         remove the dead anchor first (clears the failure), then optionally \
                         re-anchor the new location:\n  \
                         wiki mesh remove {} {anchor_arg}\n  \
                         # then optionally: wiki mesh add {} <new-location>",
                        d.mesh_name, d.mesh_name, d.mesh_name
                    ),
                });
            }
        }
    }

    // ── Fix #4: mesh coverage ─────────────────────────────────────────────────
    //
    // Runs AFTER the link/anchor patches are written to disk so mesh anchors
    // reflect the corrected line ranges, and BEFORE `check::run`'s post-recheck
    // (which rebuilds the mesh index fresh, so a created mesh shows as covered
    // and a failed/dropped one resurfaces as a residual `mesh_uncovered`).
    // Best-effort: a single mesh that cannot be created is recorded as a
    // failure, never aborting the pass. In `dry_run` it previews only.
    let mut mesh = super::mesh::scaffold::create_mesh_coverage(files, repo_root, source, dry_run, None, git_reader)?;

    // Cleanup stage: delete scaffold meshes whose sole wiki page was removed.
    // Runs AFTER creation so the two halves never interfere.
    // Scoped to the same scan_root + globs used for discovery so a subtree
    // invocation only touches orphans whose sole .md anchor is within scope.
    let cleanup =
        super::mesh::scaffold::cleanup_orphaned_meshes(repo_root, scan_root, globs, dry_run)?;
    mesh.deleted.extend(cleanup.deleted);
    mesh.planned_deletions.extend(cleanup.planned_deletions);
    if !cleanup.advisories.is_empty() {
        mesh.advisories.push_str(&cleanup.advisories);
    }

    // Stage whitespace-only hash refreshes alongside the other applied paths.
    mesh.applied.extend(anchor_refresh_applied);

    Ok(FixPlan {
        fixes,
        skipped,
        mesh,
        mesh_conflicts: mesh_conflicts.len(),
        anchor_stale_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    struct TestRepo {
        dir: TempDir,
    }

    impl TestRepo {
        fn new() -> Self {
            let dir = TempDir::new().expect("tempdir");
            let repo = TestRepo { dir };
            repo.git(&["init", "-q"]);
            repo.git(&["checkout", "-q", "-b", "main"]);
            repo
        }

        fn path(&self) -> &Path {
            self.dir.path()
        }

        fn write(&self, rel: &str, content: &str) {
            let full = self.dir.path().join(rel);
            if let Some(parent) = full.parent() {
                fs::create_dir_all(parent).expect("create_dir_all");
            }
            fs::write(&full, content).expect("write file");
        }

        fn commit(&self, message: &str) {
            self.git(&["add", "-A"]);
            self.git(&["commit", "-q", "-m", message]);
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

    fn wiki_page(title: &str, body: &str) -> String {
        format!("---\ntitle: {title}\nsummary: A page about {title}.\n---\n{body}")
    }

    /// Fix #5 walks HEAD history to recover the prior heading slug after the
    /// rename has already been committed.
    #[test]
    fn fix5_walks_head_history_for_renamed_heading() {
        let repo = TestRepo::new();
        repo.write(
            "wiki/target.md",
            &wiki_page("Target", "## Installation\n\nbody\n"),
        );
        repo.write(
            "wiki/source.md",
            &wiki_page("Source", "See [setup](./target.md#installation).\n"),
        );
        repo.commit("seed");

        // Rename the heading and commit it, so HEAD no longer holds the old slug.
        repo.write(
            "wiki/target.md",
            &wiki_page("Target", "## Setup and Installation\n\nbody\n"),
        );
        repo.commit("rename heading");

        let source = repo.path().join("wiki/source.md");
        let target = repo.path().join("wiki/target.md");
        let plan = run_fix_pass(
            &[source.clone(), target.clone()],
            repo.path(),
            repo.path(),
            &[],
            crate::index::DocSource::WorkingTree,
            /* dry_run */ true,
            &mut ContentCache::new(),
            None,
        )
        .expect("fix pass");

        assert!(
            plan.fixes
                .iter()
                .any(|f| matches!(f.kind, FixKind::HeadingRename)
                    && f.new_href.ends_with("#setup-and-installation")
                    && f.old_href.ends_with("#installation")),
            "expected a HeadingRename fix rewriting #installation → #setup-and-installation; \
             got fixes={:?} skipped={:?}",
            plan.fixes,
            plan.skipped,
        );
    }

    /// `heading_positions` assigns a unique `(depth, parent_slug,
    /// sibling_index)` triple to every heading, so `headings_at_position`
    /// returns at most one match. This test pins that invariant — the multi-
    /// replacement skip path in Fix #5 is reached only through duplicate-
    /// position content, which `heading_positions` does not produce.
    #[test]
    fn headings_at_position_is_at_most_one() {
        let baseline = "## Installation\n\nbody\n";
        let positions = heading_positions(baseline);
        let pos = positions
            .iter()
            .find(|(h, _)| h.slug == "installation")
            .unwrap()
            .1
            .clone();

        // Current content with two same-depth siblings — positions differ in
        // sibling_index, so at most one matches the baseline position.
        let split = headings_at_position("## Setup\n\nx\n## Installation Details\n\ny\n", &pos);
        assert!(
            split.len() <= 1,
            "headings_at_position should return at most one heading per position; got {:?}",
            split
        );
    }

    /// When the broken slug does not resolve in HEAD or any historical
    /// revision, Fix #5 must emit a SkippedFix with the canonical reason.
    #[test]
    fn fix5_skips_when_heading_absent_from_all_layers() {
        let repo = TestRepo::new();
        repo.write(
            "wiki/target.md",
            &wiki_page("Target", "## Something Else\n\nbody\n"),
        );
        repo.write(
            "wiki/source.md",
            &wiki_page("Source", "See [setup](./target.md#installation).\n"),
        );
        repo.commit("seed without installation heading");

        let source = repo.path().join("wiki/source.md");
        let target = repo.path().join("wiki/target.md");
        let plan = run_fix_pass(
            &[source.clone(), target.clone()],
            repo.path(),
            repo.path(),
            &[],
            crate::index::DocSource::WorkingTree,
            /* dry_run */ true,
            &mut ContentCache::new(),
            None,
        )
        .expect("fix pass");

        assert!(
            plan.skipped
                .iter()
                .any(|s| matches!(s.kind, FixKind::HeadingRename)
                    && s.reason == "heading not found in any layer"),
            "expected SkippedFix(HeadingRename, 'heading not found in any layer'); \
             got fixes={:?} skipped={:?}",
            plan.fixes,
            plan.skipped,
        );
    }

    /// Two-layer chained rename (A→B staged, B→C worktree) must resolve to the
    /// terminal destination C, not to Ambiguous([B, C]).
    #[test]
    fn chained_rename_resolves_to_terminal_destination() {
        let repo = TestRepo::new();

        // Write and commit a.rs.
        repo.write("src/a.rs", "content\n");
        repo.commit("add a.rs");

        // Layer 2 (index↔HEAD): stage rename a.rs → b.rs.
        repo.git(&["mv", "src/a.rs", "src/b.rs"]);

        // Layer 1 (worktree↔index): worktree rename b.rs → c.rs via intent-to-add.
        let worktree_b = repo.path().join("src/b.rs");
        let worktree_c = repo.path().join("src/c.rs");
        fs::rename(&worktree_b, &worktree_c).expect("rename b.rs -> c.rs");
        // Intent-to-add c.rs so git diff (worktree↔index) sees the rename.
        repo.git(&["add", "-N", "src/c.rs"]);

        // Build the rename map — this exercises the chain resolution loop.
        let mut map = RenameMap::build(repo.path()).expect("build rename map");

        // a.rs should resolve to the terminal destination c.rs, not Ambiguous.
        match map.successor(Path::new("src/a.rs")) {
            SuccessorResult::Unique(p) => {
                assert_eq!(
                    p, PathBuf::from("src/c.rs"),
                    "chained rename A→B→C must resolve to terminal C"
                );
            }
            SuccessorResult::Ambiguous(dests) => {
                panic!(
                    "expected Unique(src/c.rs) but got Ambiguous({:?}) — \
                     chain loop is appending instead of replacing intermediate destinations",
                    dests
                );
            }
            other => {
                panic!("expected Unique, got {:?}", other);
            }
        }
    }

    /// When the caller supplies a pre-built `MeshIndex`, `run_fix_pass` must
    /// reuse it (clone) for Fix #2 coverage checks and Fix #4 coverage
    /// creation instead of re-building from the `.wiki/` store. This test:
    /// 1. Creates a wiki page with a line-range link to a code file.
    /// 2. Creates a `.wiki/` mesh covering both files.
    /// 3. Shifts the code file so the anchor is stale.
    /// 4. Builds the MeshIndex from committed mesh files.
    /// 5. Runs `run_fix_pass` with `Some(&mesh_index)` (dry-run).
    /// 6. Asserts the plan contains a `MeshAnchorShift` fix — proving the
    ///    pre-built index was used for the coverage gate, which is the
    ///    precondition for emitting the fix.
    #[test]
    fn fix_pass_with_prebuilt_mesh_index_resolves_mesh_anchor_shift() {
        let repo = TestRepo::new();
        use git_mesh_core::AnchorExtent;
        use git_mesh_core::mesh_file::MeshFile;

        // Write the code file with a known block at lines 1–3.
        repo.write("src/lib.rs", "fn foo() {\n    42\n}\n");
        // Write a wiki page linking to the code block with a line-range anchor.
        repo.write(
            "wiki/page.md",
            &wiki_page("Page", "See [code](../src/lib.rs#L1-L3).\n"),
        );
        repo.commit("seed");

        // Create a mesh that anchors both the code block and the wiki page.
        let code_extent = AnchorExtent::LineRange { start: 1, end: 3 };
        let mut content_cache = store::FileContentCache::new();
        let code_hash =
            store::hash_anchor(repo.path(), "src/lib.rs", code_extent, &mut content_cache).unwrap();
        let code_anchor =
            store::anchor_record("src/lib.rs".to_string(), code_extent, code_hash);
        let wiki_extent = AnchorExtent::LineRange { start: 1, end: 3 };
        let mut content_cache = store::FileContentCache::new();
        let wiki_hash =
            store::hash_anchor(repo.path(), "wiki/page.md", wiki_extent, &mut content_cache).unwrap();
        let wiki_anchor =
            store::anchor_record("wiki/page.md".to_string(), wiki_extent, wiki_hash);
        let mesh = MeshFile {
            anchors: vec![code_anchor, wiki_anchor],
            why: String::new(),
        };
        store::write(repo.path(), "my-mesh", &mesh).unwrap();
        repo.commit("add mesh");

        // Make the code block stale by prepending a line: the original block
        // at lines 1–3 shifts to lines 2–4.
        repo.write("src/lib.rs", "// header\nfn foo() {\n    42\n}\n");
        // Leave uncommitted — dirty working tree ensures `plan_mesh_follows`
        // detects the drift (the `tree_unchanged` sqlite-gate is absent).

        let wiki_abs = repo.path().join("wiki/page.md");
        let lib_abs = repo.path().join("src/lib.rs");
        let mut content_cache = ContentCache::new();
        let plan = run_fix_pass(
            &[wiki_abs, lib_abs],
            repo.path(),
            repo.path(),
            &[],
            crate::index::DocSource::WorkingTree,
            /* dry_run */ true,
            &mut content_cache,
            None,
        )
        .expect("fix pass");

        // Move-follow should relocate the code anchor from L1-L3 to L2-L4
        // because the content shifted down by one line. The pre-built
        // MeshIndex supplies coverage info, which confirms the mesh covers
        // the (wiki, target) pair — so a MeshAnchorShift fix is produced
        // rather than a "no mesh coverage" skip.
        assert!(
            plan.fixes
                .iter()
                .any(|f| matches!(f.kind, FixKind::MeshAnchorShift)),
            "expected a MeshAnchorShift fix for the shifted anchor; \
             got fixes={:?} skipped={:?}",
            plan.fixes,
            plan.skipped,
        );
    }

    // ── mesh staleness fixture (shared with phase-2 move/conflict tests) ─────

    /// Build a git repo with a `.wiki/` mesh whose anchor points at `src/lib.rs`
    /// at a known line range, then append a line to the file so the anchor is
    /// stale and the function actually runs the scan path.
    ///
    /// Returns `(repo, anchor_content_hash)`.
    #[allow(dead_code)]
    fn setup_stale_mesh_repo() -> (TestRepo, String) {
        use crate::commands::mesh::store;
        use git_mesh_core::AnchorExtent;
        use git_mesh_core::mesh_file::MeshFile;

        let repo = TestRepo::new();

        // Write a source file with a known block on lines 1–3 (1-based).
        let content = "fn foo() {\n    42\n}\n";
        repo.write("src/lib.rs", content);
        repo.git(&["config", "user.email", "t@t"]);
        repo.git(&["config", "user.name", "t"]);
        repo.commit("initial");

        // Hash the exact extent as wiki would store it.
        let extent = AnchorExtent::LineRange { start: 1, end: 3 };
        let hash =
            store::hash_anchor(repo.path(), "src/lib.rs", extent, &mut store::FileContentCache::new())
                .unwrap();

        // Write the mesh file.
        let anchor = store::anchor_record("src/lib.rs".to_string(), extent, hash.clone());
        let mesh = MeshFile {
            anchors: vec![anchor],
            why: String::new(),
        };
        store::write(repo.path(), "myslug", &mesh).unwrap();
        repo.commit("add mesh");

        // Now mutate the source file to make the anchor stale (the block
        // has shifted: a line was prepended).
        let mutated = "// header\nfn foo() {\n    42\n}\n";
        repo.write("src/lib.rs", mutated);
        // Do NOT commit — leaving the file dirty ensures `tree_unchanged`
        // returns false so `plan_mesh_follows` actually runs.

        (repo, hash)
    }

    #[test]
    fn test_plan_mesh_follows_skips_wikiignored_anchor_target() {
        use crate::commands::mesh::store;
        use git_mesh_core::AnchorExtent;
        use git_mesh_core::mesh_file::MeshFile;

        let repo = TestRepo::new();

        // Source file with a known block on lines 1–3.
        repo.write("src/lib.rs", "fn foo() {\n    42\n}\n");
        repo.commit("initial");

        let extent = AnchorExtent::LineRange { start: 1, end: 3 };
        let hash = store::hash_anchor(
            repo.path(),
            "src/lib.rs",
            extent,
            &mut store::FileContentCache::new(),
        )
        .unwrap();
        let anchor = store::anchor_record("src/lib.rs".to_string(), extent, hash);
        let mesh = MeshFile {
            anchors: vec![anchor],
            why: String::new(),
        };
        store::write(repo.path(), "myslug", &mesh).unwrap();
        repo.commit("add mesh");

        // Wikiignore the anchor's source file, then make it stale.
        repo.write(".wiki/.wikiignore", "src/lib.rs\n");
        repo.write("src/lib.rs", "// header\nfn foo() {\n    42\n}\n");
        // Leave uncommitted (dirty) so plan_mesh_follows runs.

        let outcome =
            plan_mesh_follows(repo.path(), crate::index::DocSource::WorkingTree).expect("plan");
        assert!(
            outcome.drifted.is_empty(),
            "a wikiignored anchor target must produce no drift: {:?}",
            outcome.drifted
        );
    }

    // ── Conflict resolution tests ─────────────────────────────────────────────

    /// A conflicted mesh file with differing anchors (the why text outside
    /// markers goes to both sides).
    const CONFLICTED_MESH: &str = "<<<<<<< HEAD\n\
         src/file.rs rk64:abc123\n\
         =======\n\
         src/file.rs rk64:def456\n\
         >>>>>>> other\n\
         \n\
         Why text.";

    #[test]
    fn split_conflict_blocks_simple() {
        let (ours, theirs) = split_conflict_blocks(CONFLICTED_MESH).unwrap();
        // Lines outside markers (blank line + why text) go to both sides.
        assert_eq!(ours, "src/file.rs rk64:abc123\n\nWhy text.\n");
        assert_eq!(theirs, "src/file.rs rk64:def456\n\nWhy text.\n");

        // Parse each side as a valid MeshFile (MeshFile::parse handles
        // the blank-line separator between anchors and why text).
        let ours_mesh = MeshFile::parse(&ours).unwrap();
        let theirs_mesh = MeshFile::parse(&theirs).unwrap();
        assert_eq!(ours_mesh.anchors.len(), 1);
        assert_eq!(theirs_mesh.anchors.len(), 1);
        assert_eq!(ours_mesh.why, "Why text.");
        assert_eq!(theirs_mesh.why, "Why text.");
    }

    #[test]
    fn split_conflict_blocks_no_markers() {
        let text = "src/a.rs rk64:abc123\n\nSome why.\n";
        let (ours, theirs) = split_conflict_blocks(text).unwrap();
        assert_eq!(ours, text);
        assert_eq!(theirs, text);
    }

    #[test]
    fn split_conflict_blocks_multiple_blocks() {
        let text = "<<<<<<< HEAD\n\
             src/a.rs rk64:aaa\n\
             =======\n\
             src/a.rs rk64:bbb\n\
             >>>>>>> theirs\n\
             \n\
             <<<<<<< HEAD\n\
             src/b.rs rk64:ccc\n\
             =======\n\
             src/b.rs rk64:ddd\n\
             >>>>>>> theirs\n";
        let (ours, theirs) = split_conflict_blocks(text).unwrap();
        // The blank line between conflict blocks is outside markers → both sides.
        assert_eq!(ours, "src/a.rs rk64:aaa\n\nsrc/b.rs rk64:ccc\n");
        assert_eq!(theirs, "src/a.rs rk64:bbb\n\nsrc/b.rs rk64:ddd\n");
    }

    #[test]
    fn split_conflict_blocks_unterminated_is_error() {
        let text = "<<<<<<< HEAD\nsrc/a.rs rk64:abc\n";
        assert!(split_conflict_blocks(text).is_err());
    }

    #[test]
    fn split_conflict_blocks_lines_outside_markers_go_to_both() {
        let text = "src/shared.rs rk64:xyz123\n\
             <<<<<<< HEAD\n\
             src/ours.rs rk64:aaa\n\
             =======\n\
             src/theirs.rs rk64:bbb\n\
             >>>>>>> other\n";
        let (ours, theirs) = split_conflict_blocks(text).unwrap();
        assert!(ours.contains("src/shared.rs rk64:xyz123"));
        assert!(theirs.contains("src/shared.rs rk64:xyz123"));
        assert!(ours.contains("src/ours.rs rk64:aaa"));
        assert!(!ours.contains("src/theirs.rs rk64:bbb"));
        assert!(!theirs.contains("src/ours.rs rk64:aaa"));
        assert!(theirs.contains("src/theirs.rs rk64:bbb"));
    }

    /// 1.4 union-of-divergent-anchors: different anchors on each branch → one
    /// canonical mesh with the union of both sides.
    #[test]
    fn resolve_conflicted_union_of_anchors() {
        let repo = TestRepo::new();
        let root = repo.path();

        // Ours has src/a.rs, theirs has src/b.rs — no overlap. Why text is
        // the same on both sides and placed outside markers so it goes to both.
        let conflicted = "\
             <<<<<<< HEAD\n\
             src/a.rs rk64:aaaaaaaaaaaaaaaa\n\
             =======\n\
             src/b.rs rk64:bbbbbbbbbbbbbbbb\n\
             >>>>>>> other\n\
             \n\
             Unified why.\n";

        // Create the source files the anchors reference.
        repo.write("src/a.rs", "// a\n");
        repo.write("src/b.rs", "// b\n");

        // Write the conflicted mesh.
        let wiki = root.join(".wiki");
        fs::create_dir_all(&wiki).unwrap();
        fs::write(wiki.join("myslug"), conflicted).unwrap();

        let report = resolve_conflicted_meshes(root).unwrap();
        assert!(
            report.skipped.is_empty(),
            "expected no skipped: {:?}",
            report.skipped
        );
        assert_eq!(report.resolved.len(), 1, "myslug must be resolved");
        assert!(report.partial.is_empty());

        let mesh = store::read_one(root, "myslug").unwrap().unwrap();
        assert_eq!(mesh.anchors.len(), 2, "expected union of both sides");
        let paths: Vec<&str> = mesh.anchors.iter().map(|a| a.path.as_str()).collect();
        assert!(paths.contains(&"src/a.rs"));
        assert!(paths.contains(&"src/b.rs"));
        assert_eq!(mesh.why, "Unified why.");
    }

    /// Same-anchor divergence (range shift): same anchor path+extent on both
    /// sides, different content_hash. With source files provided, merge_mesh_files
    /// re-hashes against the worktree and resolves the divergence.
    #[test]
    fn resolve_conflicted_same_anchor_rehashed() {
        let repo = TestRepo::new();
        let root = repo.path();

        // Source file with known content.
        repo.write("src/file.rs", "fn current() {}\n");

        // Compute the hash that the worktree file produces at L1-L1.
        let extent = git_mesh_core::AnchorExtent::LineRange { start: 1, end: 1 };
        let worktree_hash = store::hash_anchor(
            root,
            "src/file.rs",
            extent,
            &mut store::FileContentCache::new(),
        )
        .unwrap();

        // Conflicted mesh with same anchor, different hashes on each side.
        let conflicted = "<<<<<<< HEAD\n\
             src/file.rs#L1-L1 rk64:aaaaaaaaaaaaaaaa\n\
             =======\n\
             src/file.rs#L1-L1 rk64:bbbbbbbbbbbbbbbb\n\
             >>>>>>> other\n\
             \n\
             Why.";

        let wiki = root.join(".wiki");
        fs::create_dir_all(&wiki).unwrap();
        fs::write(wiki.join("slug"), conflicted).unwrap();

        let report = resolve_conflicted_meshes(root).unwrap();
        assert!(
            report.skipped.is_empty(),
            "expected no skipped: {:?}",
            report.skipped
        );
        assert_eq!(report.resolved.len(), 1, "must be resolved via re-hash");

        let mesh = store::read_one(root, "slug").unwrap().unwrap();
        assert_eq!(mesh.anchors.len(), 1);
        assert_eq!(mesh.anchors[0].content_hash, worktree_hash);
    }

    /// Clean-source precondition: source file has conflict markers → mesh is
    /// skipped with a reason naming the conflicted source file.
    #[test]
    fn resolve_conflicted_skipped_when_source_has_markers() {
        let repo = TestRepo::new();
        let root = repo.path();

        // Source file with conflict markers.
        repo.write("src/file.rs", "<<<<<<< HEAD\nstuff\n=======\nthings\n>>>>>>> other\n");

        let conflicted = "\
             <<<<<<< HEAD\n\
             src/file.rs rk64:aaa\n\
             =======\n\
             src/file.rs rk64:bbb\n\
             >>>>>>> other\n\
             \n\
             Why.\n";

        let wiki = root.join(".wiki");
        fs::create_dir_all(&wiki).unwrap();
        fs::write(wiki.join("slug"), conflicted).unwrap();

        let report = resolve_conflicted_meshes(root).unwrap();
        assert_eq!(report.resolved.len(), 0);
        assert_eq!(report.partial.len(), 0);
        assert_eq!(
            report.skipped.len(),
            1,
            "must skip mesh when source has markers: {:?}",
            report.skipped
        );
        let (slug, reason) = &report.skipped[0];
        assert_eq!(slug, "slug");
        assert!(
            reason.contains("src/file.rs"),
            "reason must name the conflicted source: {reason}"
        );
    }

    /// Empty-path `--why` sentinel: both branches changed the why text → the
    /// mesh is partially resolved (anchors clean, why in conflict — partial exit).
    #[test]
    fn resolve_conflicted_why_only_partial() {
        let repo = TestRepo::new();
        let root = repo.path();

        // Same anchor on both sides (identical hash). The blank line before
        // the markers separates the anchor block from the why text.
        let conflicted = "src/a.rs rk64:aaaaaaaaaaaaaaaa\n\
             \n\
             <<<<<<< HEAD\n\
             Why from HEAD.\n\
             =======\n\
             Why from other.\n\
             >>>>>>> other\n";

        repo.write("src/a.rs", "// a\n");
        let wiki = root.join(".wiki");
        fs::create_dir_all(&wiki).unwrap();
        fs::write(wiki.join("slug"), conflicted).unwrap();

        let report = resolve_conflicted_meshes(root).unwrap();
        assert!(
            report.skipped.is_empty(),
            "expected no skipped: {:?}",
            report.skipped
        );
        assert_eq!(report.partial.len(), 1, "why conflict is partial");
        assert!(report.resolved.is_empty());

        // File should still exist on disk (not git added).
        let path = wiki.join("slug");
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("src/a.rs rk64:aaaaaaaaaaaaaaaa"));
    }

    /// Partial via why conflict: same anchors on both sides but different why
    /// text → mesh is partially resolved (anchors clean, why in markers).
    /// NOT staged.
    #[test]
    fn resolve_conflicted_partial_via_why_conflict() {
        let repo = TestRepo::new();
        let root = repo.path();

        repo.write("src/shared.rs", "// shared\n");
        // Same anchor on both sides; why text diverges (inside markers after
        // the blank-line separator so it's treated as why, not anchors).
        let conflicted = "src/shared.rs rk64:cccccccccccccccc\n\
             \n\
             <<<<<<< HEAD\n\
             Why from HEAD.\n\
             =======\n\
             Why from other.\n\
             >>>>>>> other\n";

        let wiki = root.join(".wiki");
        fs::create_dir_all(&wiki).unwrap();
        fs::write(wiki.join("slug"), conflicted).unwrap();

        let report = resolve_conflicted_meshes(root).unwrap();
        assert!(
            report.skipped.is_empty(),
            "expected no skipped: {:?}",
            report.skipped
        );
        assert_eq!(report.partial.len(), 1, "expected partial from why conflict");
        assert!(report.resolved.is_empty(), "no fully resolved meshes");

        let path = wiki.join("slug");
        let content = fs::read_to_string(&path).unwrap();
        // The shared anchor should be present cleanly.
        assert!(content.contains("src/shared.rs rk64:cccccccccccccccc"));
    }

    /// Canonical ordering: ours and theirs differ only in anchor order →
    /// merged result has canonical (path, start_line, end_line) order, no residue.
    #[test]
    fn resolve_conflicted_canonical_ordering() {
        let repo = TestRepo::new();
        let root = repo.path();

        repo.write("src/a.rs", "// a\n");
        repo.write("src/b.rs", "// b\n");

        // Ours: [b.rs, a.rs]; theirs: [a.rs, b.rs]
        let conflicted = "\
             <<<<<<< HEAD\n\
             src/b.rs rk64:bbbbbbbbbbbbbbbb\n\
             src/a.rs rk64:aaaaaaaaaaaaaaaa\n\
             =======\n\
             src/a.rs rk64:aaaaaaaaaaaaaaaa\n\
             src/b.rs rk64:bbbbbbbbbbbbbbbb\n\
             >>>>>>> other\n\
             \n\
             Why.\n";

        let wiki = root.join(".wiki");
        fs::create_dir_all(&wiki).unwrap();
        fs::write(wiki.join("slug"), conflicted).unwrap();

        let report = resolve_conflicted_meshes(root).unwrap();
        assert!(
            report.skipped.is_empty(),
            "expected no skipped: {:?}",
            report.skipped
        );
        assert_eq!(report.resolved.len(), 1, "must be resolved");
        assert!(report.partial.is_empty());

        let mesh = store::read_one(root, "slug").unwrap().unwrap();
        assert_eq!(mesh.anchors.len(), 2);
        // Canonical order: src/a.rs before src/b.rs.
        assert_eq!(mesh.anchors[0].path, "src/a.rs");
        assert_eq!(mesh.anchors[1].path, "src/b.rs");
    }

    /// Diff3 base markers (|||||||) are discarded during marker splitting.
    #[test]
    fn split_conflict_blocks_discards_diff3_base() {
        let text = "<<<<<<< HEAD\n\
             src/file.rs rk64:ours_hash\n\
             |||||||\n\
             src/file.rs rk64:base_hash\n\
             =======\n\
             src/file.rs rk64:theirs_hash\n\
             >>>>>>> other\n";
        let (ours, theirs) = split_conflict_blocks(text).unwrap();
        // Ours should NOT contain the base marker or base content.
        assert_eq!(ours, "src/file.rs rk64:ours_hash\n");
        assert_eq!(theirs, "src/file.rs rk64:theirs_hash\n");
    }

    /// build_partial_output produces the expected format with resolved anchors,
    /// conflict markers for residuals, and why text.
    #[test]
    fn build_partial_output_format() {
        let resolved = vec![
            AnchorRecord {
                path: "src/clean.rs".to_string(),
                start_line: 1,
                end_line: 1,
                algorithm: "rk64".to_string(),
                content_hash: "aaa".to_string(),
            },
        ];
        let residual = vec![
            UnresolvedAnchor {
                path: "src/conflicted.rs".to_string(),
                start_line: 2,
                end_line: 4,
                ours: AnchorRecord {
                    path: "src/conflicted.rs".to_string(),
                    start_line: 2,
                    end_line: 4,
                    algorithm: "rk64".to_string(),
                    content_hash: "bbb".to_string(),
                },
                theirs: AnchorRecord {
                    path: "src/conflicted.rs".to_string(),
                    start_line: 2,
                    end_line: 4,
                    algorithm: "rk64".to_string(),
                    content_hash: "ccc".to_string(),
                },
            },
        ];
        let why = "Why text.";
        let output = build_partial_output(&resolved, why, &residual, &[], "", "");
        assert!(output.contains("src/clean.rs#L1-L1 rk64:aaa"));
        assert!(output.contains("<<<<<<< ours"));
        assert!(output.contains("======="));
        assert!(output.contains(">>>>>>> theirs"));
        assert!(output.contains("Why text."));
        assert!(output.contains("src/conflicted.rs#L2-L4 rk64:bbb"));
        assert!(output.contains("src/conflicted.rs#L2-L4 rk64:ccc"));
    }

    /// find_conflicted_meshes in store.rs works end-to-end: a mesh with markers
    /// is returned, a clean mesh is not.
    #[test]
    fn find_conflicted_meshes_e2e() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        let wiki = root.join(".wiki");
        fs::create_dir_all(&wiki).unwrap();

        // Clean mesh.
        let clean = "src/clean.rs rk64:aaa\n\nClean.\n";
        fs::write(wiki.join("clean"), clean).unwrap();

        // Conflicted mesh.
        let conflicted = "<<<<<<< HEAD\nsrc/c.rs rk64:bbb\n=======\nsrc/c.rs rk64:ccc\n>>>>>>> t\n\nWhy.\n";
        fs::write(wiki.join("conflicted"), conflicted).unwrap();

        let results = crate::commands::mesh::store::find_conflicted_meshes(root).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "conflicted");
    }
}
