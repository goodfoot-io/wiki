use crate::git::{git_show_file_lines, git_stdout};
use crate::link::read_link;
use crate::types::*;
use anyhow::{Result, anyhow};
use std::path::Path;
use std::process::Command;

pub fn show_mesh(repo: &gix::Repository, name: &str) -> Result<Mesh> {
    show_mesh_at(repo, name, None)
}

pub fn show_mesh_at(repo: &gix::Repository, name: &str, commit_ish: Option<&str>) -> Result<Mesh> {
    let work_dir = repo
        .workdir()
        .ok_or_else(|| anyhow!("Bare repositories are not supported"))?;
    let commit_oid = resolve_mesh_revision(work_dir, name, commit_ish)?;
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
    mesh_commit_info_at(repo, name, None)
}

pub fn mesh_commit_info_at(
    repo: &gix::Repository,
    name: &str,
    commit_ish: Option<&str>,
) -> Result<MeshCommitInfo> {
    let work_dir = repo
        .workdir()
        .ok_or_else(|| anyhow!("Bare repositories are not supported"))?;
    let commit_oid = resolve_mesh_revision(work_dir, name, commit_ish)?;
    let author_name = git_stdout(work_dir, ["show", "-s", "--format=%an", &commit_oid])?;
    let author_email = git_stdout(work_dir, ["show", "-s", "--format=%ae", &commit_oid])?;
    let author_date = git_stdout(work_dir, ["show", "-s", "--format=%aD", &commit_oid])?;
    let summary = git_stdout(work_dir, ["show", "-s", "--format=%s", &commit_oid])?;

    Ok(MeshCommitInfo {
        commit_oid,
        author_name,
        author_email,
        author_date,
        summary,
    })
}

pub fn resolve_commit_ish(repo: &gix::Repository, commit_ish: &str) -> Result<String> {
    let work_dir = repo
        .workdir()
        .ok_or_else(|| anyhow!("Bare repositories are not supported"))?;
    git_stdout(work_dir, ["rev-parse", commit_ish])
}

pub fn is_ancestor_commit(
    repo: &gix::Repository,
    ancestor: &str,
    descendant: &str,
) -> Result<bool> {
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

pub fn read_mesh(repo: &gix::Repository, name: &str) -> Result<MeshStored> {
    read_mesh_at(repo, name, None)
}

pub fn read_mesh_at(
    repo: &gix::Repository,
    name: &str,
    commit_ish: Option<&str>,
) -> Result<MeshStored> {
    let mesh = show_mesh_at(repo, name, commit_ish)?;
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

pub fn mesh_log(
    repo: &gix::Repository,
    name: &str,
    limit: Option<usize>,
) -> Result<Vec<MeshCommitInfo>> {
    let work_dir = repo
        .workdir()
        .ok_or_else(|| anyhow!("Bare repositories are not supported"))?;
    let mut args = vec!["rev-list".to_string()];
    if let Some(limit) = limit {
        args.push(format!("--max-count={limit}"));
    }
    args.push(format!("refs/meshes/v1/{name}"));

    let commits = git_stdout(work_dir, args.iter().map(String::as_str))?;
    commits
        .lines()
        .filter(|line| !line.is_empty())
        .map(|commit_oid| mesh_commit_info_at(repo, name, Some(commit_oid)))
        .collect()
}

pub fn read_mesh_links(_repo: &gix::Repository, _commit_id: &gix::ObjectId) -> Result<Vec<String>> {
    let work_dir = _repo
        .workdir()
        .ok_or_else(|| anyhow!("Bare repositories are not supported"))?;
    git_show_file_lines(work_dir, &_commit_id.to_string(), "links")
}

pub(crate) fn resolve_mesh_revision(
    work_dir: &Path,
    name: &str,
    commit_ish: Option<&str>,
) -> Result<String> {
    let mesh_ref = format!("refs/meshes/v1/{name}");
    let revision = match commit_ish {
        None => mesh_ref,
        Some("HEAD") => mesh_ref,
        Some(value) => {
            if let Some(suffix) = value.strip_prefix("HEAD") {
                format!("{mesh_ref}{suffix}")
            } else {
                value.to_string()
            }
        }
    };
    git_stdout(work_dir, ["rev-parse", &revision])
}
