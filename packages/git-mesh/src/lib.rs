pub mod types;

use anyhow::{Result, anyhow};
use chrono::Utc;
use std::path::Path;
use std::process::Command;
pub use types::*;
use uuid::Uuid;

pub fn create_link(repo: &gix::Repository, input: CreateLinkInput) -> Result<(String, Link)> {
    let work_dir = repo
        .workdir()
        .ok_or_else(|| anyhow!("Bare repositories are not supported"))?;
    let anchor_sha = match input.anchor_sha {
        Some(anchor_sha) => anchor_sha,
        None => git_stdout(work_dir, ["rev-parse", "HEAD"])?,
    };
    let id = input.id.unwrap_or_else(|| Uuid::new_v4().to_string());
    let link = build_link(work_dir, &anchor_sha, input.sides)?;
    write_link_ref(work_dir, &id, &link)?;
    Ok((id, link))
}

fn build_link_side(
    work_dir: &std::path::Path,
    anchor_sha: &str,
    side: SideSpec,
) -> Result<LinkSide> {
    let blob = resolve_side_blob(work_dir, anchor_sha, &side)?;
    validate_side_range(work_dir, &blob, &side)?;

    Ok(LinkSide {
        path: side.path.clone(),
        start: side.start,
        end: side.end,
        blob,
        copy_detection: side.copy_detection.unwrap_or(DEFAULT_COPY_DETECTION),
        ignore_whitespace: side.ignore_whitespace.unwrap_or(DEFAULT_IGNORE_WHITESPACE),
    })
}

fn build_link(work_dir: &Path, anchor_sha: &str, sides: [SideSpec; 2]) -> Result<Link> {
    let [left, right] = normalize_side_specs(sides);
    let mut sides = [
        build_link_side(work_dir, anchor_sha, left)?,
        build_link_side(work_dir, anchor_sha, right)?,
    ];
    sides.sort();
    Ok(Link {
        anchor_sha: anchor_sha.to_string(),
        created_at: Utc::now().to_rfc3339(),
        sides,
    })
}

fn resolve_side_blob(
    work_dir: &std::path::Path,
    anchor_sha: &str,
    side: &SideSpec,
) -> Result<String> {
    git_stdout(
        work_dir,
        ["rev-parse", &format!("{anchor_sha}:{}", side.path)],
    )
}

fn validate_side_range(work_dir: &std::path::Path, blob: &str, side: &SideSpec) -> Result<()> {
    anyhow::ensure!(side.start >= 1, "range start must be at least 1");
    anyhow::ensure!(side.end >= side.start, "range end must be at least start");

    let line_count = git_stdout(work_dir, ["cat-file", "-p", blob])?
        .lines()
        .count() as u32;
    anyhow::ensure!(
        side.end <= line_count,
        "range {}..={} is out of bounds for {} lines in {}",
        side.start,
        side.end,
        line_count,
        side.path
    );

    Ok(())
}

pub fn commit_mesh(repo: &gix::Repository, input: CommitInput) -> Result<()> {
    let work_dir = repo
        .workdir()
        .ok_or_else(|| anyhow!("Bare repositories are not supported"))?;
    let mesh_ref = format!("refs/meshes/v1/{}", input.name);
    let expected_tip = match input.expected_tip.as_deref() {
        Some(expected_tip) => Some(git_stdout(work_dir, ["rev-parse", expected_tip])?),
        None => git_stdout(work_dir, ["rev-parse", &mesh_ref]).ok(),
    };

    if input.amend && (!input.adds.is_empty() || !input.removes.is_empty()) {
        anyhow::bail!("amend does not accept link changes");
    }

    if !input.amend && input.adds.is_empty() && input.removes.is_empty() {
        anyhow::bail!("mesh commit must add or remove at least one link");
    }

    if expected_tip.is_none() && input.adds.is_empty() {
        anyhow::bail!(
            "mesh `{}` does not exist; supply --link to create it",
            input.name
        );
    }

    let mut links = match expected_tip.as_deref() {
        Some(tip) => read_mesh_links(repo, &gix::ObjectId::from_hex(tip.as_bytes())?)?,
        None => Vec::new(),
    };

    for sides in &input.removes {
        remove_mesh_link(work_dir, &mut links, &normalize_range_specs(sides.clone()))?;
    }

    let anchor_sha = match input.anchor_sha {
        Some(anchor_sha) => anchor_sha,
        None => git_stdout(work_dir, ["rev-parse", "HEAD"])?,
    };
    let mut prepared_links = Vec::with_capacity(input.adds.len());
    for sides in input.adds {
        let normalized_sides = normalize_side_specs(sides);
        if mesh_contains_sides(work_dir, &links, &normalized_sides)? {
            anyhow::bail!("mesh already contains link for pair");
        }
        let link = build_link(work_dir, &anchor_sha, normalized_sides)?;
        let id = Uuid::new_v4().to_string();
        prepared_links.push((id.clone(), link));
        links.push(id);
    }

    for (id, link) in prepared_links {
        write_link_ref(work_dir, &id, &link)?;
    }

    write_mesh_commit(
        work_dir,
        &input.name,
        &input.message,
        &links,
        expected_tip.as_deref(),
        input.amend,
    )
}

