//! `wiki install` command scaffold.
//!
//! Phase 1 of the Codex install plan: this module only validates flags and
//! prints a placeholder message. Network/download, skill installation, hook
//! JSON upsert, and `config.toml` edits are implemented in later phases.

use std::path::Path;

use miette::{Result, miette};

/// Run the `wiki install` command.
///
/// Fails closed unless `--codex` is provided — only the Codex integration is
/// supported initially. Other flags are accepted but not yet acted on.
pub fn run(
    codex: bool,
    force: bool,
    dry_run: bool,
    codex_home: Option<&Path>,
    git_ref: &str,
) -> Result<i32> {
    if !codex {
        return Err(miette!(
            "wiki install: --codex is required (only the Codex integration is supported)"
        ));
    }

    let _ = (force, dry_run, codex_home, git_ref);

    println!("install --codex: not yet implemented (scaffold)");
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_codex_flag() {
        let err = run(false, false, false, None, "main").unwrap_err();
        assert!(err.to_string().contains("--codex is required"));
    }

    #[test]
    fn codex_flag_prints_scaffold_message() {
        let code = run(true, false, false, None, "main").expect("run");
        assert_eq!(code, 0);
    }

    #[test]
    fn accepts_all_flags_without_acting() {
        let code = run(true, true, true, Some(Path::new("/tmp/codex")), "v1.0.0").expect("run");
        assert_eq!(code, 0);
    }
}
