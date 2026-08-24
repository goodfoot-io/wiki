use std::cmp::Reverse;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::io;
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

use miette::Result;
use serde::{Deserialize, Serialize};

use super::check::{ContentCache, anchor_cache_for_run};
use super::drift;
use crate::frontmatter::parse_frontmatter;
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
    /// Fix 3: rewrite an alias href to the canonical slug.
    AliasToCanonical,
    /// Fix 5: update a heading anchor that was renamed in-place (same position).
    HeadingRename,
    /// Drift pass: rewrite a relocated link's full href (path and range).
    LinkRelocate,
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
#[derive(Debug)]
pub struct FixPlan {
    pub fixes: Vec<Fix>,
    pub skipped: Vec<SkippedFix>,
    /// Number of `Unknown` (unverified) drift outcomes. Fail-closed: each is
    /// also recorded in `skipped`; this count drives a non-zero exit in both
    /// fix-mode gates — wired by name per the span-group doctrine (an unwired
    /// count field compiles silently).
    pub unverified: usize,
    /// Number of `Drift`/`Uncertified` outcomes that `--fix` cannot
    /// auto-settle (only a `links-reviewed` bump certifies them). Each is also
    /// recorded in `skipped` with the bump remedy; the count drives a
    /// non-zero exit in both fix-mode gates.
    pub certification_skips: usize,
    /// Repo-relative paths of every file the pass wrote (markdown rewrites,
    /// relocations, field initializations) — `--print-applied` prints exactly
    /// this list, in write order.
    pub applied_paths: Vec<String>,
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
    /// Cache for on-demand git-history lookups (follow walk for present
    /// paths, deletion-walk + `git show` for deleted paths).
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
    /// On miss, performs an on-demand git-history lookup and caches the
    /// result. Every destination is followed to its own chain end: a
    /// destination that no longer exists at HEAD may be an intermediate of a
    /// further committed rename, and the runtime lookup resolves it the same
    /// way the build-time chain loop resolves map rows.
    pub fn successor(&mut self, old_path: &Path) -> SuccessorResult {
        let dests = self.lookup_dests(old_path);
        let ends = self.chain_ends(dests);
        self.classify(&ends)
    }

    /// Resolve `old_path`'s recorded successors: in-memory map, then the
    /// on-demand cache, then git history (inserted into both, so a result
    /// found here is visible to every later lookup — the runtime counterpart
    /// of the build-time chain loop).
    fn lookup_dests(&mut self, old_path: &Path) -> Vec<String> {
        let key = old_path.to_string_lossy().into_owned();
        if let Some(dests) = self.map.get(&key) {
            return dests.clone();
        }
        if let Some(cached) = self.log_cache.get(&key) {
            return cached.clone();
        }
        let results = git_log_follow_renames(&self.repo_root, old_path).unwrap_or_default();
        if !results.is_empty() {
            self.map.insert(key.clone(), results.clone());
        }
        self.log_cache.insert(key, results.clone());
        results
    }

    /// Follow every destination through further renames to its chain end. A
    /// destination present at HEAD is terminal — any further committed
    /// rename would have removed it — so only missing destinations expand
    /// through `lookup_dests` (which consults the map and history, walking
    /// deletion history for committed renames). A visited set guards cycles:
    /// a cycle member is kept as-is, and the caller's existence check fails
    /// closed on it.
    fn chain_ends(&mut self, dests: Vec<String>) -> Vec<String> {
        let mut visited: HashSet<String> = HashSet::new();
        let mut current = dests;
        loop {
            let mut changed = false;
            let mut next = Vec::new();
            for d in current {
                if !visited.insert(d.clone()) {
                    next.push(d);
                    continue;
                }
                if self.repo_root.join(&d).exists() {
                    next.push(d);
                    continue;
                }
                let further = self.lookup_dests(Path::new(&d));
                if further.is_empty() {
                    next.push(d);
                } else {
                    next.extend(further);
                    changed = true;
                }
            }
            current = next;
            if !changed {
                break;
            }
        }
        current
    }