pub fn remove_mesh(repo: &gix::Repository, name: &str) -> Result<()> {
    let work_dir = repo
        .workdir()
        .ok_or_else(|| anyhow!("Bare repositories are not supported"))?;
    let mesh_ref = format!("refs/meshes/v1/{name}");
    git_stdout(work_dir, ["update-ref", "-d", &mesh_ref])?;
    Ok(())
}

pub fn rename_mesh(
    repo: &gix::Repository,
    old_name: &str,
    new_name: &str,
    keep: bool,
) -> Result<()> {
    let work_dir = repo
        .workdir()
        .ok_or_else(|| anyhow!("Bare repositories are not supported"))?;
    let old_ref = format!("refs/meshes/v1/{old_name}");
    let new_ref = format!("refs/meshes/v1/{new_name}");
    let commit_oid = git_stdout(work_dir, ["rev-parse", &old_ref])?;
    git_stdout(work_dir, ["update-ref", &new_ref, &commit_oid])?;
    if !keep {
        git_stdout(work_dir, ["update-ref", "-d", &old_ref])?;
    }
    Ok(())
}

pub fn restore_mesh(repo: &gix::Repository, name: &str, commit_ish: &str) -> Result<()> {
    let work_dir = repo
        .workdir()
        .ok_or_else(|| anyhow!("Bare repositories are not supported"))?;
    let mesh_ref = format!("refs/meshes/v1/{name}");
    let revision = if commit_ish == "HEAD" {
        mesh_ref.clone()
    } else if let Some(suffix) = commit_ish.strip_prefix("HEAD") {
        format!("{mesh_ref}{suffix}")
    } else {
        commit_ish.to_string()
    };
    let commit_oid = git_stdout(work_dir, ["rev-parse", &revision])?;
    let current_tip = git_stdout(work_dir, ["rev-parse", &mesh_ref]).ok();
    let tree_oid = git_stdout(work_dir, ["show", "-s", "--format=%T", &commit_oid])?;
    let message = git_stdout(work_dir, ["show", "-s", "--format=%B", &commit_oid])?;

    let mut args = vec![
        "commit-tree".to_string(),
        tree_oid,
        "-m".to_string(),
        message,
    ];
    if let Some(parent) = current_tip.as_deref() {
        args.push("-p".to_string());
        args.push(parent.to_string());
    }

    let restored_commit = git_stdout_with_identity(work_dir, args.iter().map(String::as_str))?;
    match current_tip.as_deref() {
        Some(parent) => git_stdout(
            work_dir,
            ["update-ref", &mesh_ref, &restored_commit, parent],
        )?,
        None => git_stdout(
            work_dir,
            [
                "update-ref",
                &mesh_ref,
                &restored_commit,
                "0000000000000000000000000000000000000000",
            ],
        )?,
    };
    Ok(())
}

pub fn show_mesh(repo: &gix::Repository, name: &str) -> Result<Mesh> {
    let work_dir = repo
        .workdir()
        .ok_or_else(|| anyhow!("Bare repositories are not supported"))?;
    let commit_oid = git_stdout(work_dir, ["rev-parse", &format!("refs/meshes/v1/{name}")])?;
    let message = git_stdout(work_dir, ["show", "-s", "--format=%B", &commit_oid])?;
    let links = git_show_file_lines(work_dir, &commit_oid, "links")?;

    Ok(Mesh {
        name: name.to_string(),
        links,
        message,
    })
}

