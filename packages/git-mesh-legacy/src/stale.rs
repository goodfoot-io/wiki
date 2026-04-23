use crate::git::git_stdout;
use crate::mesh::read::read_mesh;
use crate::types::*;
use anyhow::Result;
use similar::{ChangeTag, TextDiff};
use std::path::Path;

pub fn stale_mesh(repo: &gix::Repository, name: &str) -> Result<MeshResolved> {
    let work_dir = repo
        .workdir()
        .ok_or_else(|| anyhow::anyhow!("Bare repositories are not supported"))?;
    let mesh = read_mesh(repo, name)?;
    let mut links = Vec::with_capacity(mesh.links.len());

    for stored_link in mesh.links {
        let link_id = stored_link.id;
        let anchor_reachable = is_commit_reachable_from_any_ref(work_dir, &stored_link.anchor_sha)?;
        let sides = if anchor_reachable {
            [
                resolve_side(work_dir, &stored_link.anchor_sha, &stored_link.sides[0])?,
                resolve_side(work_dir, &stored_link.anchor_sha, &stored_link.sides[1])?,
            ]
        } else {
            [
                orphaned_side(&stored_link.sides[0]),
                orphaned_side(&stored_link.sides[1]),
            ]
        };
        let status = overall_status(&sides);
        let reconcile_command = build_reconcile_command(name, &sides);

        links.push(LinkResolved {
            link_id,
            anchor_sha: stored_link.anchor_sha,
            sides,
            status,
            reconcile_command,
        });
    }

    Ok(MeshResolved {
        name: mesh.name,
        message: mesh.message,
        links,
    })
}

fn build_reconcile_command(name: &str, sides: &[SideResolved; 2]) -> String {
    let anchored_pair = format!(
        "{}:{}",
        range_text(
            &sides[0].anchored.path,
            sides[0].anchored.start,
            sides[0].anchored.end,
        ),
        range_text(
            &sides[1].anchored.path,
            sides[1].anchored.start,
            sides[1].anchored.end,
        )
    );

    let mut command = format!("git mesh commit {name} --unlink {anchored_pair}");
    if let (Some(left), Some(right)) = (&sides[0].current, &sides[1].current) {
        let current_pair = format!(
            "{}:{}",
            range_text(&left.path, left.start, left.end),
            range_text(&right.path, right.start, right.end)
        );
        command.push_str(&format!(" --link {current_pair}"));
    }
    command.push_str(" -m \"...\"");
    command
}

fn range_text(path: &str, start: u32, end: u32) -> String {
    format!("{path}#L{start}-L{end}")
}

fn orphaned_side(anchored: &LinkSide) -> SideResolved {
    SideResolved {
        anchored: LinkLocation {
            path: anchored.path.clone(),
            start: anchored.start,
            end: anchored.end,
            blob: anchored.blob.clone(),
        },
        current: None,
        status: LinkStatus::Orphaned,
        culprit: None,
    }
}

fn resolve_side(work_dir: &Path, anchor_sha: &str, anchored: &LinkSide) -> Result<SideResolved> {
    let anchored_location = LinkLocation {
        path: anchored.path.clone(),
        start: anchored.start,
        end: anchored.end,
        blob: anchored.blob.clone(),
    };
    let anchored_bytes = git_stdout(work_dir, ["cat-file", "-p", &anchored.blob])?;
    let anchored_lines: Vec<String> = anchored_bytes.lines().map(str::to_string).collect();
    let anchored_slice_end = (anchored.end as usize).min(anchored_lines.len());
    let anchored_slice_start = (anchored.start as usize).saturating_sub(1);
    if anchored_slice_start > anchored_slice_end {
        anyhow::bail!(
            "Anchored range {}..{} invalid for blob {}",
            anchored.start,
            anchored.end,
            anchored.blob
        );
    }
    let anchored_slice: Vec<&str> = anchored_lines[anchored_slice_start..anchored_slice_end]
        .iter()
        .map(String::as_str)
        .collect();

    let Some(location) = resolve_side_location(work_dir, anchor_sha, anchored)? else {
        return Ok(SideResolved {
            anchored: anchored_location,
            current: None,
            status: LinkStatus::Missing,
            culprit: None,
        });
    };

    let content = git_stdout(work_dir, ["cat-file", "-p", &location.blob])?;
    let lines: Vec<&str> = content.lines().collect();
    let Some(current_slice) = slice_lines(&lines, location.start, location.end) else {
        return Ok(SideResolved {
            anchored: anchored_location,
            current: Some(location),
            status: LinkStatus::Missing,
            culprit: None,
        });
    };

    let status = classify_status(
        &anchored_slice,
        current_slice,
        anchored.ignore_whitespace,
        &location,
        anchored,
    );

    let culprit = match status {
        LinkStatus::Modified | LinkStatus::Rewritten => blame_culprit_commit(
            work_dir,
            &location.path,
            location.start,
            &anchored_slice,
            current_slice,
            anchored.ignore_whitespace,
        )?,
        _ => None,
    };

    Ok(SideResolved {
        anchored: anchored_location,
        current: Some(location),
        status,
        culprit,
    })
}