    fn classify(&self, dests: &[String]) -> SuccessorResult {
        let mut unique: Vec<String> = Vec::new();
        for d in dests {
            if !unique.contains(d) {
                unique.push(d.clone());
            }
        }
        match unique.len() {
            0 => SuccessorResult::None,
            1 => SuccessorResult::Unique(PathBuf::from(&unique[0])),
            _ => SuccessorResult::Ambiguous(unique.into_iter().map(PathBuf::from).collect()),
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

/// Resolve the committed rename destination(s) of `old_path` (repo-relative).
///
/// For a path present at HEAD, the follow walk reports its rename rows
/// directly. For a deleted path — the case the Broken routing needs — the
/// follow walk cannot: a pathspec-limited diff renders the rename's old side
/// as a plain deletion, so `git log --follow --diff-filter=R` on a deleted
/// path yields no R rows even with `--full-history`. Instead, ONE repo-wide
/// `git log --diff-filter=RD --name-status --format=%H` (no pathspec — the
/// rename row's old side is the only reliable filter) is parsed client-side,
/// newest commit first, returning the new side of the first rename row whose
/// old side equals the searched path.
fn git_log_follow_renames(repo_root: &Path, old_path: &Path) -> Result<Vec<String>> {
    let path_str = old_path.to_string_lossy();

    if repo_root.join(old_path).exists() {
        // Present path: the follow walk shows rename rows directly.
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
        return Ok(vec![]);
    }

    // Deleted path: one repo-wide log of renames and deletions, newest
    // first. A commit that renamed the searched path away necessarily also
    // deleted it, so the old code's enumeration of deleting commits was a
    // subset of exactly these commits; scanning every commit's rows for an R
    // row whose OLD side equals the path cannot false-match a commit that
    // deletes some other file while renaming Y≠X (old-side equality guards).
    let output = Command::new("git")
        .current_dir(repo_root)
        .args(["log", "--diff-filter=RD", "--name-status", "--format=%H"])
        .output()
        .map_err(|e| miette::miette!("git log failed: {e}"))?;

    if !output.status.success() {
        return Ok(vec![]);
    }

    let text = String::from_utf8_lossy(&output.stdout);
    for (_, rows) in drift::parse_name_status_log(&text) {
        if let Some(row) = rows.iter().find(|r| r.is_rename() && r.old_path == path_str) {
            return Ok(vec![row.new_path.clone()]);
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

// First consumer is the P3 drift-phase implementation (Decision 2 committed
// value reads); remove the allow when it lands.
#[allow(dead_code)]
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

// ── Fix #1 implementation ─────────────────────────────────────────────────────

/// Scan `files` for `broken_link` diagnostics and attempt to rewrite paths whose
/// targets were renamed in git. Returns a `FixPlan` describing what was (or would
/// be) applied.
///
/// When `dry_run` is false, patched content is written back to disk.
///
/// The pass reads git history directly (the drift phase never shells out to
/// a `git span` binary), so fix mode is worktree-only by construction.
#[allow(clippy::too_many_arguments)]
pub fn run_fix_pass(
    files: &[PathBuf],
    repo_root: &Path,
    source: DocSource,
    dry_run: bool,
    content_cache: &mut ContentCache,
    reporter: &crate::cache::CacheReporter,
) -> Result<FixPlan> {
    // Replay pending journals first (plan Decision 8): a previous run killed
    // mid-materialization is completed from its journal before any fresh
    // planning, so planning observes post-replay disk state. Dry runs are
    // side-effect-free: no replay application, no staging.
    let replay_applied = if dry_run {
        HashMap::new()
    } else {
        replay_pending(repo_root)?
    };

    let mut rename_map = RenameMap::build(repo_root)?;

    let mut fixes: Vec<Fix> = Vec::new();
    let mut skipped: Vec<SkippedFix> = Vec::new();
    // file abs path → patched content. Seeded first with the replay's
    // applied contents so every phase below reads post-replay state (the
    // per-run content cache was warmed before the replay rewrote files).
    let mut patches: HashMap<PathBuf, String> = replay_applied.clone();

    for file in files {
        // Prefer a replay-seeded patch as the base (plan Decision 8): the
        // content cache was warmed before journal replay rewrote this file,
        // so the overlay — not the cache — holds current state. Matches the
        // base-selection rule of the later fix phases.
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
            // Line-range links are the drift phase's sole authority (plan
            // Decision 7); its `Broken` routing invokes the rename machinery
            // directly, so the generic Fix #1 loop must not double-handle
            // them.
            if link.start_line.is_some() {
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
                    // Git documented no rename row at all — either the target
                    // was deleted outright or the rename fell below git's
                    // similarity threshold (heavy edits make it look like a
                    // delete-plus-add).
                    skipped.push(SkippedFix {
                        file: file_rel.clone(),
                        line: link.source_line,
                        kind: FixKind::BrokenLinkRename,
                        reason: "no successor in git history — the target was deleted or \
                                 the rename fell below git's similarity threshold"
                            .to_string(),
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

    // ── Drift pass (plan Decision 6) ────────────────────────────────────────────────
    //
    // Classify every line-range link against its page's git-derived anchor
    // epoch and apply the fix-mode actions: relocate `Moved` links (full href
    // rewrite), route `Broken` targets through the rename machinery, skip
    // `Drift`/`Uncertified`/`Unknown` with fail-closed counts, and initialize
    // the `links-reviewed` field on pages that lack one.
    let drift = run_drift_fix_phase(
        files,
        repo_root,
        source,
        content_cache,
        reporter,
        &mut rename_map,
        &mut patches,
        &mut fixes,
        &mut skipped,
    )?;
    let unverified = drift.unverified;
    let certification_skips = drift.certification_skips;

    // Materialize patches to disk unless dry_run (plan Decision 8). A
    // replay-seeded entry the planning phases left untouched describes
    // content already on disk (replay hash-verified it before delivering);
    // dropping those no-ops keeps the journal and `applied_paths` honest.
    if !dry_run && !patches.is_empty() {
        for (path, seeded) in &replay_applied {
            if patches.get(path).map(|p| p == seeded).unwrap_or(false) {
                patches.remove(path);
            }
        }
    }
    if !dry_run && !patches.is_empty() {
        materialize_with_journal(repo_root, &patches)?;
    }

    // `applied_paths`: every patched file, repo-relative, in deterministic
    // order — `--print-applied` prints exactly this list (Decision 7).
    let mut applied_paths: Vec<String> = patches
        .keys()
        .map(|p| {
            p.strip_prefix(repo_root)
                .unwrap_or(p)
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    applied_paths.sort();
    applied_paths.dedup();

    Ok(FixPlan {
        fixes,
        skipped,
        unverified,
        certification_skips,
        applied_paths,
    })
}

/// Outcome of the drift fix phase (plan Decision 6): the fail-closed counts
/// that drive both fix-mode exit gates.
pub(crate) struct DriftFixPhaseOutcome {
    pub(crate) unverified: usize,
    pub(crate) certification_skips: usize,
}

/// Drift fix phase (plan Decision 6): classify every line-range link against
/// its page's git-derived anchor epoch and apply the fix-mode actions —
/// relocate `Moved` links, route `Broken` targets through the rename
/// machinery, skip `Drift`/`Uncertified`/`Unknown` with fail-closed counts,
/// and initialize the `links-reviewed` field on pages that lack one.
///
/// The phase only writes into `patches` in memory; [`run_fix_pass`] is the
/// sole materializer. Href patches carry byte offsets against the page's
/// original content, so they are applied first; the field insertion lands
/// last because it shifts every offset after the YAML block.
#[allow(clippy::too_many_arguments)]
fn run_drift_fix_phase(
    files: &[PathBuf],
    repo_root: &Path,
    source: DocSource,
    content_cache: &mut ContentCache,
    reporter: &crate::cache::CacheReporter,
    rename_map: &mut RenameMap,
    patches: &mut HashMap<PathBuf, String>,
    fixes: &mut Vec<Fix>,
    skipped: &mut Vec<SkippedFix>,
) -> Result<DriftFixPhaseOutcome> {
    // Fix mode is worktree-only by construction: the CLI guard rejects it
    // under a non-worktree source, and `run()` must not mutate files either
    // (pinned by `wiki_check_fix_rejects_non_worktree_source`).
    if source != DocSource::WorkingTree {
        return Ok(DriftFixPhaseOutcome {
            unverified: 0,
            certification_skips: 0,
        });
    }

    // Per-run anchor cache (plan decisions 2, 7, 8): constructed once per
    // phase and threaded through the drift seams; any disabled path
    // (common-dir resolution failure, `WIKI_ANCHOR_CACHE=0`, held init
    // lock, open error) falls back to uncached computation. `reporter`
    // is shared with the post-fix re-check's construction site, so the
    // cache-fault warning fires at most once per run (plan decision 7).
    let anchor_cache = anchor_cache_for_run(reporter);

    let mut unverified = 0;
    let mut certification_skips = 0;
    // Shared across files: the move scan's candidate inventory is loaded once
    // per run, on the first link that needs it.
    let mut ctx = drift::MoveScanCtx::new();

    for file in files {
        // The drift phase runs last (see run_fix_pass), so earlier phases
        // (Fix #1, Fix #3) may already hold a rewritten whole file for this
        // page. Read the patched content when one exists — the earlier
        // phases' href offsets are against that content, and this phase's
        // own offsets must be too, or the drift relocation would clobber
        // their rewrite with a file built from the original bytes.
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
        if !drift::has_line_range_links(&content) {
            continue;
        }
        let file_rel = file
            .strip_prefix(repo_root)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| file.to_string_lossy().into_owned());
        let page_path = file_rel.replace('\\', "/");

        let current_value = drift::read_links_reviewed(&content);
        let committed_value = match DocSource::Head.read(repo_root, &page_path) {
            Ok(Some(head_content)) => drift::read_links_reviewed(&head_content),
            _ => drift::LinksReviewedRead::Readable(None),
        };
        let epoch = drift::find_anchor_commit(
            repo_root,
            anchor_cache.cache(),
            &page_path,
            &current_value,
            &committed_value,
        )
        .map_err(|e| miette::miette!("{e}"))?;

        // A field-less page self-heals under `--fix`: initialize the field,
        // then classify under a pending epoch — certification outcomes are
        // suppressed (the page is under review) while `Broken` still flags.
        // Without a frontmatter block the insert is impossible; the page then
        // falls through with no patch and the post-fix re-check flags it
        // `anchor_epoch_missing`, so nothing is silently passed.
        let (classify_epoch, field_init) = match &epoch {
            drift::LinkEpoch::Missing => (
                drift::LinkEpoch::Current {
                    value: Some("1".to_string()),
                },
                drift::insert_links_reviewed(&content),
            ),
            other => (other.clone(), None),
        };
        let classes = drift::classify_page(
            repo_root,
            anchor_cache.cache(),
            source,
            &page_path,
            &content,
            &classify_epoch,
            &mut ctx,
        )
        .map_err(|e| miette::miette!("{e}"))?;

        let mut file_patches: Vec<(usize, usize, String)> = Vec::new();
        for c in &classes {
            match &c.outcome {
                drift::DriftOutcome::Moved {
                    new_path,
                    new_start,
                    new_end,
                    content_identical,
                } => {
                    // Follow the move: rewrite path and range together as the
                    // full href (round-4 review: never path-only).
                    let fragment = if new_start == new_end {
                        format!("L{new_start}")
                    } else {
                        format!("L{new_start}-L{new_end}")
                    };
                    let new_href = rewrite_href(
                        &c.original_href,
                        Some(&fragment),
                        Path::new(new_path),
                        file,
                        repo_root,
                    );
                    if *content_identical {
                        // Byte-identical relocation: the certified content
                        // moved, and the rewritten href stays certified.
                        fixes.push(Fix {
                            file: file_rel.clone(),
                            line: c.source_line,
                            kind: FixKind::LinkRelocate,
                            byte_start: c.href_byte_start,
                            byte_end: c.href_byte_end,
                            old_href: c.original_href.clone(),
                            new_href: new_href.clone(),
                            reason: format!(
                                "certified content moved to {new_path} lines {new_start}-{new_end}"
                            ),
                            confidence: Confidence::High,
                        });
                    } else {
                        // Honest fuzzy relocation (amendment Change 3): the
                        // destination is a lightly-edited near-copy, not the
                        // certified content. The href still follows the move,
                        // but no "fixed:" line may claim "certified content
                        // moved" — the link needs re-certification, and the
                        // post-fix re-check agrees (it classifies the same
                        // Moved, which is silent, so the skip below is the one
                        // coherent diagnostic driving exit 1).
                        certification_skips += 1;
                        skipped.push(SkippedFix {
                            file: file_rel.clone(),
                            line: c.source_line,
                            kind: FixKind::LinkRelocate,
                            reason: format!(
                                "relocated to {new_path} lines {new_start}-{new_end}, but the \
                                 content there is not byte-identical to the certified block — \
                                 bump `links-reviewed:` after reviewing it"
                            ),
                        });
                    }
                    file_patches.push((c.href_byte_start, c.href_byte_end, new_href));
                }
                drift::DriftOutcome::Broken => {
                    // A renamed-but-unrecognizable target routes through the
                    // rename machinery; the range fragment is preserved
                    // verbatim — only the path part was renamed.
                    match rename_map.successor(Path::new(&c.target_path)) {
                        SuccessorResult::Unique(new_rel) => {
                            let new_abs = repo_root.join(&new_rel);
                            if !new_abs.exists() {
                                skipped.push(SkippedFix {
                                    file: file_rel.clone(),
                                    line: c.source_line,
                                    kind: FixKind::BrokenLinkRename,
                                    reason: format!(
                                        "target deleted; no successor (rename destination {} \
                                         missing)",
                                        new_rel.display()
                                    ),
                                });
                                continue;
                            }
                            let fragment = c
                                .original_href
                                .find('#')
                                .map(|i| &c.original_href[i + 1..]);
                            let new_href = rewrite_href(
                                &c.original_href,
                                fragment,
                                &new_rel,
                                file,
                                repo_root,
                            );
                            fixes.push(Fix {
                                file: file_rel.clone(),
                                line: c.source_line,
                                kind: FixKind::BrokenLinkRename,
                                byte_start: c.href_byte_start,
                                byte_end: c.href_byte_end,
                                old_href: c.original_href.clone(),
                                new_href: new_href.clone(),
                                reason: format!("renamed to {}", new_rel.display()),
                                confidence: Confidence::High,
                            });
                            file_patches.push((c.href_byte_start, c.href_byte_end, new_href));
                        }
                        SuccessorResult::Ambiguous(candidates) => {
                            let names: Vec<String> =
                                candidates.iter().map(|p| p.display().to_string()).collect();
                            skipped.push(SkippedFix {
                                file: file_rel.clone(),
                                line: c.source_line,
                                kind: FixKind::BrokenLinkRename,
                                reason: format!("ambiguous rename candidates: {}", names.join(", ")),
                            });
                        }
                        SuccessorResult::None => {
                            // Git documented no rename row at all — either
                            // the target was deleted outright or the rename
                            // fell below git's similarity threshold (heavy
                            // edits make it look like a delete-plus-add).
                            skipped.push(SkippedFix {
                                file: file_rel.clone(),
                                line: c.source_line,
                                kind: FixKind::BrokenLinkRename,
                                reason: "no successor in git history — the target was \
                                         deleted or the rename fell below git's \
                                         similarity threshold"
                                    .to_string(),
                            });
                        }
                    }
                }
                drift::DriftOutcome::Drift => {
                    // Only a `links-reviewed` bump certifies content edited in
                    // place — `--fix` never settles Drift on its own.
                    certification_skips += 1;
                    skipped.push(SkippedFix {
                        file: file_rel.clone(),
                        line: c.source_line,
                        kind: FixKind::LinkRelocate,
                        reason: format!(
                            "content at `{}#L{}-L{}` changed since the anchor epoch — bump \
                             `links-reviewed:` after reviewing it",
                            c.target_path, c.start_line, c.end_line
                        ),
                    });
                }
                drift::DriftOutcome::RangeDiffered => {
                    // A hand-edited href that the move scan cannot settle is
                    // a reviewer decision, never an auto-fix — same skip
                    // semantics as Drift: bump `links-reviewed:` after
                    // reviewing the link.
                    certification_skips += 1;
                    skipped.push(SkippedFix {
                        file: file_rel.clone(),
                        line: c.source_line,
                        kind: FixKind::LinkRelocate,
                        reason: "the link's range no longer points at the certified block — \
                                 bump `links-reviewed:` after reviewing it"
                            .to_string(),
                    });
                }
                drift::DriftOutcome::Uncertified => {
                    certification_skips += 1;
                    skipped.push(SkippedFix {
                        file: file_rel.clone(),
                        line: c.source_line,
                        kind: FixKind::LinkRelocate,
                        reason: format!(
                            "line-range link `{}` was not present at the page's anchor epoch — \
                             bump `links-reviewed:` after reviewing it",
                            c.original_href
                        ),
                    });
                }
                drift::DriftOutcome::Unknown => {
                    // Ambiguous move: never first-hit-wins.
                    unverified += 1;
                    skipped.push(SkippedFix {
                        file: file_rel.clone(),
                        line: c.source_line,
                        kind: FixKind::LinkRelocate,
                        reason: format!(
                            "could not verify line-range link `{}`: the certified content \
                             occurs at multiple locations — re-point the link and bump \
                             `links-reviewed:`",
                            c.original_href
                        ),
                    });
                }
                drift::DriftOutcome::UnknownLabelDeleted => {
                    // Pairing ambiguity — which epoch record certified the
                    // survivor; content identity could not resolve it. Same
                    // skip semantics as the multi-location Unknown: never
                    // first-hit-wins, page untouched.
                    unverified += 1;
                    skipped.push(SkippedFix {
                        file: file_rel.clone(),
                        line: c.source_line,
                        kind: FixKind::LinkRelocate,
                        reason: format!(
                            "could not verify line-range link `{}`: a duplicate link with \
                             this display text was removed since the last review; the \
                             surviving link cannot be matched to a reviewed record — \
                             re-point it to the block's current location and bump \
                             `links-reviewed:`",
                            c.original_href
                        ),
                    });
                }
                drift::DriftOutcome::Healthy => {}
            }
        }

        if file_patches.is_empty() && field_init.is_none() {
            continue;
        }

        // Href patches first (offsets are against the original content), the
        // field insertion last (it shifts every offset after the YAML block).
        let patched = if file_patches.is_empty() {
            content
        } else {
            let mut sorted = file_patches;
            sorted.sort_by_key(|p| Reverse(p.0));
            let mut patched = content;
            for (start, end, replacement) in sorted {
                patched.replace_range(start..end, &replacement);
            }
            patched
        };
        let final_content = if field_init.is_some() {
            drift::insert_links_reviewed(&patched).unwrap_or(patched)
        } else {
            patched
        };
        patches.insert(file.clone(), final_content);
    }

    Ok(DriftFixPhaseOutcome {
        unverified,
        certification_skips,
    })
}

// ── Fix journals (plan merged-store-generations D8) ──────────────────────────
//
// Crash-recovery for multi-file materialization. The bare `std::fs::write`
// loop this replaces aborted on first error with an arbitrary subset already
// rewritten; a SIGINT mid-loop persisted that subset silently. Every fix run
// now materializes through a private per-worktree journal:
//
//   `<dot-git>/wiki/journal/<scope16>/`
//     blob-0..blob-N   staged target contents (0600, fd-hardened)
//     manifest.json    {version, created_at, status, scope_digest, entries}
//
// Status machine: prepared (staged, manifest written last) → committed (all
// targets written) → delivered (every on-disk file hash-verifies) → journal
// directory removed. Any interruption leaves a `prepared`/`committed`
// journal behind; the NEXT run replays it idempotently BEFORE planning fresh
// patches. Expired (>7-day TTL) or corrupt journals (unparseable manifest,
// stage sha mismatch, scope-digest mismatch) are deleted and the pass
// recomputes cleanly — never partial application.

/// Journal manifest schema version.
const JOURNAL_VERSION: u32 = 1;
/// Time-to-live for unrecovered journals: 7 days in milliseconds.
const JOURNAL_TTL_MS: u64 = 7 * 24 * 60 * 60 * 1000;

/// Lifecycle state of a journal's manifest (`status` field).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum JournalStatus {
    /// Staged targets exist and the manifest is on disk; application has not
    /// been completed (or has not started).
    Prepared,
    /// Every staged target was written to its working-tree path.
    Committed,
    /// Every written file hash-verified against its recorded sha256.
    Delivered,
}

/// One staged target inside a journal manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct JournalEntry {
    /// Repo-relative path of the working-tree file this entry rewrites.
    path_rel: String,
    /// Sibling stage file name holding the target content (`blob-N`).
    stage_file: String,
    /// Lowercase hex sha256 of the full target content.
    sha256: String,
}

/// The `manifest.json` document (schema v1).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct JournalManifest {
    version: u32,
    /// Unix milliseconds at staging time; drives the TTL.
    created_at: u64,
    status: JournalStatus,
    /// Hex sha256 binding the journal to its exact target set and repo.
    scope_digest: String,
    entries: Vec<JournalEntry>,
}

/// Process-wide once-flag: at most one stale-journal warning line per run,
/// mirroring `CacheReporter`'s first-call-wins pattern.
static STALE_JOURNAL_WARNED: AtomicBool = AtomicBool::new(false);

/// Unix milliseconds for TTL stamps and `created_at`.
fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The canonical repository identity bound into every scope digest: the
/// common git dir string. Two worktrees of one repo share it, so identical
/// target sets in either worktree collide onto one journal — safe, because
/// journals live per-worktree while the digest pins the destination set.
fn journal_repo_identity() -> Result<String> {
    let dir = crate::git::common_dir()?;
    Ok(dir.to_string_lossy().into_owned())
}

/// The scope digest: SHA-256 (via [`crate::cache::key::sha256_hex`]) over
/// the length-tagged framing of sorted `(path_rel, target_sha256)` pairs
/// followed by the repository identity string.
fn scope_digest(sorted_pairs: &[(String, String)], identity: &str) -> String {
    let mut fields: Vec<&str> = Vec::with_capacity(sorted_pairs.len() * 2 + 1);
    for (path_rel, sha) in sorted_pairs {
        fields.push(path_rel);
        fields.push(sha);
    }
    fields.push(identity);
    crate::cache::key::sha256_hex(&crate::cache::key::canonical_fields(&fields))
}

/// A journal entry's `path_rel` must name a normal relative path strictly
/// under the repo root. Anything else (absolute, `..`, empty) is corruption
/// — the journal never redirects a write outside the repository.
fn is_safe_journal_rel(path_rel: &str) -> bool {
    if path_rel.is_empty() {
        return false;
    }
    let path = Path::new(path_rel);
    if path.is_absolute() {
        return false;
    }
    path.components()
        .all(|c| matches!(c, Component::Normal(_)))
}

/// A stage file name must be a plain sibling file name (`blob-N`).
fn is_safe_stage_name(stage_file: &str) -> bool {
    !stage_file.is_empty()
        && !stage_file.contains('/')
        && !stage_file.contains('\\')
        && !stage_file.contains('\0')
}

/// Serialize and write `manifest.json` into the retained journal directory:
/// truncate-in-place, write, best-effort fsync. The manifest is written
/// LAST during staging so a crash before it leaves no half-described
/// journal — only stage residue a replay classifies corrupt.
fn write_manifest(dir_fd: &crate::store::fd::DirFd, manifest: &JournalManifest) -> io::Result<()> {
    let bytes = serde_json::to_vec(manifest)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let mut file = dir_fd.create_file("manifest.json")?;
    file.set_len(0)?;
    file.write_all(&bytes)?;
    let _ = file.sync_data();
    Ok(())
}

/// A journal whose manifest parsed and whose every stage blob hash-matches
/// its recorded sha256: ready for idempotent application.
struct LoadedJournal {
    /// `(path_rel, target content)` in manifest-entry order; content is
    /// UTF-8 (validated during classification) and its sha256 equals the
    /// recorded `entries[i].sha256`.
    stages: Vec<(String, String)>,
}

/// Replay verdict for one journal directory.
enum Disposition {
    /// Valid and unexpired: apply idempotently, mark delivered, remove.
    Apply(Box<LoadedJournal>, Box<JournalManifest>),
    /// Expired, unparseable, stage-corrupt, or digest-mismatched: delete
    /// and continue cleanly.
    Stale,
}

/// Classify one journal directory without mutating anything. Every
/// validation failure collapses to [`Disposition::Stale`] — the caller
/// deletes stale directories and reports them through the once-per-run
/// warning budget.
fn classify_journal(dir: &Path, identity: &str, now: u64) -> Disposition {
    let manifest_bytes = match std::fs::read(dir.join("manifest.json")) {
        Ok(b) => b,
        Err(_) => return Disposition::Stale,
    };
    let manifest: JournalManifest = match serde_json::from_slice(&manifest_bytes) {
        Ok(m) => m,
        Err(_) => return Disposition::Stale,
    };
    if manifest.version != JOURNAL_VERSION {
        return Disposition::Stale;
    }
    // TTL: expired journals recompute cleanly regardless of integrity.
    if now.saturating_sub(manifest.created_at) > JOURNAL_TTL_MS {
        return Disposition::Stale;
    }
    if manifest.entries.is_empty() || manifest.scope_digest.len() != 64 {
        return Disposition::Stale;
    }
    for entry in &manifest.entries {
        if !is_safe_journal_rel(&entry.path_rel) || !is_safe_stage_name(&entry.stage_file) {
            return Disposition::Stale;
        }
        let Ok(bytes) = std::fs::read(dir.join(&entry.stage_file)) else {
            return Disposition::Stale;
        };
        if crate::cache::key::sha256_hex(&bytes) != entry.sha256 {
            return Disposition::Stale;
        }
    }
    // Digest mismatch fails toward recompute: the recorded digest must be
    // exactly what the manifest's own entries plus the repo identity derive.
    let mut sorted: Vec<(String, String)> = manifest
        .entries
        .iter()
        .map(|e| (e.path_rel.clone(), e.sha256.clone()))
        .collect();
    sorted.sort();
    if scope_digest(&sorted, identity) != manifest.scope_digest {
        return Disposition::Stale;
    }
    let mut stages = Vec::with_capacity(manifest.entries.len());
    for entry in &manifest.entries {
        let bytes = std::fs::read(dir.join(&entry.stage_file)).expect("stage read verified above");
        let Ok(content) = String::from_utf8(bytes) else {
            return Disposition::Stale;
        };
        stages.push((entry.path_rel.clone(), content));
    }
    Disposition::Apply(
        Box::new(LoadedJournal { stages }),
        Box::new(manifest),
    )
}

/// Apply one valid journal idempotently: skip targets whose current bytes
/// already hash to the recorded sha256, write the rest, mark delivered,
/// remove the directory. Any write failure propagates — fail closed, the
/// journal stays for the next replay.
fn replay_apply_journal(
    dir: &Path,
    repo_root: &Path,
    stages: &[(String, String)],
    manifest: JournalManifest,
) -> Result<()> {
    for (path_rel, content) in stages {
        let target = repo_root.join(path_rel);
        let content_sha = crate::cache::key::sha256_hex(content.as_bytes());
        let already_applied = std::fs::read(&target)
            .map(|cur| crate::cache::key::sha256_hex(&cur) == content_sha)
            .unwrap_or(false);
        if !already_applied {
            std::fs::write(&target, content).map_err(|e| {
                miette::miette!(
                    "fix journal replay failed to write {}: {e}; journal kept for replay",
                    target.display()
                )
            })?;
        }
    }
    // Delivered: the state machine's terminal record before removal. A kill
    // between this write and the directory removal leaves a delivered
    // journal that replays as pure idempotent skips next run.
    let mut delivered = manifest;
    delivered.status = JournalStatus::Delivered;
    let dir_fd = crate::store::fd::DirFd::open(dir)
        .map_err(|e| miette::miette!("fix journal reopen failed: {e}"))?;
    write_manifest(&dir_fd, &delivered).map_err(|e| {
        miette::miette!("fix journal delivery stamp failed: {e}; journal kept for replay")
    })?;
    std::fs::remove_dir_all(dir)
        .map_err(|e| miette::miette!("fix journal cleanup failed: {e}"))?;
    Ok(())
}

/// Replay any pending journals under `<dot-git>/wiki/journal` BEFORE fresh
/// patches are planned. Valid, unexpired journals are applied idempotently
/// (targets whose current bytes already hash to the recorded sha256 are
/// skipped), marked delivered, and removed. Expired or corrupt journals are
/// deleted and reported through at most one stderr warning line per process.
///
/// Returns the applied `(absolute path → final content)` pairs so the caller
/// can seed its planning overlay — planning must observe post-replay disk
/// state, not the pre-replay bytes its content cache warmed.
pub(crate) fn replay_pending(repo_root: &Path) -> Result<HashMap<PathBuf, String>> {
    let Some(dot_git) = crate::index::find_dot_git(repo_root) else {
        return Ok(HashMap::new());
    };
    let journal_root = dot_git.join("wiki").join("journal");
    let Ok(readings) = std::fs::read_dir(&journal_root) else {
        return Ok(HashMap::new());
    };
    let mut dirs: Vec<PathBuf> = readings
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();

    // Without the repository identity the scope digest cannot be verified;
    // leave every journal untouched rather than discard recovery data on a
    // technicality. The next run with resolvable identity settles them.
    let Ok(identity_dir) = crate::git::common_dir() else {
        return Ok(HashMap::new());
    };
    let identity = identity_dir.to_string_lossy().into_owned();
    let now = unix_ms();

    let mut discarded = 0usize;
    let mut applied: HashMap<PathBuf, String> = HashMap::new();

    for dir in dirs {
        match classify_journal(&dir, &identity, now) {
            Disposition::Stale => {
                std::fs::remove_dir_all(&dir)
                    .map_err(|e| miette::miette!("stale fix journal removal failed: {e}"))?;
                discarded += 1;
            }
            Disposition::Apply(loaded, manifest) => {
                let LoadedJournal { stages } = *loaded;
                replay_apply_journal(&dir, repo_root, &stages, *manifest)?;
                for (path_rel, content) in &stages {
                    applied.insert(repo_root.join(path_rel), content.clone());
                }
            }
        }
    }

    if discarded > 0 && !STALE_JOURNAL_WARNED.swap(true, Ordering::Relaxed) {
        eprintln!(
            "warning: {discarded} stale fix journal(s) discarded; recomputing cleanly"
        );
    }
    Ok(applied)
}

/// Materialize `patches` through a crash-recovery journal: stage every
/// target, write the manifest last (prepared), apply all writes, mark
/// committed, verify every on-disk hash, mark delivered, remove the
/// directory. Any failure leaves the journal in place for the next run's
/// replay — fail closed, never partial application.
fn materialize_with_journal(repo_root: &Path, patches: &HashMap<PathBuf, String>) -> Result<()> {
    // Deterministic stage order: sorted (path_rel, sha256) pairs feed both
    // the scope digest and the `blob-N` numbering.
    let mut staged: Vec<(String, String, Vec<u8>)> = patches
        .iter()
        .map(|(abs, content)| {
            let path_rel = abs
                .strip_prefix(repo_root)
                .unwrap_or(abs)
                .to_string_lossy()
                .replace('\\', "/");
            (
                path_rel,
                crate::cache::key::sha256_hex(content.as_bytes()),
                content.as_bytes().to_vec(),
            )
        })
        .collect();
    staged.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));

    let dot_git = crate::index::find_dot_git(repo_root).ok_or_else(|| {
        miette::miette!("cannot locate the git directory for fix journals")
    })?;
    let identity = journal_repo_identity()?;
    let pairs: Vec<(String, String)> = staged
        .iter()
        .map(|(p, s, _)| (p.clone(), s.clone()))
        .collect();
    let digest = scope_digest(&pairs, &identity);
    let scope16 = &digest[..16];

    // The journal subtree is created exclusively through fd-hardened
    // helpers: private 0700 components, symlink refusal on descent.
    let journal_rel = Path::new("wiki").join("journal");
    let git_fd = crate::store::fd::DirFd::open(&dot_git)
        .map_err(|e| miette::miette!("fix journal root unusable: {e}"))?;
    let journal_fd = git_fd
        .ensure_private_subtree(&journal_rel)
        .map_err(|e| miette::miette!("fix journal subtree unusable: {e}"))?;
    let scope_abs = dot_git.join(&journal_rel).join(scope16);
    if scope_abs.exists() {
        // Replay ran before staging, so a surviving same-scope journal means
        // something anomalous — refuse rather than overwrite recovery data.
        return Err(miette::miette!(
            "fix journal {} already exists; refusing to stage over it",
            scope_abs.display()
        ));
    }
    let scope_fd = journal_fd
        .ensure_private_subtree(Path::new(scope16))
        .map_err(|e| miette::miette!("fix journal scope dir unusable: {e}"))?;

    // Stage targets first; the manifest lands last (prepared).
    for (i, (_, _, bytes)) in staged.iter().enumerate() {
        let name = format!("blob-{i}");
        let mut file = scope_fd
            .create_file(&name)
            .map_err(|e| miette::miette!("fix journal stage {name} failed: {e}"))?;
        file.set_len(0)
            .map_err(|e| miette::miette!("fix journal stage {name} truncate failed: {e}"))?;
        file.write_all(bytes)
            .map_err(|e| miette::miette!("fix journal stage {name} write failed: {e}"))?;
        let _ = file.sync_data();
    }
    let manifest = JournalManifest {
        version: JOURNAL_VERSION,
        created_at: unix_ms(),
        status: JournalStatus::Prepared,
        scope_digest: digest,
        entries: staged
            .iter()
            .enumerate()
            .map(|(i, (path_rel, sha256, _))| JournalEntry {
                path_rel: path_rel.clone(),
                stage_file: format!("blob-{i}"),
                sha256: sha256.clone(),
            })
            .collect(),
    };
    write_manifest(&scope_fd, &manifest)
        .map_err(|e| miette::miette!("fix journal manifest write failed: {e}"))?;

    // Read back and hash-verify every stage BEFORE touching any target, so
    // corrupted stages can never reach working files.
    let mut stage_bytes = Vec::with_capacity(manifest.entries.len());
    for entry in &manifest.entries {
        let bytes = std::fs::read(scope_abs.join(&entry.stage_file)).map_err(|e| {
            miette::miette!(
                "fix journal stage {} unreadable: {e}; journal kept for replay",
                entry.stage_file
            )
        })?;
        if crate::cache::key::sha256_hex(&bytes) != entry.sha256 {
            return Err(miette::miette!(
                "fix journal stage {} failed integrity; journal kept for replay",
                entry.stage_file
            ));
        }
        stage_bytes.push(bytes);
    }

    // Apply: attempt EVERY write, then fail closed on any error — the
    // journal stays prepared, and the next run replays deterministically.
    let mut first_failure: Option<(String, std::io::Error)> = None;
    for (i, entry) in manifest.entries.iter().enumerate() {
        let target = repo_root.join(&entry.path_rel);
        if let Err(e) = std::fs::write(&target, &stage_bytes[i])
            && first_failure.is_none()
        {
            first_failure = Some((entry.path_rel.clone(), e));
        }
    }
    if let Some((path_rel, e)) = first_failure {
        return Err(miette::miette!(
            "failed to write {path_rel}: {e}; fix journal kept for replay"
        ));
    }

    // Committed: every staged target reached disk.
    let mut committed = manifest.clone();
    committed.status = JournalStatus::Committed;
    write_manifest(&scope_fd, &committed)
        .map_err(|e| miette::miette!("fix journal commit stamp failed: {e}"))?;

    // Verify: every on-disk file must hash-match its recorded sha256. Any
    // mismatch errors out fail-closed with the journal retained for replay.
    for entry in &manifest.entries {
        let current = std::fs::read(repo_root.join(&entry.path_rel)).map_err(|e| {
            miette::miette!(
                "fix journal verification unreadable {}: {e}; journal kept for replay",
                entry.path_rel
            )
        })?;
        if crate::cache::key::sha256_hex(&current) != entry.sha256 {
            return Err(miette::miette!(
                "fix journal verification failed for {}; journal kept for replay",
                entry.path_rel
            ));
        }
    }

    // Delivered, then remove: clean delivery leaves no journal behind.
    let mut delivered = committed;
    delivered.status = JournalStatus::Delivered;
    write_manifest(&scope_fd, &delivered)
        .map_err(|e| miette::miette!("fix journal delivery stamp failed: {e}"))?;
    std::fs::remove_dir_all(&scope_abs)
        .map_err(|e| miette::miette!("fix journal cleanup failed: {e}"))?;
    Ok(())
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

    /// A wiki page carrying `links-reviewed: 1` — a certified page, so the
    /// drift engine anchors at its commit instead of taking the field-init
    /// path.
    fn certified_wiki_page(title: &str, body: &str) -> String {
        format!(
            "---\ntitle: {title}\nsummary: A page about {title}.\nlinks-reviewed: 1\n---\n{body}"
        )
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
        let reporter = crate::cache::CacheReporter::default();
        let plan = run_fix_pass(
            &[source.clone(), target.clone()],
            repo.path(),
            crate::index::DocSource::WorkingTree,
            /* dry_run */ true,
            &mut ContentCache::new(),
            &reporter,
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
        let reporter = crate::cache::CacheReporter::default();
        let plan = run_fix_pass(
            &[source.clone(), target.clone()],
            repo.path(),
            crate::index::DocSource::WorkingTree,
            /* dry_run */ true,
            &mut ContentCache::new(),
            &reporter,
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

    // ── git_log_follow_renames: deleted-path lookup (P4) ─────────────────────

    /// A committed rename renders the old path as deleted in the worktree;
    /// the deleted-path lookup must resolve its successor through the R row
    /// of the renaming commit.
    #[test]
    fn deleted_path_resolves_successor_through_rename() {
        let repo = TestRepo::new();
        repo.write("src/a.md", "content\n");
        repo.commit("create a.md");
        repo.git(&["mv", "src/a.md", "src/b.md"]);
        repo.commit("rename a.md to b.md");

        // Later unrelated history must not disturb the resolution.
        repo.write("other.txt", "noise\n");
        repo.commit("unrelated");

        let got = git_log_follow_renames(repo.path(), Path::new("src/a.md")).expect("lookup");
        assert_eq!(got, vec!["src/b.md".to_string()]);
    }

    /// A commit that deletes X outright while renaming Y→Z in the SAME
    /// commit must not produce a false match for X — old-side equality is
    /// the guard. An older commit's genuine rename of X is still found
    /// through newer non-matching commits (newest-first scan).
    #[test]
    fn deleted_path_scan_is_not_confused_by_unrelated_renames() {
        let repo = TestRepo::new();
        repo.write("src/x.md", "x\n");
        repo.write("src/y.md", "y\n");
        repo.commit("create x.md and y.md");
        // Genuine rename of X one step back in history.
        repo.git(&["mv", "src/x.md", "src/x2.md"]);
        repo.commit("rename x.md to x2.md");
        repo.git(&["mv", "src/x2.md", "src/final.md"]);
        repo.commit("rename x2.md to final.md");
        // Newest commit: deletes Y outright while renaming an unrelated file.
        repo.write("src/w.md", "w\n");
        repo.commit("add w");
        repo.git(&["rm", "src/y.md"]);
        repo.git(&["mv", "src/w.md", "src/v.md"]);
        repo.commit("delete y.md and rename w.md to v.md");

        let y = git_log_follow_renames(repo.path(), Path::new("src/y.md")).expect("lookup y");
        assert!(
            y.is_empty(),
            "an outright deletion has no successor; got {y:?} — \
             the R(w→v) row of the deleting commit must not match"
        );
        let x = git_log_follow_renames(repo.path(), Path::new("src/x.md")).expect("lookup x");
        assert_eq!(
            x,
            vec!["src/x2.md".to_string()],
            "the FIRST (newest) matching rename wins, skipping newer commits \
             that do not touch x.md"
        );
    }

    /// Multi-hop renames resolve to the FIRST hop here — chaining to the
    /// terminal destination is `RenameMap`'s job (`chain_ends`).
    #[test]
    fn deleted_path_returns_first_hop_only() {
        let repo = TestRepo::new();
        repo.write("docs/a.md", "a\n");
        repo.commit("create a.md");
        repo.git(&["mv", "docs/a.md", "docs/b.md"]);
        repo.commit("a -> b");
        repo.git(&["mv", "docs/b.md", "docs/c.md"]);
        repo.commit("b -> c");

        let got =
            git_log_follow_renames(repo.path(), Path::new("docs/a.md")).expect("lookup");
        assert_eq!(got, vec!["docs/b.md".to_string()]);
    }

    /// The present-path branch is unchanged: it returns the NEW side of the
    /// most recent rename row of the pathspec-limited follow walk — for a
    /// file renamed INTO its current name that is the path itself; a never-
    /// renamed file has no rows and resolves to no successor here (chaining
    /// and map layers live in `RenameMap`).
    #[test]
    fn present_path_follows_newest_rename_row() {
        let repo = TestRepo::new();
        repo.write("docs/old.md", "old\n");
        repo.commit("create old.md");
        repo.git(&["mv", "docs/old.md", "docs/newer.md"]);
        repo.commit("old -> newer");
        repo.write("unrelated.txt", "u\n");
        repo.commit("unrelated");

        let got =
            git_log_follow_renames(repo.path(), Path::new("docs/newer.md")).expect("lookup");
        assert_eq!(got, vec!["docs/newer.md".to_string()]);

        let none =
            git_log_follow_renames(repo.path(), Path::new("unrelated.txt")).expect("lookup");
        assert!(none.is_empty());
    }

    // ── Drift fix phase (plan Decision 6) ──────────────────────────────────────
    //
    // The P3 acceptance checks for `run_drift_fix_phase` — written as P2
    // skipped checks, unignored one at a time as the implementation landed.
    // The phase writes into `patches` in memory; `run_fix_pass` materializes
    // to disk.

    /// Certified block used by the drift-fix fixtures: distinctive enough
    /// that an edit to it can never rk64-collide with another file's content.
    const BLOCK: &str = "fn canonical() {\n    compute()\n    resolve()\n}\n";

    /// An emptied target that keeps four lines: the certified range L2-L4
    /// still fits, so a cross-file move reaches the move scan instead of
    /// classifying Broken on the extent check.
    const EMPTIED: &str = "// emptied\n// emptied\n// emptied\n// emptied\n";

    /// Fixture: a repo whose committed HEAD carries `wiki/page.md` with
    /// `links-reviewed: 1` and one line-range link to `src/target.rs` lines
    /// 2-4, where the certified block sits. Returns the repo and the page's
    /// absolute path.
    fn certified_page_repo() -> (TestRepo, PathBuf) {
        let repo = TestRepo::new();
        repo.write("src/target.rs", &format!("// preamble\n{BLOCK}"));
        repo.write(
            "wiki/page.md",
            &certified_wiki_page("Page", "See [target](../src/target.rs#L2-L4)."),
        );
        repo.commit("certified baseline");
        let page = repo.path().join("wiki/page.md");
        (repo, page)
    }

    /// Run the drift fix phase over `files` and return
    /// `(outcome, fixes, skipped, patches)`.
    fn drift_phase(
        repo: &TestRepo,
        files: &[PathBuf],
    ) -> (
        DriftFixPhaseOutcome,
        Vec<Fix>,
        Vec<SkippedFix>,
        HashMap<PathBuf, String>,
    ) {
        let mut rename_map = RenameMap::build(repo.path()).expect("rename map");
        let mut patches: HashMap<PathBuf, String> = HashMap::new();
        let mut fixes: Vec<Fix> = Vec::new();
        let mut skipped: Vec<SkippedFix> = Vec::new();
        let reporter = crate::cache::CacheReporter::default();
        let outcome = run_drift_fix_phase(
            files,
            repo.path(),
            crate::index::DocSource::WorkingTree,
            &mut ContentCache::new(),
            &reporter,
            &mut rename_map,
            &mut patches,
            &mut fixes,
            &mut skipped,
        )
        .expect("drift fix phase");
        (outcome, fixes, skipped, patches)
    }

    /// Moved same-file: the block shifts down one line; the phase rewrites
    /// the full href (same path, new range) as a `LinkRelocate` fix.
    #[test]
    fn drift_fix_relocates_moved_same_file() {
        let (repo, page) = certified_page_repo();
        repo.write(
            "src/target.rs",
            &format!("// preamble\n// shifted\n{BLOCK}"),
        );
        repo.commit("shift block");

        let (outcome, fixes, skipped, patches) = drift_phase(&repo, std::slice::from_ref(&page));
        assert_eq!(outcome.unverified, 0);
        assert_eq!(outcome.certification_skips, 0);
        assert_eq!(skipped.len(), 0, "nothing to skip: {skipped:?}");
        assert_eq!(fixes.len(), 1, "one relocation: {fixes:?}");
        let fix = &fixes[0];
        assert!(matches!(fix.kind, FixKind::LinkRelocate));
        assert_eq!(fix.old_href, "../src/target.rs#L2-L4");
        assert_eq!(fix.new_href, "../src/target.rs#L3-L5");
        assert_eq!(fix.file, "wiki/page.md");
        let patched = patches.get(&page).expect("page patched");
        assert!(
            patched.contains("../src/target.rs#L3-L5"),
            "patch must carry the relocated href: {patched}"
        );
    }

    /// Moved cross-file: the block disappears from the target and reappears
    /// in the file that succeeds it via rename history; the phase rewrites
    /// path and range together. The rename row is the identity evidence the
    /// cross-file tier requires — a content-only copy in an unrelated file
    /// must never win (Change 2 of the relocation amendment).
    #[test]
    fn drift_fix_relocates_moved_cross_file() {
        let (repo, page) = certified_page_repo();
        repo.git(&["mv", "src/target.rs", "src/moved.rs"]);
        repo.write("src/moved.rs", &format!("// preamble\n// x\n{BLOCK}"));
        repo.commit("move block cross-file");

        let (outcome, fixes, skipped, _) = drift_phase(&repo, std::slice::from_ref(&page));
        assert_eq!(outcome.unverified, 0);
        assert_eq!(outcome.certification_skips, 0);
        assert_eq!(skipped.len(), 0);
        assert_eq!(fixes.len(), 1, "one relocation: {fixes:?}");
        let fix = &fixes[0];
        assert!(matches!(fix.kind, FixKind::LinkRelocate));
        assert_eq!(fix.old_href, "../src/target.rs#L2-L4");
        assert_eq!(fix.new_href, "../src/moved.rs#L3-L5");
    }

    /// A staged rename with intact content classifies `Moved` (the move scan
    /// finds the block at the renamed path) and relocates the full href.
    #[test]
        fn drift_fix_renamed_target_relocates_via_move_scan() {
        let repo = TestRepo::new();
        repo.write("src/old.rs", BLOCK);
        repo.write(
            "wiki/page.md",
            &certified_wiki_page("Page", "See [old](../src/old.rs#L1-L4)."),
        );
        repo.commit("certified baseline");
        repo.git(&["mv", "src/old.rs", "src/new.rs"]);

        let (_, fixes, _, _) = drift_phase(&repo, &[repo.path().join("wiki/page.md")]);
        assert_eq!(fixes.len(), 1, "one relocation: {fixes:?}");
        let fix = &fixes[0];
        assert!(matches!(fix.kind, FixKind::LinkRelocate));
        assert_eq!(fix.old_href, "../src/old.rs#L1-L4");
        assert_eq!(fix.new_href, "../src/new.rs#L1-L4");
    }

    /// A renamed target whose content was edited beyond move-scan
    /// recognition classifies `Broken` and routes through the rename
    /// machinery: a unique successor applies as a `BrokenLinkRename` fix
    /// that preserves the range fragment verbatim.
    #[test]
        fn drift_fix_broken_routes_through_rename_map() {
        let repo = TestRepo::new();
        repo.write("src/old.rs", BLOCK);
        repo.write(
            "wiki/page.md",
            &certified_wiki_page("Page", "See [old](../src/old.rs#L1-L4)."),
        );
        repo.commit("certified baseline");
        repo.git(&["mv", "src/old.rs", "src/new.rs"]);
        repo.write("src/new.rs", "fn unrecognizable() {}\n");

        let (outcome, fixes, skipped, _) = drift_phase(&repo, &[repo.path().join("wiki/page.md")]);
        assert_eq!(outcome.unverified, 0);
        assert_eq!(outcome.certification_skips, 0);
        assert_eq!(fixes.len(), 1, "one rename fix: {fixes:?}");
        let fix = &fixes[0];
        assert!(matches!(fix.kind, FixKind::BrokenLinkRename));
        assert_eq!(fix.old_href, "../src/old.rs#L1-L4");
        assert_eq!(fix.new_href, "../src/new.rs#L1-L4");
        assert_eq!(skipped.len(), 0);
    }

    /// Broken with no rename successor stays a skipped `Broken` — fail-closed,
    /// no invented destination.
    #[test]
        fn drift_fix_broken_no_successor_is_skipped() {
        let repo = TestRepo::new();
        repo.write("src/old.rs", BLOCK);
        repo.write(
            "wiki/page.md",
            &certified_wiki_page("Page", "See [old](../src/old.rs#L1-L4)."),
        );
        repo.commit("certified baseline");
        repo.git(&["rm", "src/old.rs"]);
        repo.commit("delete target");

        let (_, fixes, skipped, _) = drift_phase(&repo, &[repo.path().join("wiki/page.md")]);
        assert_eq!(fixes.len(), 0, "no invented fix: {fixes:?}");
        assert_eq!(skipped.len(), 1, "one broken skip: {skipped:?}");
        assert!(matches!(skipped[0].kind, FixKind::BrokenLinkRename));
    }

    /// Drift (content edited in place, no move match) skips with the
    /// `links-reviewed:` bump remedy and counts into `certification_skips`.
    #[test]
        fn drift_fix_skips_drift_with_bump_remedy() {
        let (repo, page) = certified_page_repo();
        repo.write(
            "src/target.rs",
            "// preamble\nfn canonical() {\n    recompute()\n    resolve()\n}\n",
        );
        repo.commit("edit block in place");

        let (outcome, fixes, skipped, _) = drift_phase(&repo, std::slice::from_ref(&page));
        assert_eq!(fixes.len(), 0);
        assert_eq!(outcome.certification_skips, 1);
        assert_eq!(outcome.unverified, 0);
        assert_eq!(skipped.len(), 1, "one drift skip: {skipped:?}");
        assert!(
            skipped[0].reason.contains("links-reviewed"),
            "skip remedy must name the field: {skipped:?}"
        );
    }

    /// Uncertified (a link added since the anchor commit) skips with the
    /// bump remedy and counts into `certification_skips`.
    #[test]
        fn drift_fix_skips_uncertified_with_bump_remedy() {
        let (repo, page) = certified_page_repo();
        repo.write("src/extra.rs", "fn extra() {}\n");
        let mut content = std::fs::read_to_string(&page).expect("read page");
        content.push_str("See also [extra](../src/extra.rs#L1-L1).\n");
        std::fs::write(&page, content).expect("write page");

        let (outcome, fixes, skipped, _) = drift_phase(&repo, std::slice::from_ref(&page));
        assert_eq!(fixes.len(), 0);
        assert_eq!(outcome.certification_skips, 1);
        assert_eq!(outcome.unverified, 0);
        assert_eq!(skipped.len(), 1, "one uncertified skip: {skipped:?}");
        assert!(
            skipped[0].reason.contains("links-reviewed"),
            "skip remedy must name the field: {skipped:?}"
        );
    }

    /// Unknown (the certified content occurs at ≥2 locations) skips and
    /// counts into `unverified` — never first-hit-wins. The two extra
    /// windows sit in the target itself: as separate files they would carry
    /// no identity evidence and never reach Unknown (Change 2).
    #[test]
        fn drift_fix_skips_unknown_with_unverified_count() {
        let (repo, page) = certified_page_repo();
        repo.write(
            "src/target.rs",
            &format!("// preamble\n{EMPTIED}\n// gap\n{BLOCK}\n// gap\n{BLOCK}"),
        );
        repo.commit("duplicate block in place, certified window gone");

        let (outcome, fixes, skipped, _) = drift_phase(&repo, std::slice::from_ref(&page));
        assert_eq!(fixes.len(), 0, "ambiguous: never auto-fix: {fixes:?}");
        assert_eq!(outcome.unverified, 1);
        assert_eq!(outcome.certification_skips, 0);
        assert_eq!(skipped.len(), 1, "one unknown skip: {skipped:?}");
    }

    /// UnknownLabelDeleted (a duplicate with this display text was removed
    /// since the epoch and content identity could not resolve the pairing)
    /// skips with the same unverified count — the reason names the pairing
    /// ambiguity, never the multi-location text.
    #[test]
    fn drift_fix_skips_label_deleted_with_honest_reason() {
        let repo = TestRepo::new();
        repo.write(
            "src/target.rs",
            "fn alpha() {\n    a()\n}\n\nfn beta() {\n    b()\n}\n",
        );
        repo.write(
            "wiki/page.md",
            &certified_wiki_page(
                "Page",
                "See [target](../src/target.rs#L1-L3) and [target](../src/target.rs#L5-L7).",
            ),
        );
        repo.commit("certify two same-display-text links");
        // The first link is deleted; the survivor is re-pointed to content
        // matching no candidate's certified block.
        repo.write(
            "src/target.rs",
            "fn alpha() {\n    a()\n}\n\nfn beta() {\n    b()\n}\n\nfn gamma() {\n    g()\n}\n",
        );
        repo.write(
            "wiki/page.md",
            &certified_wiki_page("Page", "See [target](../src/target.rs#L9-L11)."),
        );
        repo.commit("delete duplicate, re-point to uncertified content");

        let page = repo.path().join("wiki/page.md");
        let (outcome, fixes, skipped, patches) = drift_phase(&repo, std::slice::from_ref(&page));
        assert_eq!(fixes.len(), 0, "pairing ambiguity: never auto-fix: {fixes:?}");
        assert_eq!(outcome.unverified, 1);
        assert_eq!(outcome.certification_skips, 0);
        assert_eq!(skipped.len(), 1, "one skip: {skipped:?}");
        assert!(
            matches!(skipped[0].kind, FixKind::LinkRelocate),
            "skip kind must stay LinkRelocate: {skipped:?}"
        );
        assert!(
            skipped[0]
                .reason
                .contains("duplicate link with this display text was removed"),
            "reason must name the deleted duplicate: {skipped:?}"
        );
        assert!(
            !skipped[0].reason.contains("occurs at multiple locations"),
            "reason must not claim a multi-location ambiguity: {skipped:?}"
        );
        assert!(
            patches.is_empty(),
            "page must be byte-untouched: {patches:?}"
        );
    }

    /// A page with range links and no field anywhere gets `links-reviewed: 1`
    /// initialized in the patch; the pending field suppresses certification
    /// outcomes, and a second run produces no patch (never rewrites the
    /// value).
    #[test]
        fn drift_fix_initializes_field_once() {
        let repo = TestRepo::new();
        repo.write("src/target.rs", &format!("// preamble\n{BLOCK}"));
        repo.write(
            "wiki/page.md",
            &wiki_page("Page", "See [target](../src/target.rs#L2-L4)."),
        );
        repo.commit("field-less page");

        let page = repo.path().join("wiki/page.md");
        let (outcome, fixes, skipped, patches) = drift_phase(&repo, std::slice::from_ref(&page));
        assert_eq!(outcome.unverified, 0);
        assert_eq!(outcome.certification_skips, 0);
        assert_eq!(fixes.len(), 0);
        assert_eq!(skipped.len(), 0);
        let patched = patches.get(&page).expect("field-initialized patch");
        assert!(
            patched.contains("links-reviewed: 1"),
            "field must be initialized: {patched}"
        );
        // Materialize the first run's patch (run_fix_pass would have written
        // it) so the second run sees the field in the worktree.
        std::fs::write(&page, patched).expect("materialize patch");

        // Second run: the field exists in the worktree (committed side still
        // absent) → pending-bump epoch → no patch, no counts.
        let (outcome2, fixes2, skipped2, patches2) = drift_phase(&repo, std::slice::from_ref(&page));
        assert_eq!(outcome2.unverified, 0);
        assert_eq!(outcome2.certification_skips, 0);
        assert_eq!(fixes2.len(), 0);
        assert_eq!(skipped2.len(), 0);
        assert!(
            !patches2.contains_key(&page),
            "second run must not rewrite the field"
        );
    }

    /// A pending bump (current field differs from the newest committed
    /// value) suppresses certification outcomes: drifted content produces
    /// no skips, no counts, and no patch.
    #[test]
        fn drift_fix_pending_bump_suppresses_certification_work() {
        let (repo, page) = certified_page_repo();
        repo.write(
            "src/target.rs",
            "// preamble\nfn canonical() {\n    recompute()\n    resolve()\n}\n",
        );
        repo.commit("edit block in place");
        // Bump the field in the worktree only — a deliberate re-certification
        // in progress.
        let mut content = std::fs::read_to_string(&page).expect("read page");
        content = content.replace("links-reviewed: 1", "links-reviewed: 2");
        std::fs::write(&page, content).expect("write page");

        let (outcome, fixes, skipped, patches) = drift_phase(&repo, std::slice::from_ref(&page));
        assert_eq!(outcome.unverified, 0);
        assert_eq!(outcome.certification_skips, 0);
        assert_eq!(fixes.len(), 0);
        assert_eq!(skipped.len(), 0, "pending bump suppresses Drift: {skipped:?}");
        assert!(
            !patches.contains_key(&page),
            "the field value is never rewritten"
        );
    }
}
