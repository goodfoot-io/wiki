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

use crate::commands::mesh::store;
use crate::commands::mesh_coverage::build_mesh_index;
use crate::frontmatter::parse_frontmatter;
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
    /// Plausible but could be wrong; requires human review.
    Low,
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

// ── Layered baseline reader ────────────────────────────────────────────────────

/// Read `path` from the most recent layer that holds a baseline version
/// (worktree → index → HEAD). Returns `(content, source_label)` for the first
/// layer whose content is non-empty, or `Ok(None)` when no baseline exists.
///
/// For Fix #1, we only need to know if the path existed in git history — the
/// content is not used for path rewriting. The staged-mesh layer is deferred to
/// Fix #2.
#[allow(dead_code)]
pub fn read_at_baseline(path: &Path, repo_root: &Path) -> Result<Option<(String, &'static str)>> {
    let path_rel = path
        .strip_prefix(repo_root)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string_lossy().into_owned());

    // Worktree
    if let Some(s) = DocSource::WorkingTree.read(repo_root, &path_rel)? {
        return Ok(Some((s, "worktree")));
    }

    // Index
    if let Some(s) = DocSource::Index.read(repo_root, &path_rel)? {
        return Ok(Some((s, "index")));
    }

    // HEAD
    if let Some(s) = DocSource::Head.read(repo_root, &path_rel)? {
        return Ok(Some((s, "head")));
    }

    // HEAD history via git log --follow
    let output = Command::new("git")
        .current_dir(repo_root)
        .args([
            "log",
            "--diff-filter=R",
            "--follow",
            "--format=%H",
            "--",
            &path_rel,
        ])
        .output()
        .map_err(|e| miette::miette!("git log failed: {e}"))?;

    if output.status.success() {
        let text = String::from_utf8_lossy(&output.stdout);
        for sha in text.lines() {
            let sha = sha.trim();
            if sha.is_empty() {
                continue;
            }
            let blob = Command::new("git")
                .current_dir(repo_root)
                .args(["show", &format!("{sha}:{path_rel}")])
                .output()
                .map_err(|e| miette::miette!("git show failed: {e}"))?;
            if blob.status.success()
                && let Ok(s) = String::from_utf8(blob.stdout)
            {
                // Leak sha string for 'static lifetime — only in tests / rare paths.
                let label = Box::leak(format!("history:{sha}").into_boxed_str());
                return Ok(Some((s, label)));
            }
        }
    }

    Ok(None)
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
}

