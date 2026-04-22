pub mod types;

pub use types::*;
use anyhow::{anyhow, Result};
use chrono::Utc;
use std::fs;
use std::path::Path;
use std::process::Command;
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
    let [left, right] = input.sides;
    let mut sides = [build_link_side(work_dir, left)?, build_link_side(work_dir, right)?];
    sides.sort();
    let link = Link {
        anchor_sha,
        created_at: Utc::now().to_rfc3339(),
        sides,
    };
    let blob_oid = git_with_input(
        work_dir,
        ["hash-object", "-w", "--stdin"],
        &serialize_link(&link),
    )?;
    git_stdout(work_dir, ["update-ref", &format!("refs/links/v1/{id}"), &blob_oid])?;
    Ok((id, link))
}

fn build_link_side(work_dir: &std::path::Path, side: SideSpec) -> Result<LinkSide> {
    validate_side_range(work_dir, &side)?;

    Ok(LinkSide {
        path: side.path.clone(),
        start: side.start,
        end: side.end,
        blob: git_stdout(work_dir, ["rev-parse", &format!("HEAD:{}", side.path)])?,
        copy_detection: side.copy_detection.unwrap_or(DEFAULT_COPY_DETECTION),
        ignore_whitespace: side.ignore_whitespace.unwrap_or(DEFAULT_IGNORE_WHITESPACE),
    })
}

fn validate_side_range(work_dir: &std::path::Path, side: &SideSpec) -> Result<()> {
    anyhow::ensure!(side.start >= 1, "range start must be at least 1");
    anyhow::ensure!(side.end >= side.start, "range end must be at least start");

    let line_count = fs::read_to_string(work_dir.join(&side.path))?.lines().count() as u32;
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

    if input.amend && (!input.adds.is_empty() || !input.removes.is_empty()) {
        anyhow::bail!("amend does not accept link changes");
    }

    if !input.amend && input.adds.is_empty() && input.removes.is_empty() {
        anyhow::bail!("mesh commit must add or remove at least one link");
    }

    if !input.removes.is_empty() {
        anyhow::bail!("remove operations are not implemented");
    }

    let mut links = if input.amend {
        let mesh = show_mesh(repo, &input.name)?;
        mesh.links
    } else {
        show_mesh(repo, &input.name).map(|mesh| mesh.links).unwrap_or_default()
    };

    for sides in input.adds {
        let normalized_sides = normalize_side_specs(sides);
        if mesh_contains_sides(work_dir, &links, &normalized_sides)? {
            anyhow::bail!("mesh already contains link for pair");
        }

        let (id, _) = create_link(
            repo,
            CreateLinkInput {
                sides: normalized_sides,
                anchor_sha: input.anchor_sha.clone(),
                id: None,
            },
        )?;
        links.push(id);
    }

    write_mesh_commit(work_dir, &input.name, &input.message, &links)
}

pub fn remove_mesh(_repo: &gix::Repository, _name: &str) -> Result<()> {
    Err(anyhow!("Not implemented"))
}

pub fn rename_mesh(
    _repo: &gix::Repository,
    _old_name: &str,
    _new_name: &str,
    _keep: bool,
) -> Result<()> {
    Err(anyhow!("Not implemented"))
}

pub fn restore_mesh(_repo: &gix::Repository, _name: &str, _commit_ish: &str) -> Result<()> {
    Err(anyhow!("Not implemented"))
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

pub fn stale_mesh(_repo: &gix::Repository, _name: &str) -> Result<MeshResolved> {
    Err(anyhow!("Not implemented"))
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
            let copy_detection = match parts.next().ok_or_else(|| anyhow!("missing copy detection"))? {
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
    Err(anyhow!("Not implemented"))
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
        side.copy_detection
            .get_or_insert(DEFAULT_COPY_DETECTION);
        side.ignore_whitespace
            .get_or_insert(DEFAULT_IGNORE_WHITESPACE);
    }

    sides.sort_by(|a, b| {
        (&a.path, a.start, a.end, a.copy_detection, a.ignore_whitespace)
            .cmp(&(&b.path, b.start, b.end, b.copy_detection, b.ignore_whitespace))
    });
    sides
}

fn mesh_contains_sides(work_dir: &Path, links: &[String], sides: &[SideSpec; 2]) -> Result<bool> {
    for link_id in links {
        let link_oid = git_stdout(work_dir, ["rev-parse", &format!("refs/links/v1/{link_id}")])?;
        let link_text = git_stdout(work_dir, ["cat-file", "-p", &link_oid])?;
        let link = parse_link(&link_text)?;
        if link_matches_sides(&link, sides) {
            return Ok(true);
        }
    }

    Ok(false)
}

fn link_matches_sides(link: &Link, sides: &[SideSpec; 2]) -> bool {
    link.sides.iter().zip(sides.iter()).all(|(existing, candidate)| {
        existing.path == candidate.path
            && existing.start == candidate.start
            && existing.end == candidate.end
            && existing.copy_detection == candidate.copy_detection.unwrap_or(DEFAULT_COPY_DETECTION)
            && existing.ignore_whitespace
                == candidate
                    .ignore_whitespace
                    .unwrap_or(DEFAULT_IGNORE_WHITESPACE)
    })
}

fn write_mesh_commit(work_dir: &Path, name: &str, message: &str, links: &[String]) -> Result<()> {
    let mut links_text = String::new();
    for link in links {
        links_text.push_str(link);
        links_text.push('\n');
    }

    let links_blob = git_with_input(work_dir, ["hash-object", "-w", "--stdin"], &links_text)?;
    let tree_oid = git_with_input(
        work_dir,
        ["mktree"],
        &format!("100644 blob {links_blob}\tlinks\n"),
    )?;

    let mesh_ref = format!("refs/meshes/v1/{name}");
    let parent = git_stdout(work_dir, ["rev-parse", &mesh_ref]).ok();

    let mut args = vec![
        "commit-tree".to_string(),
        tree_oid,
        "-m".to_string(),
        message.to_string(),
    ];
    if let Some(parent) = parent {
        args.push("-p".to_string());
        args.push(parent);
    }

    let commit_oid = git_stdout_with_identity(work_dir, args.iter().map(String::as_str))?;
    git_stdout(work_dir, ["update-ref", &mesh_ref, &commit_oid])?;
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
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("missing stdin"))?;
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
