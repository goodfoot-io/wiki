//! `git mesh fetch` / `git mesh push` — §7.

use crate::cli::{FetchArgs, PushArgs};
use anyhow::Result;

pub fn run_fetch(_repo: &gix::Repository, _args: FetchArgs) -> Result<i32> {
    todo!("cli::sync::run_fetch — §7")
}

pub fn run_push(_repo: &gix::Repository, _args: PushArgs) -> Result<i32> {
    todo!("cli::sync::run_push — §7")
}