/// Compute mesh move plans in-process from the `.wiki/` store.
///
/// Hot-path gate: if the working tree is unchanged since the last verified
/// index state ([`crate::index::tree_unchanged`]), no anchor can have drifted,
/// so we return no plans without re-hashing anything.
///
/// Per anchor (across every mesh in `.wiki/`):
/// 1. **Freshness.** Re-hash the anchored extent in the file it names. If it
///    equals the stored hash, the anchor is fresh — skip.
/// 2. **Same-file-first.** If stale, scan only the anchor's own file with
///    `near = old_start` (whole-file: no `near`). Exactly one match ⇒ a plan to
///    the new range in the same file.
/// 3. **Change set.** Still unmatched ⇒ scan the working-tree change set with
///    `near = None`. Exactly one match ⇒ plan. Two or more matches ⇒ conflict:
///    no plan, the link is left untouched (fail-closed). Zero ⇒ genuinely
///    stale, no auto-fix.
///
/// The candidate set is the change set by default; there is no `--rescan` flag
/// in the current CLI surface, so a whole-tree rescan is not offered here.
pub fn plan_mesh_follows(repo_root: &Path) -> Result<MeshFollowOutcome> {
    // Stat-gate the whole pass: an unchanged tree cannot have drifted anchors,
    // so there is nothing to re-hash.
    if crate::index::tree_unchanged(repo_root) {
        return Ok(MeshFollowOutcome::default());
    }

    let meshes = store::read_all(repo_root)?;
    if meshes.is_empty() {
        return Ok(MeshFollowOutcome::default());
    }

    // Working-tree change set: repo-relative paths that differ from HEAD
    // (staged, unstaged, or untracked).
    let change_set = working_tree_change_set(repo_root)?;

    // Read every file an anchor names plus every change-set file exactly once
    // (a file with K anchors is read once, not K times), so `LineIndex` and the
    // content hashes are computed over a single owned buffer per path.
    let mut file_bytes: HashMap<String, Vec<u8>> = HashMap::new();
    for path in change_set.iter().cloned().chain(
        meshes
            .iter()
            .flat_map(|(_, m)| m.anchors.iter().map(|a| a.path.clone())),
    ) {
        file_bytes
            .entry(path.clone())
            .or_insert_with(|| std::fs::read(repo_root.join(&path)).unwrap_or_default());
    }

    // Build the change-set line index once and reuse it for every stale
    // anchor's cross-file search. The anchor's own file may appear here too:
    // by the time control reaches the change-set scan the own-file pass has
    // already found 0 matches (a unique hit or a ≥2 ambiguity would have
    // `continue`d), so its inclusion contributes nothing and the result is
    // identical to excluding it.
    let change_set_idx: Vec<(String, LineIndex)> = change_set
        .iter()
        .filter_map(|p| {
            let b = file_bytes.get(p)?;
            (!b.is_empty()).then(|| (p.clone(), LineIndex::build(b)))
        })
        .collect();

    let mut plans: Vec<MeshMovePlan> = Vec::new();
    let mut conflicts: Vec<MeshConflict> = Vec::new();

    for (slug, mesh) in &meshes {
        for anchor in &mesh.anchors {
            let extent = if anchor.start_line == 0 && anchor.end_line == 0 {
                AnchorExtent::WholeFile
            } else {
                AnchorExtent::LineRange {
                    start: anchor.start_line,
                    end: anchor.end_line,
                }
            };

            // The stored content identity is an rk64 fingerprint (16 hex).
            // A record that does not parse as rk64 cannot be matched against
            // rk64 windows — skip it rather than guess.
            let Some(stored_fp) = rk64_from_hex(&anchor.content_hash) else {
                continue;
            };

            // The anchor's own file, read once above. Missing/empty ⇒ skip
            // straight to the change-set search.
            let own_bytes = file_bytes
                .get(&anchor.path)
                .map(Vec::as_slice)
                .unwrap_or(&[]);

            if !own_bytes.is_empty() {
                // Index the own file once and reuse it for both the freshness
                // check and the same-file move scan.
                let own_pair = [(anchor.path.clone(), LineIndex::build(own_bytes))];

                // 1. Freshness — re-fingerprint in place. If it matches, the
                //    anchor is fresh; nothing to do.
                if cheap_fingerprint_indexed(&own_pair[0].1, &extent) == stored_fp {
                    continue;
                }

                // 2. Same-file-first move (exhaustive over the file's windows,
                //    fail-closed on rk64 ambiguity).
                let near = match extent {
                    AnchorExtent::WholeFile => None,
                    AnchorExtent::LineRange { start, .. } => Some(start),
                };
                let hits = scan_indexed_rk64(&own_pair, stored_fp, extent, near);
                if hits.len() == 1 {
                    push_plan(&mut plans, slug, anchor, &hits[0]);
                    continue;
                }
                if hits.len() >= 2 {
                    // Same-file ambiguity: the content appears in ≥ 2 places in
                    // the anchor's own file. Fail-closed — report, no rewrite.
                    push_conflict(&mut conflicts, slug, anchor, hits.len());
                    continue;
                }
            }

            // 3. Change-set move. Exactly one match ⇒ a unique relocation.
            //    Zero ⇒ genuinely stale, no auto-fix. Two or more ⇒ conflict:
            //    fail-closed, leave the link untouched.
            let hits = scan_indexed_rk64(&change_set_idx, stored_fp, extent, None);
            if hits.len() == 1 {
                push_plan(&mut plans, slug, anchor, &hits[0]);
            } else if hits.len() >= 2 {
                push_conflict(&mut conflicts, slug, anchor, hits.len());
            }
        }
    }

    Ok(MeshFollowOutcome { plans, conflicts })
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
pub fn run_fix_pass(
    files: &[PathBuf],
    repo_root: &Path,
    scan_root: &Path,
    globs: &[String],
    source: DocSource,
    dry_run: bool,
    content_cache: &mut ContentCache,
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
    } = plan_mesh_follows(repo_root)?;

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

    // ── Fix #4: mesh coverage ─────────────────────────────────────────────────
    //
    // Runs AFTER the link/anchor patches are written to disk so mesh anchors
    // reflect the corrected line ranges, and BEFORE `check::run`'s post-recheck
    // (which rebuilds the mesh index fresh, so a created mesh shows as covered
    // and a failed/dropped one resurfaces as a residual `mesh_uncovered`).
    // Best-effort: a single mesh that cannot be created is recorded as a
    // failure, never aborting the pass. In `dry_run` it previews only.
    let mut mesh = super::mesh::scaffold::create_mesh_coverage(files, repo_root, source, dry_run, None)?;

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

    Ok(FixPlan {
        fixes,
        skipped,
        mesh,
        mesh_conflicts: mesh_conflicts.len(),
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
}
