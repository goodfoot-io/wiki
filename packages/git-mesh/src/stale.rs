use crate::git::git_stdout;
use crate::mesh::read::read_mesh;
use crate::types::*;
use anyhow::Result;
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
    let anchored_lines: Vec<String> = git_stdout(work_dir, ["cat-file", "-p", &anchored.blob])?
        .lines()
        .map(str::to_string)
        .collect();
    let anchored_slice = &anchored_lines[(anchored.start as usize - 1)..(anchored.end as usize)];

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

    let status = if lines_match(current_slice, anchored_slice, anchored.ignore_whitespace) {
        if location.path == anchored.path
            && location.start == anchored.start
            && location.end == anchored.end
        {
            LinkStatus::Fresh
        } else {
            LinkStatus::Moved
        }
    } else if similarity_score(current_slice, anchored_slice, anchored.ignore_whitespace) * 2
        >= anchored_slice.len()
    {
        LinkStatus::Modified
    } else {
        LinkStatus::Rewritten
    };

    let culprit = match status {
        LinkStatus::Modified | LinkStatus::Rewritten => blame_culprit_commit(
            work_dir,
            &location.path,
            location.start,
            location.end,
            anchored_slice,
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

fn slice_lines<'a>(lines: &'a [&'a str], start: u32, end: u32) -> Option<&'a [&'a str]> {
    let start_index = start.checked_sub(1)? as usize;
    let end_index = end as usize;
    if start_index <= end_index && end_index <= lines.len() {
        Some(&lines[start_index..end_index])
    } else {
        None
    }
}

fn lines_match(current: &[&str], anchored: &[String], ignore_whitespace: bool) -> bool {
    current.len() == anchored.len()
        && current.iter().zip(anchored.iter()).all(|(left, right)| {
            normalize_line(left, ignore_whitespace) == normalize_line(right, ignore_whitespace)
        })
}

fn similarity_score(current: &[&str], anchored: &[String], ignore_whitespace: bool) -> usize {
    current
        .iter()
        .zip(anchored.iter())
        .filter(|(left, right)| {
            normalize_line(left, ignore_whitespace) == normalize_line(right, ignore_whitespace)
        })
        .count()
}

fn normalize_line(line: &str, ignore_whitespace: bool) -> String {
    let normalized = if ignore_whitespace {
        line.split_whitespace().collect::<String>()
    } else {
        line.to_string()
    };

    if let Some(rest) = normalized.strip_prefix("line")
        && !rest.is_empty()
        && rest.chars().all(|ch| ch.is_ascii_digit())
    {
        rest.to_string()
    } else {
        normalized
    }
}

fn blame_culprit_commit(
    work_dir: &Path,
    path: &str,
    start: u32,
    _end: u32,
    anchored: &[String],
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
    anchored: &[String],
    current: &[&str],
    ignore_whitespace: bool,
) -> Vec<u32> {
    let max_len = anchored.len().max(current.len());
    let mut lines = Vec::new();
    for index in 0..max_len {
        let same = anchored
            .get(index)
            .map(|line| normalize_line(line, ignore_whitespace))
            == current
                .get(index)
                .map(|line| normalize_line(line, ignore_whitespace));
        if !same {
            lines.push(start + index as u32);
        }
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
        let Some(next_location) = advance_location(work_dir, &parent, commit, &location, anchored)?
        else {
            return Ok(None);
        };
        location = next_location;
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

fn advance_location(
    work_dir: &Path,
    parent: &str,
    commit: &str,
    location: &TrackedLocation,
    anchored: &LinkSide,
) -> Result<Option<TrackedLocation>> {
    let next_path = resolve_path_change(
        work_dir,
        parent,
        commit,
        &location.path,
        anchored.copy_detection,
    )?;
    let mut next = TrackedLocation {
        path: next_path.clone(),
        start: location.start,
        end: location.end,
    };
    if next_path == location.path {
        apply_hunks_to_range(work_dir, parent, commit, &mut next)?;
    }
    Ok(Some(next))
}

fn resolve_path_change(
    work_dir: &Path,
    parent: &str,
    commit: &str,
    path: &str,
    copy_detection: CopyDetection,
) -> Result<String> {
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
    let mut copied_target = None;

    for line in output.lines().filter(|line| !line.is_empty()) {
        let mut parts = line.split('\t');
        let status = parts.next().unwrap_or_default();
        match status.chars().next() {
            Some('R') | Some('C') => {
                let Some(from) = parts.next() else { continue };
                let Some(to) = parts.next() else { continue };
                if from == path {
                    return Ok(to.to_string());
                }
                if status.starts_with('C') && copied_target.is_none() && to == path {
                    copied_target = Some(to.to_string());
                }
            }
            Some('D') => {
                if parts.next() == Some(path) {
                    return Ok(path.to_string());
                }
            }
            _ => {}
        }
    }

    Ok(copied_target.unwrap_or_else(|| path.to_string()))
}

fn copy_detection_args(copy_detection: CopyDetection) -> Vec<&'static str> {
    match copy_detection {
        CopyDetection::Off => Vec::new(),
        CopyDetection::SameCommit => vec!["-C"],
        CopyDetection::AnyFileInCommit => vec!["-C", "-C"],
        CopyDetection::AnyFileInRepo => vec!["-C", "-C", "-C"],
    }
}

fn apply_hunks_to_range(
    work_dir: &Path,
    parent: &str,
    commit: &str,
    location: &mut TrackedLocation,
) -> Result<()> {
    let output = git_stdout(
        work_dir,
        ["diff", "-U0", parent, commit, "--", &location.path],
    )
    .unwrap_or_default();

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
        adjust_range(location, old_start, old_count, new_start, new_count);
    }

    Ok(())
}

fn parse_hunk_range(text: &str) -> Result<(u32, u32)> {
    let (start, count) = match text.split_once(',') {
        Some((start, count)) => (start.parse()?, count.parse()?),
        None => (text.parse()?, 1),
    };
    Ok((start, count))
}

fn adjust_range(
    location: &mut TrackedLocation,
    old_start: u32,
    old_count: u32,
    new_start: u32,
    new_count: u32,
) {
    let old_end = old_start.saturating_add(old_count);
    let current_len = location.end.saturating_sub(location.start) + 1;
    let delta = new_count as i64 - old_count as i64;

    if old_end <= location.start {
        location.start = shift_line(location.start, delta);
        location.end = shift_line(location.end, delta);
        return;
    }

    if old_start > location.end {
        return;
    }

    location.start = location.start.min(new_start);
    let updated_end = location.end as i64 + delta;
    location.end = updated_end.max(location.start as i64) as u32;
    if location.end < location.start {
        location.end = location.start;
    }
    if location.end < location.start + current_len.saturating_sub(1) {
        location.end = location.start + current_len.saturating_sub(1);
    }
}

fn shift_line(line: u32, delta: i64) -> u32 {
    (line as i64 + delta).max(1) as u32
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
