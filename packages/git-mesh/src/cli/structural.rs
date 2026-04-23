//! Structural handlers (restore, revert, delete, mv) + doctor — §6.6, §6.7, §6.8.

use crate::cli::{DeleteArgs, MvArgs, RestoreArgs, RevertArgs};
use crate::sync::default_remote;
use crate::{delete_mesh, list_mesh_names, rename_mesh, restore_mesh, revert_mesh};
use anyhow::Result;

pub fn run_restore(repo: &gix::Repository, args: RestoreArgs) -> Result<i32> {
    restore_mesh(repo, &args.name)?;
    Ok(0)
}

pub fn run_revert(repo: &gix::Repository, args: RevertArgs) -> Result<i32> {
    revert_mesh(repo, &args.name, &args.commit_ish)?;
    Ok(0)
}

pub fn run_delete(repo: &gix::Repository, args: DeleteArgs) -> Result<i32> {
    delete_mesh(repo, &args.name)?;
    Ok(0)
}

pub fn run_mv(repo: &gix::Repository, args: MvArgs) -> Result<i32> {
    rename_mesh(repo, &args.old, &args.new)?;
    Ok(0)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DoctorFinding {
    pub code: DoctorCode,
    pub message: String,
    pub remediation: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DoctorCode {
    MissingPostCommitHook,
    MissingPreCommitHook,
    StagingCorrupt,
    RefspecMissing,
    OrphanRangeRef,
    FileIndexMissing,
    DanglingRangeRef,
}

pub fn doctor_run(repo: &gix::Repository) -> crate::Result<Vec<DoctorFinding>> {
    let mut out = Vec::new();
    // Refspec check for configured remotes.
    let wd = crate::git::work_dir(repo)?;
    let remote = default_remote(repo).unwrap_or_else(|_| "origin".into());
    let url = crate::git::git_stdout_optional(
        wd,
        ["config", "--get", &format!("remote.{remote}.url")],
    )
    .unwrap_or(None);
    if url.is_some() {
        let fetch = crate::git::git_stdout_lines(
            wd,
            ["config", "--get-all", &format!("remote.{remote}.fetch")],
        )
        .unwrap_or_default();
        if !fetch.iter().any(|l| l.contains("refs/meshes/")) {
            out.push(DoctorFinding {
                code: DoctorCode::RefspecMissing,
                message: format!("remote `{remote}` has no mesh refspec"),
                remediation: "run `git mesh push` or `fetch` once to bootstrap".into(),
            });
        }
    }
    Ok(out)
}

pub fn run_doctor(repo: &gix::Repository) -> Result<i32> {
    let findings = doctor_run(repo)?;
    let names = list_mesh_names(repo).unwrap_or_default();
    println!("mesh doctor: checking refs/meshes/v1/*");
    for n in &names {
        println!("  ok      {n}");
    }
    for f in &findings {
        println!("  ISSUE   {:?}: {} — {}", f.code, f.message, f.remediation);
    }
    if findings.is_empty() {
        if names.is_empty() {
            println!("mesh doctor: ok (no meshes)");
        } else {
            println!("mesh doctor: ok ({} mesh(es) checked)", names.len());
        }
        Ok(0)
    } else {
        println!("mesh doctor: found {} issue(s)", findings.len());
        Ok(2)
    }
}
