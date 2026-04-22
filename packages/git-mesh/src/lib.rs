pub mod types;

pub use types::*;
use anyhow::{anyhow, Result};
use chrono::Utc;
use gix::refs::transaction::PreviousValue;
use uuid::Uuid;

pub fn create_link(repo: &gix::Repository, input: CreateLinkInput) -> Result<(String, Link)> {
    let anchor_sha = repo
        .rev_parse_single(input.anchor_sha.as_deref().unwrap_or("HEAD"))?
        .detach();

    let [a, b] = input.sides;
    let side_a = build_side(repo, a, &anchor_sha)?;
    let side_b = build_side(repo, b, &anchor_sha)?;
    let mut pair = [side_a, side_b];
    pair.sort();

    let link = Link {
        anchor_sha: anchor_sha.to_string(),
        created_at: Utc::now().to_rfc3339(),
        sides: pair,
    };

    let id       = input.id.unwrap_or_else(|| Uuid::new_v4().to_string());
    let text     = serialize_link(&link);
    let blob_oid = repo.write_blob(text.as_bytes())?.detach();

    repo.reference(
        format!("refs/links/v1/{id}"),
        blob_oid,
        PreviousValue::MustNotExist,
        format!("create link {id}"),
    )?;
    Ok((id, link))
}

fn build_side(
    repo: &gix::Repository,
    spec: SideSpec,
    anchor_sha: &gix::ObjectId,
) -> Result<LinkSide> {
    let blob = resolve_blob(repo, anchor_sha, &spec.path, spec.start, spec.end)?;
    Ok(LinkSide {
        path: spec.path,
        start: spec.start,
        end: spec.end,
        blob: blob.to_string(),
        copy_detection: spec.copy_detection.unwrap_or(DEFAULT_COPY_DETECTION),
        ignore_whitespace: spec.ignore_whitespace.unwrap_or(DEFAULT_IGNORE_WHITESPACE),
    })
}

fn resolve_blob(
    repo: &gix::Repository,
    anchor_sha: &gix::ObjectId,
    path: &str,
    start: u32,
    end: u32,
) -> Result<gix::ObjectId> {
    let commit = repo.find_commit(*anchor_sha)?;
    let tree   = commit.tree()?;
    let entry  = tree
        .lookup_entry_by_path(path)?
        .ok_or_else(|| anyhow!("{path} not found at {anchor_sha}"))?;
    let blob   = repo.find_blob(entry.id())?;
    let mut lines = blob.data.iter().filter(|&&b| b == b'\n').count() as u32;
    if !blob.data.is_empty() && *blob.data.last().unwrap() != b'\n' {
        lines += 1;
    }
    if lines == 0 {
        lines = 1; // Even an empty file is often treated as having 1 line, or we can just say out of bounds if start>lines.
    }
    if start < 1 || end < start || end > lines {
        return Err(anyhow!("range {start}-{end} out of bounds for {path} ({lines} lines)"));
    }
    Ok(entry.id().detach())
}

pub fn commit_mesh(_repo: &gix::Repository, _input: CommitInput) -> Result<()> {
    Err(anyhow::anyhow!("Not implemented"))
}

pub fn remove_mesh(_repo: &gix::Repository, _name: &str) -> Result<()> {
    Err(anyhow::anyhow!("Not implemented"))
}

pub fn rename_mesh(_repo: &gix::Repository, _old_name: &str, _new_name: &str, _keep: bool) -> Result<()> {
    Err(anyhow::anyhow!("Not implemented"))
}

pub fn restore_mesh(_repo: &gix::Repository, _name: &str, _commit_ish: &str) -> Result<()> {
    Err(anyhow::anyhow!("Not implemented"))
}

pub fn show_mesh(_repo: &gix::Repository, _name: &str) -> Result<Mesh> {
    Err(anyhow::anyhow!("Not implemented"))
}

pub fn stale_mesh(_repo: &gix::Repository, _name: &str) -> Result<MeshResolved> {
    Err(anyhow::anyhow!("Not implemented"))
}

pub fn serialize_link(link: &Link) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    writeln!(out, "anchor {}", link.anchor_sha).unwrap();
    writeln!(out, "created {}", link.created_at).unwrap();
    for s in &link.sides {
        writeln!(out, "side {} {} {} {} {}\t{}",
                 s.start, s.end, s.blob,
                 copy_detection_str(s.copy_detection), s.ignore_whitespace,
                 s.path).unwrap();
    }
    out
}

fn copy_detection_str(cd: CopyDetection) -> &'static str {
    match cd {
        CopyDetection::Off => "off",
        CopyDetection::SameCommit => "same-commit",
        CopyDetection::AnyFileInCommit => "any-file-in-commit",
        CopyDetection::AnyFileInRepo => "any-file-in-repo",
    }
}

pub fn parse_link(_text: &str) -> Result<Link> {
    Err(anyhow::anyhow!("Not implemented"))
}

pub fn read_mesh_links(_repo: &gix::Repository, _commit_id: &gix::ObjectId) -> Result<Vec<String>> {
    Err(anyhow::anyhow!("Not implemented"))
}
