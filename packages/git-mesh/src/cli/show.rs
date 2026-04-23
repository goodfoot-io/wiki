//! `git mesh` list / `git mesh <name>` show / `git mesh ls` — §10.4, §3.4.

use crate::cli::{LsArgs, ShowArgs};
use anyhow::Result;

/// `git mesh` (no args) — list every mesh. Dispatched from `main` when
/// no subcommand and no positional are given.
pub fn run_list(_repo: &gix::Repository) -> Result<i32> {
    todo!("cli::show::run_list — §10.2 reading")
}

/// `git mesh <name>` / `git mesh <name> --log` / `--format=...`.
pub fn run_show(_repo: &gix::Repository, _args: ShowArgs) -> Result<i32> {
    todo!("cli::show::run_show — §10.4")
}

/// `git mesh ls [<path>[#L<s>-L<e>]]` — reads the file index (§3.4).
pub fn run_ls(_repo: &gix::Repository, _args: LsArgs) -> Result<i32> {
    todo!("cli::show::run_ls — §3.4")
}