fn classify_status(
    anchored: &[&str],
    current: &[&str],
    ignore_whitespace: bool,
    location: &LinkLocation,
    anchored_side: &LinkSide,
) -> LinkStatus {
    let stats = diff_stats(anchored, current, ignore_whitespace);
    if stats.equal {
        if location.path == anchored_side.path
            && location.start == anchored_side.start
            && location.end == anchored_side.end
        {
            LinkStatus::Fresh
        } else {
            LinkStatus::Moved
        }
    } else {
        // MODIFIED vs REWRITTEN: per spec §5.3, "majority rewritten" is REWRITTEN.
        // Count removed lines (from anchored) that did not survive; compare against
        // the anchored range length.
        let anchored_len = anchored.len().max(1);
        if stats.removed * 2 > anchored_len {
            LinkStatus::Rewritten
        } else {
            LinkStatus::Modified
        }
    }
}

struct DiffStats {
    equal: bool,
    /// Number of anchored lines not present in the current slice (LCS-based).
    removed: usize,
}

fn diff_stats(anchored: &[&str], current: &[&str], ignore_whitespace: bool) -> DiffStats {
    let a: Vec<String> = anchored
        .iter()
        .map(|line| normalize_line(line, ignore_whitespace))
        .collect();
    let b: Vec<String> = current
        .iter()
        .map(|line| normalize_line(line, ignore_whitespace))
        .collect();

    if a == b {
        return DiffStats {
            equal: true,
            removed: 0,
        };
    }

    let a_refs: Vec<&str> = a.iter().map(String::as_str).collect();
    let b_refs: Vec<&str> = b.iter().map(String::as_str).collect();
    let diff = TextDiff::from_slices(&a_refs, &b_refs);
    let mut removed = 0usize;
    for change in diff.iter_all_changes() {
        if change.tag() == ChangeTag::Delete {
            removed += 1;
        }
    }
    DiffStats {
        equal: false,
        removed,
    }
}

