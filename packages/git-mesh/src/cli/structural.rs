//! Structural handlers (restore, revert, delete, mv) + doctor — §6.6, §6.7, §6.8.

use crate::cli::{DeleteArgs, MvArgs, RestoreArgs, RevertArgs};
use anyhow::Result;

pub fn run_restore(_repo: &gix::Repository, _args: RestoreArgs) -> Result<i32> {
    todo!("cli::structural::run_restore — §6.8")
}

pub fn run_revert(_repo: &gix::Repository, _args: RevertArgs) -> Result<i32> {
    todo!("cli::structural::run_revert — §6.6")
}

pub fn run_delete(_repo: &gix::Repository, _args: DeleteArgs) -> Result<i32> {
    todo!("cli::structural::run_delete — §6.8")
}

pub fn run_mv(_repo: &gix::Repository, _args: MvArgs) -> Result<i32> {
    todo!("cli::structural::run_mv — §6.8")
}

/// One finding from `git mesh doctor` (§6.7). Pure data; the CLI owns
/// rendering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DoctorFinding {
    pub code: DoctorCode,
    pub message: String,
    /// Human-readable remediation suggestion (may include a shell snippet).
    pub remediation: String,
}

/// Categorical codes for doctor checks. Keeps machine-readable output
/// stable across future check additions.
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

/// Library-level doctor runner. Pure: returns findings + any self-heal
/// effects (currently only file-index regeneration per §6.7).
pub fn doctor_run(_repo: &gix::Repository) -> crate::Result<Vec<DoctorFinding>> {
    todo!("cli::structural::doctor_run — §6.7")
}

/// `git mesh doctor` — render findings from [`doctor_run`].
pub fn run_doctor(_repo: &gix::Repository) -> Result<i32> {
    todo!("cli::structural::run_doctor — §6.7")
}
