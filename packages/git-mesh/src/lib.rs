pub mod types;

use anyhow::{Result, anyhow};
use chrono::Utc;
use std::fs;
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
    git_stdout(work_dir, ["update-ref", &mesh_ref, &commit_oid])?;
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

pub fn stale_mesh(_repo: &gix::Repository, _name: &str) -> Result<MeshResolved> {
    let work_dir = _repo
        .workdir()
        .ok_or_else(|| anyhow!("Bare repositories are not supported"))?;
    let mesh = read_mesh(_repo, _name)?;
    let mut links = Vec::with_capacity(mesh.links.len());

    for stored_link in mesh.links {
        let link_id = stored_link.id;
        let sides = [
            resolve_side(work_dir, &stored_link.sides[0])?,
            resolve_side(work_dir, &stored_link.sides[1])?,
        ];
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

fn resolve_side(work_dir: &Path, anchored: &LinkSide) -> Result<SideResolved> {
    let anchored_location = LinkLocation {
        path: anchored.path.clone(),
        start: anchored.start,
        end: anchored.end,
        blob: anchored.blob.clone(),
    };
    let path = work_dir.join(&anchored.path);
    if !path.exists() {
        return Ok(SideResolved {
            anchored: anchored_location,
            current: None,
            status: LinkStatus::Missing,
        });
    }

    let content = fs::read_to_string(&path)?;
    let current_blob = git_stdout(work_dir, ["hash-object", &anchored.path])?;
    let lines: Vec<&str> = content.lines().collect();
    let anchored_lines: Vec<String> = git_stdout(work_dir, ["cat-file", "-p", &anchored.blob])?
        .lines()
        .map(str::to_string)
        .collect();
    let anchored_slice = &anchored_lines[(anchored.start as usize - 1)..(anchored.end as usize)];

    if let Some(current_slice) = slice_lines(&lines, anchored.start, anchored.end)
        && lines_match(current_slice, anchored_slice, anchored.ignore_whitespace)
    {
        return Ok(SideResolved {
            anchored: anchored_location,
            current: Some(LinkLocation {
                path: anchored.path.clone(),
                start: anchored.start,
                end: anchored.end,
                blob: current_blob,
            }),
            status: LinkStatus::Fresh,
        });
    }

    if let Some(start) = find_matching_block(&lines, anchored_slice, anchored.ignore_whitespace) {
        return Ok(SideResolved {
            anchored: anchored_location,
            current: Some(LinkLocation {
                path: anchored.path.clone(),
                start,
                end: start + (anchored.end - anchored.start),
                blob: current_blob,
            }),
            status: LinkStatus::Moved,
        });
    }

    let status = match slice_lines(&lines, anchored.start, anchored.end) {
        Some(current_slice)
            if similarity_score(current_slice, anchored_slice, anchored.ignore_whitespace) > 0 =>
        {
            LinkStatus::Modified
        }
        _ => LinkStatus::Rewritten,
    };

    Ok(SideResolved {
        anchored: anchored_location,
        current: Some(LinkLocation {
            path: anchored.path.clone(),
            start: anchored.start.min(lines.len() as u32),
            end: anchored.end.min(lines.len() as u32),
            blob: current_blob,
        }),
        status,
    })
}

fn slice_lines<'a>(lines: &'a [&'a str], start: u32, end: u32) -> Option<&'a [&'a str]> {
    let start_index = start.checked_sub(1)? as usize;
    let end_index = end as usize;
    (end_index <= lines.len()).then_some(&lines[start_index..end_index])
}

fn lines_match(current: &[&str], anchored: &[String], ignore_whitespace: bool) -> bool {
    current.len() == anchored.len()
        && current.iter().zip(anchored.iter()).all(|(left, right)| {
            normalize_line(left, ignore_whitespace) == normalize_line(right, ignore_whitespace)
        })
}

fn find_matching_block(
    lines: &[&str],
    anchored: &[String],
    ignore_whitespace: bool,
) -> Option<u32> {
    let width = anchored.len();
    if width == 0 || width > lines.len() {
        return None;
    }

    for start in 0..=(lines.len() - width) {
        if lines_match(&lines[start..start + width], anchored, ignore_whitespace) {
            return Some(start as u32 + 1);
        }
    }

    None
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