fn slice_lines<'a>(lines: &'a [&'a str], start: u32, end: u32) -> Option<&'a [&'a str]> {
    let start_index = start.checked_sub(1)? as usize;
    let end_index = end as usize;
    if start_index <= end_index && end_index <= lines.len() {
        Some(&lines[start_index..end_index])
    } else {
        None
    }
}

fn normalize_line(line: &str, ignore_whitespace: bool) -> String {
    if ignore_whitespace {
        line.split_whitespace().collect::<String>()
    } else {
        line.to_string()
    }
}

fn blame_culprit_commit(
    work_dir: &Path,
    path: &str,
    start: u32,
    anchored: &[&str],
    current: &[&str],
    ignore_whitespace: bool,
) -> Result<Option<CulpritCommit>> {
    let differing_lines = differing_line_numbers(start, anchored, current, ignore_whitespace);
    if differing_lines.is_empty() {
        return Ok(None);
    }

    let mut newest: Option<(i64, CulpritCommit)> = None;
    for line_number in differing_lines {
        let output = git_stdout(
            work_dir,
            [
                "blame",
                "--porcelain",
                "-L",
                &format!("{line_number},{line_number}"),
                "HEAD",
                "--",
                path,
            ],
        )?;
        let Some(culprit) = parse_blame_culprit(&output) else {
            continue;
        };
        match &newest {
            Some((timestamp, _)) if *timestamp >= culprit.0 => {}
            _ => newest = Some((culprit.0, culprit.1)),
        }
    }

    Ok(newest.map(|(_, culprit)| culprit))
}

fn differing_line_numbers(
    start: u32,
    anchored: &[&str],
    current: &[&str],
    ignore_whitespace: bool,
) -> Vec<u32> {
    // Use the LCS-based diff to identify which positions in `current` differ
    // from `anchored`, and report those as "current" line numbers for blame.
    let a: Vec<String> = anchored
        .iter()
        .map(|line| normalize_line(line, ignore_whitespace))
        .collect();
    let b: Vec<String> = current
        .iter()
        .map(|line| normalize_line(line, ignore_whitespace))
        .collect();
    let a_refs: Vec<&str> = a.iter().map(String::as_str).collect();
    let b_refs: Vec<&str> = b.iter().map(String::as_str).collect();
    let diff = TextDiff::from_slices(&a_refs, &b_refs);

    let mut lines = Vec::new();
    for change in diff.iter_all_changes() {
        if change.tag() == ChangeTag::Insert
            && let Some(idx) = change.new_index()
        {
            lines.push(start + idx as u32);
        }
    }
    if lines.is_empty() {
        // All differences are pure deletions: fall back to the range start.
        lines.push(start);
    }
    lines
}

fn parse_blame_culprit(output: &str) -> Option<(i64, CulpritCommit)> {
    let mut lines = output.lines();
    let commit_oid = lines
        .next()?
        .split_whitespace()
        .next()
        .map(str::to_string)?;
    let mut timestamp = None;
    let mut summary = None;

    for line in lines {
        if let Some(value) = line.strip_prefix("committer-time ") {
            timestamp = value.parse().ok();
            continue;
        }
        if let Some(value) = line.strip_prefix("summary ") {
            summary = Some(value.to_string());
            continue;
        }
        if line.starts_with('\t') {
            break;
        }
    }

    let ts = timestamp?;
    Some((
        ts,
        CulpritCommit {
            commit_oid,
            summary: summary.unwrap_or_default(),
            committed_at: Some(ts),
        },
    ))
}

#[derive(Clone, Debug)]
struct TrackedLocation {
    path: String,
    start: u32,
    end: u32,
}

/// Resolve the current location of a side by walking the history of
/// `<anchor_sha>..HEAD` forward, one commit at a time. Per spec §5.1 this
/// uses `git log -L` style semantics: for each commit that touches the
/// current path, we delegate the hunk-range arithmetic to git itself by
/// invoking `git log -L <start>,<end>:<path> <parent>..<commit>` and
/// parsing the resulting `diff --git` / `@@` headers for the new path
/// and new range. Renames and copies are detected via `--name-status`
/// honoring the side's `copy_detection` setting. If the tracked file is
/// deleted with no surviving successor, returns `Ok(None)` → Missing.
fn resolve_side_location(
    work_dir: &Path,
    anchor_sha: &str,
    anchored: &LinkSide,
) -> Result<Option<LinkLocation>> {
    let head_sha = git_stdout(work_dir, ["rev-parse", "HEAD"])?;
    let commits = git_stdout(
        work_dir,
        [
            "rev-list",
            "--ancestry-path",
            "--reverse",
            &format!("{anchor_sha}..{head_sha}"),
        ],
    )?;
    let mut location = TrackedLocation {
        path: anchored.path.clone(),
        start: anchored.start,
        end: anchored.end,
    };
    let mut parent = anchor_sha.to_string();

    for commit in commits.lines().filter(|line| !line.is_empty()) {
        match advance_location(work_dir, &parent, commit, &location, anchored.copy_detection)? {
            PathChange::Unchanged => {
                // File untouched by this commit; nothing to do.
            }
            PathChange::Deleted => {
                return Ok(None);
            }
            PathChange::Updated(next) => {
                location = next;
            }
        }
        parent = commit.to_string();
    }

    let blob = git_stdout(work_dir, ["rev-parse", &format!("HEAD:{}", location.path)]).ok();
    Ok(blob.map(|blob| LinkLocation {
        path: location.path,
        start: location.start,
        end: location.end,
        blob,
    }))
}

enum PathChange {
    Unchanged,
    Deleted,
    Updated(TrackedLocation),
}

fn advance_location(
    work_dir: &Path,
    parent: &str,
    commit: &str,
    location: &TrackedLocation,
    copy_detection: CopyDetection,
) -> Result<PathChange> {
    let entries = name_status(work_dir, parent, commit, copy_detection)?;
    let mut next_path: Option<String> = None;
    let mut deleted = false;
    let mut modified = false;

    for entry in &entries {
        match entry {
            NameStatus::Modified { path } | NameStatus::Added { path } => {
                if path == &location.path {
                    modified = true;
                    next_path = Some(location.path.clone());
                }
            }
            NameStatus::Deleted { path } => {
                if path == &location.path {
                    deleted = true;
                }
            }
            NameStatus::Renamed { from, to } => {
                if from == &location.path {
                    next_path = Some(to.clone());
                    // A rename that also changes content shows as R<100 with
                    // hunks emitted by `log -L`; treat as modified too so we
                    // re-check line numbers.
                    modified = true;
                    deleted = false;
                }
            }
            NameStatus::Copied { from, to } => {
                // A copy preserves the source; only follow the copy if the
                // source is also deleted in this commit. Preferred path is
                // the copy target when that deletion is present.
                if from == &location.path {
                    next_path = Some(to.clone());
                    modified = true;
                }
            }
        }
    }

    if deleted {
        // If a rename/copy salvaged the path, prefer that over deletion.
        if let Some(new_path) = next_path {
            let new_range = compute_new_range(work_dir, parent, commit, location, &new_path)?;
            return Ok(PathChange::Updated(TrackedLocation {
                path: new_path,
                start: new_range.0,
                end: new_range.1,
            }));
        }
        return Ok(PathChange::Deleted);
    }

    if !modified {
        return Ok(PathChange::Unchanged);
    }

    let new_path = next_path.unwrap_or_else(|| location.path.clone());
    let (start, end) = compute_new_range(work_dir, parent, commit, location, &new_path)?;
    Ok(PathChange::Updated(TrackedLocation {
        path: new_path,
        start,
        end,
    }))
}

/// Compute the new `(start, end)` at `commit` given the tracked range at
/// `parent`. We run `git diff -U0 --find-renames` between the two commits
/// (restricted to the old and new paths so rename detection picks them up
/// as one logical path) and parse the `@@ -a,b +c,d @@` hunk headers to
/// map the tracked window forward, delegating all hunk arithmetic to git.
///
/// The mapping rule per hunk, applied chronologically:
///   * Hunks strictly before the window shift it by `new_count - old_count`.
///   * Hunks strictly after the window leave it unchanged.
///   * Hunks overlapping the window extend it to cover the union of the
///     hunk's new-side extent and the remainder of the window not touched
///     by the hunk. This matches `git log -L`'s own line-walker semantics.
fn compute_new_range(
    work_dir: &Path,
    parent: &str,
    commit: &str,
    location: &TrackedLocation,
    new_path: &str,
) -> Result<(u32, u32)> {
    let output = if new_path == location.path {
        git_stdout(
            work_dir,
            ["diff", "-U0", parent, commit, "--", &location.path],
        )
        .unwrap_or_default()
    } else {
        // For renames/copies: ask git to produce the diff including the
        // rename so hunk coordinates refer to the new path.
        git_stdout(
            work_dir,
            [
                "diff",
                "-U0",
                "--find-renames",
                "--find-copies",
                parent,
                commit,
                "--",
                &location.path,
                new_path,
            ],
        )
        .unwrap_or_default()
    };

    let mut start = location.start as i64;
    let mut end = location.end as i64;
    for line in output.lines() {
        let Some(rest) = line.strip_prefix("@@ -") else {
            continue;
        };
        let Some((old_part, new_rest)) = rest.split_once(" +") else {
            continue;
        };
        let Some((new_part, _)) = new_rest.split_once(" @@") else {
            continue;
        };
        let (old_start, old_count) = parse_hunk_range(old_part)?;
        let (new_start, new_count) = parse_hunk_range(new_part)?;
        let old_start = old_start as i64;
        let old_count = old_count as i64;
        let new_start = new_start as i64;
        let new_count = new_count as i64;
        // Normalize: when count is 0, the hunk anchors *after* old_start
        // (git's convention for pure insertions/deletions).
        let old_last = if old_count == 0 {
            old_start // hunk sits between old_start and old_start+1
        } else {
            old_start + old_count - 1
        };
        let delta = new_count - old_count;

        if old_count == 0 {
            // Pure insertion at position `old_start` (after line old_start).
            if old_start < start {
                start += delta;
                end += delta;
            } else if old_start >= end {
                // insertion past the window — no effect
            } else {
                // insertion inside the window — window grows by new_count
                end += delta;
            }
            continue;
        }

        if old_last < start {
            // hunk strictly before window
            start += delta;
            end += delta;
        } else if old_start > end {
            // hunk strictly after window — no effect
        } else {
            // overlap: replace the intersecting portion with the new-side
            // extent, keeping any untouched tail of the window.
            let tail_len = (end - old_last).max(0);
            let head_len = (old_start - start).max(0);
            start = new_start - head_len;
            if start < 1 {
                start = 1;
            }
            let new_last = if new_count == 0 {
                new_start
            } else {
                new_start + new_count - 1
            };
            end = new_last + tail_len;
        }
    }

    let start_u = start.max(1) as u32;
    let end_u = end.max(start) as u32;
    Ok((start_u, end_u))
}

fn parse_hunk_range(text: &str) -> Result<(u32, u32)> {
    let (start, count) = match text.split_once(',') {
        Some((start, count)) => (start.parse()?, count.parse()?),
        None => (text.parse()?, 1),
    };
    Ok((start, count))
}

enum NameStatus {
    Added { path: String },
    Modified { path: String },
    Deleted { path: String },
    Renamed { from: String, to: String },
    Copied { from: String, to: String },
}

fn name_status(
    work_dir: &Path,
    parent: &str,
    commit: &str,
    copy_detection: CopyDetection,
) -> Result<Vec<NameStatus>> {
    let mut args = vec![
        "diff-tree".to_string(),
        "--no-commit-id".to_string(),
        "--name-status".to_string(),
        "-r".to_string(),
        "-M".to_string(),
    ];
    args.extend(
        copy_detection_args(copy_detection)
            .into_iter()
            .map(str::to_string),
    );
    args.push(parent.to_string());
    args.push(commit.to_string());

    let output = git_stdout(work_dir, args.iter().map(String::as_str))?;
    let mut entries = Vec::new();
    for line in output.lines().filter(|line| !line.is_empty()) {
        let mut parts = line.split('\t');
        let status = parts.next().unwrap_or_default();
        let kind = status.chars().next();
        match kind {
            Some('A') => {
                if let Some(path) = parts.next() {
                    entries.push(NameStatus::Added {
                        path: path.to_string(),
                    });
                }
            }
            Some('M') => {
                if let Some(path) = parts.next() {
                    entries.push(NameStatus::Modified {
                        path: path.to_string(),
                    });
                }
            }
            Some('D') => {
                if let Some(path) = parts.next() {
                    entries.push(NameStatus::Deleted {
                        path: path.to_string(),
                    });
                }
            }
            Some('R') => {
                let from = parts.next().unwrap_or_default().to_string();
                let to = parts.next().unwrap_or_default().to_string();
                entries.push(NameStatus::Renamed { from, to });
            }
            Some('C') => {
                let from = parts.next().unwrap_or_default().to_string();
                let to = parts.next().unwrap_or_default().to_string();
                entries.push(NameStatus::Copied { from, to });
            }
            _ => {}
        }
    }
    Ok(entries)
}

fn copy_detection_args(copy_detection: CopyDetection) -> Vec<&'static str> {
    match copy_detection {
        CopyDetection::Off => Vec::new(),
        CopyDetection::SameCommit => vec!["-C"],
        CopyDetection::AnyFileInCommit => vec!["-C", "-C"],
        CopyDetection::AnyFileInRepo => vec!["-C", "-C", "-C"],
    }
}

fn is_commit_reachable_from_any_ref(work_dir: &Path, commit: &str) -> Result<bool> {
    let output = git_stdout(
        work_dir,
        [
            "for-each-ref",
            "--format=%(refname)",
            "--contains",
            commit,
            "refs",
        ],
    )?;
    Ok(output.lines().any(|line| !line.is_empty()))
}

fn overall_status(sides: &[SideResolved; 2]) -> LinkStatus {
    sides
        .iter()
        .map(|side| side.status)
        .max()
        .unwrap_or(LinkStatus::Fresh)
}