pub fn list_mesh_names(repo: &gix::Repository) -> Result<Vec<String>> {
    let work_dir = repo
        .workdir()
        .ok_or_else(|| anyhow!("Bare repositories are not supported"))?;
    let output = git_stdout(
        work_dir,
        [
            "for-each-ref",
            "--format=%(refname:strip=3)",
            "refs/meshes/v1",
        ],
    )?;

    Ok(output
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

pub fn mesh_commit_info(repo: &gix::Repository, name: &str) -> Result<MeshCommitInfo> {
    let work_dir = repo
        .workdir()
        .ok_or_else(|| anyhow!("Bare repositories are not supported"))?;
    let commit_oid = git_stdout(work_dir, ["rev-parse", &format!("refs/meshes/v1/{name}")])?;
    let author_name = git_stdout(work_dir, ["show", "-s", "--format=%an", &commit_oid])?;
    let author_email = git_stdout(work_dir, ["show", "-s", "--format=%ae", &commit_oid])?;
    let author_date = git_stdout(work_dir, ["show", "-s", "--format=%aD", &commit_oid])?;

    Ok(MeshCommitInfo {
        commit_oid,
        author_name,
        author_email,
        author_date,
    })
}

pub fn resolve_commit_ish(repo: &gix::Repository, commit_ish: &str) -> Result<String> {
    let work_dir = repo
        .workdir()
        .ok_or_else(|| anyhow!("Bare repositories are not supported"))?;
    git_stdout(work_dir, ["rev-parse", commit_ish])
}

pub fn is_ancestor_commit(repo: &gix::Repository, ancestor: &str, descendant: &str) -> Result<bool> {
    let work_dir = repo
        .workdir()
        .ok_or_else(|| anyhow!("Bare repositories are not supported"))?;
    let status = Command::new("git")
        .current_dir(work_dir)
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .status()?;
    match status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => anyhow::bail!("git merge-base --is-ancestor failed"),
    }
}

pub fn read_link(repo: &gix::Repository, id: &str) -> Result<Link> {
    let work_dir = repo
        .workdir()
        .ok_or_else(|| anyhow!("Bare repositories are not supported"))?;
    read_link_from_ref(work_dir, id)
}

pub fn read_mesh(repo: &gix::Repository, name: &str) -> Result<MeshStored> {
    let mesh = show_mesh(repo, name)?;
    let mut links = Vec::with_capacity(mesh.links.len());

    for id in mesh.links {
        let link = read_link(repo, &id)?;
        links.push(StoredLink {
            id,
            anchor_sha: link.anchor_sha,
            sides: link.sides,
        });
    }

    Ok(MeshStored {
        name: mesh.name,
        message: mesh.message,
        links,
    })
}

pub fn stale_mesh(repo: &gix::Repository, name: &str) -> Result<MeshResolved> {
    let work_dir = repo
        .workdir()
        .ok_or_else(|| anyhow!("Bare repositories are not supported"))?;
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

        links.push(LinkResolved {
            link_id,
            anchor_sha: stored_link.anchor_sha,
            sides,
            status,
        });
    }

    Ok(MeshResolved {
        name: mesh.name,
        message: mesh.message,
        links,
    })
}

pub fn serialize_link(link: &Link) -> String {
    format!(
        "anchor {}\ncreated {}\nside {} {} {} {} {}\t{}\nside {} {} {} {} {}\t{}\n",
        link.anchor_sha,
        link.created_at,
        link.sides[0].start,
        link.sides[0].end,
        link.sides[0].blob,
        serialize_copy_detection(link.sides[0].copy_detection),
        link.sides[0].ignore_whitespace,
        link.sides[0].path,
        link.sides[1].start,
        link.sides[1].end,
        link.sides[1].blob,
        serialize_copy_detection(link.sides[1].copy_detection),
        link.sides[1].ignore_whitespace,
        link.sides[1].path
    )
}

pub fn parse_link(_text: &str) -> Result<Link> {
    let mut anchor_sha = None;
    let mut created_at = None;
    let mut sides = Vec::with_capacity(2);

    for line in _text.lines() {
        if let Some(rest) = line.strip_prefix("anchor ") {
            anchor_sha = Some(rest.to_string());
            continue;
        }
        if let Some(rest) = line.strip_prefix("created ") {
            created_at = Some(rest.to_string());
            continue;
        }
        if let Some(rest) = line.strip_prefix("side ") {
            let (meta, path) = rest
                .split_once('\t')
                .ok_or_else(|| anyhow!("invalid side line"))?;
            let mut parts = meta.split_whitespace();
            let start = parts
                .next()
                .ok_or_else(|| anyhow!("missing side start"))?
                .parse()?;
            let end = parts
                .next()
                .ok_or_else(|| anyhow!("missing side end"))?
                .parse()?;
            let blob = parts
                .next()
                .ok_or_else(|| anyhow!("missing side blob"))?
                .to_string();
            let copy_detection = match parts
                .next()
                .ok_or_else(|| anyhow!("missing copy detection"))?
            {
                "off" => CopyDetection::Off,
                "same-commit" => CopyDetection::SameCommit,
                "any-file-in-commit" => CopyDetection::AnyFileInCommit,
                "any-file-in-repo" => CopyDetection::AnyFileInRepo,
                _ => anyhow::bail!("invalid copy detection"),
            };
            let ignore_whitespace = parts
                .next()
                .ok_or_else(|| anyhow!("missing ignore_whitespace"))?
                .parse()?;
            anyhow::ensure!(parts.next().is_none(), "unexpected side fields");

            sides.push(LinkSide {
                path: path.to_string(),
                start,
                end,
                blob,
                copy_detection,
                ignore_whitespace,
            });
        }
    }

    anyhow::ensure!(sides.len() == 2, "link must contain exactly two sides");
    sides.sort();

    let [left, right]: [LinkSide; 2] = sides
        .try_into()
        .map_err(|_| anyhow!("link must contain exactly two sides"))?;

    Ok(Link {
        anchor_sha: anchor_sha.ok_or_else(|| anyhow!("missing anchor"))?,
        created_at: created_at.ok_or_else(|| anyhow!("missing created timestamp"))?,
        sides: [left, right],
    })
}

