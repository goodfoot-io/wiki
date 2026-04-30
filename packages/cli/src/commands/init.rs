//! `wiki init [namespace]` — create a `wiki.toml` in the current directory.
//!
//! Fails closed if `wiki.toml` already exists. Does not walk up — always
//! writes to `cwd`. Does NOT require a loaded `WikiConfig`; it runs before
//! any `wiki.toml` exists.

use std::path::Path;

use miette::{Result, miette};

/// Create `wiki.toml` in `cwd`.
///
/// With `namespace = Some(ns)`: writes `namespace = "<ns>"`.
/// Without: writes an empty file (default namespace).
///
/// Fails if `wiki.toml` already exists (fail-closed).
pub fn run(cwd: &Path, namespace: Option<&str>) -> Result<i32> {
    let toml_path = cwd.join("wiki.toml");
    if toml_path.exists() {
        return Err(miette!(
            "wiki.toml already exists at {}; remove it first if you want to reinitialise",
            toml_path.display()
        ));
    }

    let content = match namespace {
        Some(ns) => format!("namespace = \"{ns}\"\n"),
        None => String::new(),
    };

    std::fs::write(&toml_path, &content).map_err(|e| {
        miette!("failed to write {}: {e}", toml_path.display())
    })?;

    println!("created {}", toml_path.display());
    Ok(0)
}
