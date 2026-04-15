//! `wiki install` command.
//!
//! Phase 2 of the Codex install plan: this module resolves the Codex home
//! directory and prints the planned install layout. With `--dry-run`, nothing
//! is written to disk. Network/download, skill installation, hook JSON upsert,
//! and `config.toml` edits are implemented in later phases.

use std::path::{Path, PathBuf};

use miette::{Result, miette};

/// Resolve the Codex home directory.
///
/// Priority order:
/// 1. `cli_override` (from `--codex-home`).
/// 2. `$CODEX_HOME` environment variable.
/// 3. `$HOME/.codex`.
///
/// Fails closed with a clear error if none can be resolved.
pub fn resolve_codex_home(cli_override: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = cli_override {
        return Ok(path.to_path_buf());
    }

    if let Ok(codex_home) = std::env::var("CODEX_HOME")
        && !codex_home.is_empty()
    {
        return Ok(PathBuf::from(codex_home));
    }

    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
    {
        return Ok(PathBuf::from(home).join(".codex"));
    }

    Err(miette!(
        "wiki install: cannot resolve Codex home: pass --codex-home, or set CODEX_HOME or HOME"
    ))
}

/// Run the `wiki install` command.
///
/// Fails closed unless `--codex` is provided — only the Codex integration is
/// supported initially. Phase 2: resolves Codex home and prints the planned
/// layout. With `--dry-run`, prints a dry-run report and exits without writing.
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

    let _ = force;

    let home = resolve_codex_home(codex_home)?;

    let planned: [&str; 5] = [
        "skills/wiki/SKILL.md",
        "skills/wiki/references/maintenance.md",
        "hooks.json",
        "config.toml",
        ".wiki-install/manifest.json",
    ];

    if dry_run {
        println!("wiki install --codex [DRY RUN]");
    } else {
        println!("wiki install --codex");
    }
    println!("  codex home: {}", home.display());
    println!("  ref:        {git_ref}");
    println!("  planned files:");
    for rel in planned {
        println!("    - {}/{}", home.display(), rel);
    }
    if dry_run {
        println!("dry run: no files written.");
        return Ok(0);
    }

    println!("install: download and write steps not yet implemented (phase 2 scaffold)");
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    struct EnvGuard {
        _lock: MutexGuard<'static, ()>,
        prev_codex: Option<String>,
        prev_home: Option<String>,
    }

    impl EnvGuard {
        fn new() -> Self {
            let lock = env_lock()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let prev_codex = std::env::var("CODEX_HOME").ok();
            let prev_home = std::env::var("HOME").ok();
            Self {
                _lock: lock,
                prev_codex,
                prev_home,
            }
        }

        fn set_codex_home(&self, value: Option<&str>) {
            unsafe {
                match value {
                    Some(v) => std::env::set_var("CODEX_HOME", v),
                    None => std::env::remove_var("CODEX_HOME"),
                }
            }
        }

        fn set_home(&self, value: Option<&str>) {
            unsafe {
                match value {
                    Some(v) => std::env::set_var("HOME", v),
                    None => std::env::remove_var("HOME"),
                }
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.prev_codex {
                    Some(v) => std::env::set_var("CODEX_HOME", v),
                    None => std::env::remove_var("CODEX_HOME"),
                }
                match &self.prev_home {
                    Some(v) => std::env::set_var("HOME", v),
                    None => std::env::remove_var("HOME"),
                }
            }
        }
    }

    #[test]
    fn requires_codex_flag() {
        let err = run(false, false, false, None, "main").unwrap_err();
        assert!(err.to_string().contains("--codex is required"));
    }

    #[test]
    fn dry_run_prints_and_exits_zero() {
        let code = run(
            true,
            false,
            true,
            Some(Path::new("/tmp/codex-test-home")),
            "main",
        )
        .expect("run");
        assert_eq!(code, 0);
    }

    #[test]
    fn resolves_from_cli_override() {
        let guard = EnvGuard::new();
        guard.set_codex_home(Some("/env/codex"));
        guard.set_home(Some("/env/home"));

        let override_path = PathBuf::from("/override/codex-home");
        let resolved = resolve_codex_home(Some(&override_path)).expect("resolve");
        assert_eq!(resolved, override_path);
    }

    #[test]
    fn resolves_from_codex_home_env() {
        let guard = EnvGuard::new();
        guard.set_codex_home(Some("/env/codex"));
        guard.set_home(Some("/env/home"));

        let resolved = resolve_codex_home(None).expect("resolve");
        assert_eq!(resolved, PathBuf::from("/env/codex"));
    }

    #[test]
    fn resolves_from_home_when_codex_home_unset() {
        let guard = EnvGuard::new();
        guard.set_codex_home(None);
        guard.set_home(Some("/env/home"));

        let resolved = resolve_codex_home(None).expect("resolve");
        assert_eq!(resolved, PathBuf::from("/env/home/.codex"));
    }

    #[test]
    fn fails_when_nothing_set() {
        let guard = EnvGuard::new();
        guard.set_codex_home(None);
        guard.set_home(None);

        let err = resolve_codex_home(None).unwrap_err();
        assert!(err.to_string().contains("cannot resolve Codex home"));
    }
}
