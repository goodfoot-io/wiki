//! Staging + commit handlers — §6.2, §6.3, §6.4, §10.5.

use crate::cli::{AddArgs, CommitArgs, ConfigArgs, MessageArgs, RmArgs, StatusArgs};
use anyhow::Result;

pub fn run_add(_repo: &gix::Repository, _args: AddArgs) -> Result<i32> {
    todo!("cli::commit::run_add — §6.3")
}

pub fn run_rm(_repo: &gix::Repository, _args: RmArgs) -> Result<i32> {
    todo!("cli::commit::run_rm — §6.3")
}

pub fn run_message(_repo: &gix::Repository, _args: MessageArgs) -> Result<i32> {
    todo!("cli::commit::run_message — §6.3, §10.2")
}

/// `git mesh commit [<name>]`. With no name, commits every mesh that
/// has a non-empty staging area (post-commit hook path, §10.2). The
/// command no-ops when `.git/rebase-merge/`, `.git/rebase-apply/`,
/// `.git/CHERRY_PICK_HEAD`, `.git/REVERT_HEAD`, or `.git/MERGE_HEAD`
/// is present (§6.7).
pub fn run_commit(_repo: &gix::Repository, _args: CommitArgs) -> Result<i32> {
    todo!("cli::commit::run_commit — §6.2")
}

/// `git mesh status <name>` / `git mesh status --check` (§6.4).
///
/// Exit codes:
/// * `0` — no drift, or no staging area to report.
/// * `1` — `--check` passed and at least one staged range has drifted.
pub fn run_status(_repo: &gix::Repository, _args: StatusArgs) -> Result<i32> {
    todo!("cli::commit::run_status — §6.4")
}

pub fn run_config(_repo: &gix::Repository, _args: ConfigArgs) -> Result<i32> {
    todo!("cli::commit::run_config — §10.5")
}
