pub mod types;

pub use types::*;
use anyhow::{anyhow, Result};
use chrono::Utc;
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
    let link = Link {
        anchor_sha,
        created_at: Utc::now().to_rfc3339(),
        sides: [
            LinkSide {
                path: left.path.clone(),
                start: left.start,
                end: left.end,
                blob: git_stdout(work_dir, ["rev-parse", &format!("HEAD:{}", left.path)])?,
                copy_detection: left.copy_detection.unwrap_or(DEFAULT_COPY_DETECTION),
                ignore_whitespace: left.ignore_whitespace.unwrap_or(DEFAULT_IGNORE_WHITESPACE),
            },
            LinkSide {
                path: right.path.clone(),
                start: right.start,
                end: right.end,
                blob: git_stdout(work_dir, ["rev-parse", &format!("HEAD:{}", right.path)])?,
                copy_detection: right.copy_detection.unwrap_or(DEFAULT_COPY_DETECTION),
                ignore_whitespace: right.ignore_whitespace.unwrap_or(DEFAULT_IGNORE_WHITESPACE),
            },
        ],
    };
    let blob_oid = git_with_input(
        work_dir,
        ["hash-object", "-w", "--stdin"],
        &serialize_link(&link),
    )?;
    git_stdout(work_dir, ["update-ref", &format!("refs/links/v1/{id}"), &blob_oid])?;
    Ok((id, link))
}

pub fn commit_mesh(_repo: &gix::Repository, _input: CommitInput) -> Result<()> {
    Err(anyhow!("Not implemented"))
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

pub fn show_mesh(_repo: &gix::Repository, _name: &str) -> Result<Mesh> {
    Err(anyhow!("Not implemented"))
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
    Err(anyhow!("Not implemented"))
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