pub fn read_mesh_links(_repo: &gix::Repository, _commit_id: &gix::ObjectId) -> Result<Vec<String>> {
    let work_dir = _repo
        .workdir()
        .ok_or_else(|| anyhow!("Bare repositories are not supported"))?;
    git_show_file_lines(work_dir, &_commit_id.to_string(), "links")
}

fn write_link_ref(work_dir: &Path, id: &str, link: &Link) -> Result<()> {
    let blob_oid = git_with_input(
        work_dir,
        ["hash-object", "-w", "--stdin"],
        &serialize_link(link),
    )?;
    git_stdout(
        work_dir,
        ["update-ref", &format!("refs/links/v1/{id}"), &blob_oid],
    )?;
    Ok(())
}

fn read_link_from_ref(work_dir: &Path, id: &str) -> Result<Link> {
    let link_oid = git_stdout(work_dir, ["rev-parse", &format!("refs/links/v1/{id}")])?;
    let link_text = git_stdout(work_dir, ["cat-file", "-p", &link_oid])?;
    parse_link(&link_text)
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
        });
    };

    let content = git_stdout(work_dir, ["cat-file", "-p", &location.blob])?;
    let lines: Vec<&str> = content.lines().collect();
    let Some(current_slice) = slice_lines(&lines, location.start, location.end) else {
        return Ok(SideResolved {
            anchored: anchored_location,
            current: Some(location),
            status: LinkStatus::Missing,
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

    Ok(SideResolved {
        anchored: anchored_location,
        current: Some(location),
        status,
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

fn serialize_copy_detection(copy_detection: CopyDetection) -> &'static str {
    match copy_detection {
        CopyDetection::Off => "off",
        CopyDetection::SameCommit => "same-commit",
        CopyDetection::AnyFileInCommit => "any-file-in-commit",
        CopyDetection::AnyFileInRepo => "any-file-in-repo",
    }
}

fn normalize_side_specs(mut sides: [SideSpec; 2]) -> [SideSpec; 2] {
    for side in &mut sides {
        side.copy_detection.get_or_insert(DEFAULT_COPY_DETECTION);
        side.ignore_whitespace
            .get_or_insert(DEFAULT_IGNORE_WHITESPACE);
    }

    sides.sort_by(|a, b| {
        (
            &a.path,
            a.start,
            a.end,
            a.copy_detection,
            a.ignore_whitespace,
        )
            .cmp(&(
                &b.path,
                b.start,
                b.end,
                b.copy_detection,
                b.ignore_whitespace,
            ))
    });
    sides
}

fn normalize_range_specs(mut sides: [RangeSpec; 2]) -> [RangeSpec; 2] {
    sides.sort();
    sides
}

fn remove_mesh_link(
    work_dir: &Path,
    links: &mut Vec<String>,
    sides: &[RangeSpec; 2],
) -> Result<()> {
    let Some(index) = find_mesh_link_index(work_dir, links, sides)? else {
        anyhow::bail!("mesh does not contain link for pair");
    };
    links.remove(index);
    Ok(())
}

fn find_mesh_link_index(
    work_dir: &Path,
    links: &[String],
    sides: &[RangeSpec; 2],
) -> Result<Option<usize>> {
    for (index, link_id) in links.iter().enumerate() {
        let link = read_link_from_ref(work_dir, link_id)?;
        if link_matches_ranges(&link, sides) {
            return Ok(Some(index));
        }
    }

    Ok(None)
}

fn mesh_contains_sides(work_dir: &Path, links: &[String], sides: &[SideSpec; 2]) -> Result<bool> {
    for link_id in links {
        let link = read_link_from_ref(work_dir, link_id)?;
        if link_matches_sides(&link, sides) {
            return Ok(true);
        }
    }

    Ok(false)
}

fn link_matches_sides(link: &Link, sides: &[SideSpec; 2]) -> bool {
    link.sides
        .iter()
        .zip(sides.iter())
        .all(|(existing, candidate)| {
            existing.path == candidate.path
                && existing.start == candidate.start
                && existing.end == candidate.end
                && existing.copy_detection
                    == candidate.copy_detection.unwrap_or(DEFAULT_COPY_DETECTION)
                && existing.ignore_whitespace
                    == candidate
                        .ignore_whitespace
                        .unwrap_or(DEFAULT_IGNORE_WHITESPACE)
        })
}

fn link_matches_ranges(link: &Link, sides: &[RangeSpec; 2]) -> bool {
    link.sides
        .iter()
        .zip(sides.iter())
        .all(|(existing, candidate)| {
            existing.path == candidate.path
                && existing.start == candidate.start
                && existing.end == candidate.end
        })
}

fn canonicalize_links(links: &[String]) -> Vec<String> {
    let mut canonical = links.to_vec();
    canonical.sort();
    canonical.dedup();
    canonical
}

fn serialize_links_file(links: &[String]) -> String {
    let mut links_text = String::new();
    for link in canonicalize_links(links) {
        links_text.push_str(&link);
        links_text.push('\n');
    }
    links_text
}

fn write_mesh_commit(
    work_dir: &Path,
    name: &str,
    message: &str,
    links: &[String],
    expected_tip: Option<&str>,
    amend: bool,
) -> Result<()> {
    let links_text = serialize_links_file(links);

    let links_blob = git_with_input(work_dir, ["hash-object", "-w", "--stdin"], &links_text)?;
    let tree_oid = git_with_input(
        work_dir,
        ["mktree"],
        &format!("100644 blob {links_blob}\tlinks\n"),
    )?;

    let mesh_ref = format!("refs/meshes/v1/{name}");
    let parents = match (amend, expected_tip) {
        (true, Some(tip)) => git_stdout(work_dir, ["show", "-s", "--format=%P", tip])?
            .split_whitespace()
            .map(str::to_string)
            .collect(),
        (true, None) => Vec::new(),
        (false, Some(tip)) => vec![tip.to_string()],
        (false, None) => Vec::new(),
    };

    let mut args = vec![
        "commit-tree".to_string(),
        tree_oid,
        "-m".to_string(),
        message.to_string(),
    ];
    for parent in parents {
        args.push("-p".to_string());
        args.push(parent);
    }

    let commit_oid = git_stdout_with_identity(work_dir, args.iter().map(String::as_str))?;
    match expected_tip {
        Some(tip) => git_stdout(work_dir, ["update-ref", &mesh_ref, &commit_oid, tip])?,
        None => git_stdout(
            work_dir,
            [
                "update-ref",
                &mesh_ref,
                &commit_oid,
                "0000000000000000000000000000000000000000",
            ],
        )?,
    };
    Ok(())
}

fn git_show_file_lines(work_dir: &Path, commit_oid: &str, path: &str) -> Result<Vec<String>> {
    let output = git_stdout(work_dir, ["show", &format!("{commit_oid}:{path}")])?;
    Ok(output
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

fn git_stdout<I, S>(work_dir: &std::path::Path, args: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let output = Command::new("git")
        .current_dir(work_dir)
        .args(args.into_iter().map(|arg| arg.as_ref().to_string()))
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "git command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn git_stdout_with_identity<I, S>(work_dir: &Path, args: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let output = Command::new("git")
        .current_dir(work_dir)
        .env("GIT_AUTHOR_NAME", "git-mesh")
        .env("GIT_AUTHOR_EMAIL", "git-mesh@example.com")
        .env("GIT_COMMITTER_NAME", "git-mesh")
        .env("GIT_COMMITTER_EMAIL", "git-mesh@example.com")
        .args(args.into_iter().map(|arg| arg.as_ref().to_string()))
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "git command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn git_with_input<I, S>(work_dir: &std::path::Path, args: I, input: &str) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    use std::io::Write;

    let mut child = Command::new("git")
        .current_dir(work_dir)
        .args(args.into_iter().map(|arg| arg.as_ref().to_string()))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    {
        let mut stdin = child.stdin.take().ok_or_else(|| anyhow!("missing stdin"))?;
        stdin.write_all(input.as_bytes())?;
    }
    let output = child.wait_with_output()?;
    anyhow::ensure!(
        output.status.success(),
        "git command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}
