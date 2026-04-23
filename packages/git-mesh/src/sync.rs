use crate::git::{git_stdout, git_stdout_lines, git_stdout_optional};
use anyhow::{Result, anyhow};
use std::path::Path;

pub fn default_remote(repo: &gix::Repository) -> Result<String> {
    let work_dir = repo
        .workdir()
        .ok_or_else(|| anyhow!("Bare repositories are not supported"))?;
    Ok(
        git_stdout_optional(work_dir, ["config", "--get", "mesh.defaultRemote"])?
            .unwrap_or_else(|| "origin".to_string()),
    )
}

pub fn fetch_mesh_refs(repo: &gix::Repository, remote: Option<&str>) -> Result<String> {
    let work_dir = repo
        .workdir()
        .ok_or_else(|| anyhow!("Bare repositories are not supported"))?;
    let remote = remote.map(str::to_string).unwrap_or(default_remote(repo)?);
    ensure_sync_refspecs(work_dir, &remote)?;
    git_stdout(work_dir, ["fetch", &remote])?;
    Ok(remote)
}

pub fn push_mesh_refs(repo: &gix::Repository, remote: Option<&str>) -> Result<String> {
    let work_dir = repo
        .workdir()
        .ok_or_else(|| anyhow!("Bare repositories are not supported"))?;
    let remote = remote.map(str::to_string).unwrap_or(default_remote(repo)?);
    ensure_sync_refspecs(work_dir, &remote)?;
    git_stdout(work_dir, ["push", &remote])?;
    Ok(remote)
}

fn ensure_sync_refspecs(work_dir: &Path, remote: &str) -> Result<()> {
    let fetch_key = format!("remote.{remote}.fetch");
    let push_key = format!("remote.{remote}.push");
    let existing_fetch = git_stdout_lines(work_dir, ["config", "--get-all", &fetch_key])?;
    let existing_push = git_stdout_lines(work_dir, ["config", "--get-all", &push_key])?;

    for refspec in sync_refspecs() {
        if !existing_fetch.iter().any(|existing| existing == refspec) {
            git_stdout(work_dir, ["config", "--add", &fetch_key, refspec])?;
        }
        if !existing_push.iter().any(|existing| existing == refspec) {
            git_stdout(work_dir, ["config", "--add", &push_key, refspec])?;
        }
    }

    Ok(())
}

fn sync_refspecs() -> [&'static str; 2] {
    ["+refs/links/*:refs/links/*", "+refs/meshes/*:refs/meshes/*"]
}
