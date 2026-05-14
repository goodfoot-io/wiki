use std::cmp::Reverse;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use miette::Result;
use serde::Serialize;

use crate::index::DocSource;
use crate::parser::{parse_fragment_links, LinkKind};

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
#[derive(Debug)]
pub struct FixPlan {
    pub fixes: Vec<Fix>,
    pub skipped: Vec<SkippedFix>,
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
                            // Replace dest with f if dest is not the final destination.
                            if !entry.contains(&f) {
                                entry.push(f);
                                changed = true;
                            }
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
pub fn read_at_baseline(
    path: &Path,
    repo_root: &Path,
) -> Result<Option<(String, &'static str)>> {
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
        .args(["log", "--diff-filter=R", "--follow", "--format=%H", "--", &path_rel])
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

// ── Fix #1 implementation ─────────────────────────────────────────────────────

/// Scan `files` for `broken_link` diagnostics and attempt to rewrite paths whose
/// targets were renamed in git. Returns a `FixPlan` describing what was (or would
/// be) applied.
///
/// When `dry_run` is false, patched content is written back to disk.
pub fn run_fix_pass(
    files: &[PathBuf],
    repo_root: &Path,
    dry_run: bool,
) -> Result<FixPlan> {
    let mut rename_map = RenameMap::build(repo_root)?;

    let mut fixes: Vec<Fix> = Vec::new();
    let mut skipped: Vec<SkippedFix> = Vec::new();
    // file abs path → patched content
    let mut patches: HashMap<PathBuf, String> = HashMap::new();

    for file in files {
        let content = match std::fs::read_to_string(file) {
            Ok(c) => c,
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
            let is_explicit = matches!(
                first,
                Some(Component::CurDir) | Some(Component::ParentDir)
            );
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

                    let fragment = link.original_href.find('#').map(|i| &link.original_href[i + 1..]);
                    let new_href = rewrite_href(&link.original_href, fragment, &new_rel, file, repo_root);

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

    // Materialize patches to disk unless dry_run.
    if !dry_run {
        for (path, content) in &patches {
            std::fs::write(path, content)
                .map_err(|e| miette::miette!("failed to write {}: {e}", path.display()))?;
        }
    }

    Ok(FixPlan { fixes, skipped })
}
