//! `git mesh stale` — §10.4.
//!
//! Exit codes (§10.4):
//! * `0` — no stale ranges, or `--no-exit-code` was passed.
//! * `1` — at least one range is not `FRESH`.
//! * `2` — tool error (propagated by `main` when this handler returns `Err`).

use crate::cli::StaleArgs;
use anyhow::Result;

pub fn run_stale(_repo: &gix::Repository, _args: StaleArgs) -> Result<i32> {
    todo!("cli::stale_output::run_stale — §10.4")
}
