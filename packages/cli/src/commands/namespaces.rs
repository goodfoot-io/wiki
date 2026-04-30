//! `wiki namespaces` — list the current wiki and its declared peers with
//! per-peer validation status.
//!
//! Validation is non-fatal here: unlike `WikiConfig::load`, which enforces
//! rules 1–4 fail-closed, this command walks up from `cwd` to find the nearest
//! `wiki.toml` and then re-examines each declared peer independently, reporting
//! per-peer failures inline. A non-zero exit code is emitted if any peer fails
//! rule 1 (missing `wiki.toml`) or rule 2 (alias/namespace mismatch).
//!
//! Implementation note: we use approach (b) from the task description — re-walk
//! peers ourselves with lenient (non-aborting) error handling — rather than
//! adding a `WikiConfig::load_lenient` variant. This keeps `WikiConfig`
//! strictly fail-closed and confines the lenient logic to this command where
//! it is intentionally user-visible. Consequently `namespaces` bypasses the
//! normal `WikiConfig::load` and is listed alongside `install`, `hook`, and
//! `init` in the `needs_config` exclusion in `main.rs`.

use std::path::{Path, PathBuf};

use miette::{IntoDiagnostic, Result, WrapErr};
use serde::Serialize;
use toml_edit::DocumentMut;

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct NamespaceEntry {
    pub alias: String,
    pub namespace: Option<String>,
    pub path: String,
    pub abs_path: String,
    pub status: String,
}

// ── Runner ────────────────────────────────────────────────────────────────────

/// Enumerate every `wiki.toml` in the repo and print one row per namespace.
///
/// Output: `[namespace]\t[absolute path of directory containing wiki.toml]`,
/// one row per discovered wiki, with `default` first and the rest sorted
/// alphabetically. Exit 1 if two wikis declare the same namespace, or if any
/// `wiki.toml` fails to parse.
pub fn run(_cwd: &Path, repo_root: &Path, json: bool) -> Result<i32> {
    let repo_root_abs = canonicalize_or_self(repo_root);

    let toml_paths = find_descendant_tomls(&repo_root_abs);
    if toml_paths.is_empty() {
        return Err(miette::miette!(
            "no wiki.toml found under {}; run `wiki init` in your wiki root directory.",
            repo_root_abs.display()
        ));
    }

    let mut entries: Vec<NamespaceEntry> = Vec::with_capacity(toml_paths.len());
    let mut any_error = false;

    for toml_path in &toml_paths {
        let wiki_root = toml_path
            .parent()
            .ok_or_else(|| miette::miette!("wiki.toml has no parent directory"))?
            .to_path_buf();
        let abs_path = wiki_root.display().to_string();

        let (ns, _peers) = match parse_wiki_toml(toml_path) {
            Ok(v) => v,
            Err(e) => {
                any_error = true;
                entries.push(NamespaceEntry {
                    alias: String::new(),
                    namespace: None,
                    path: abs_path.clone(),
                    abs_path,
                    status: format!("error: {e}"),
                });
                continue;
            }
        };

        entries.push(NamespaceEntry {
            alias: String::new(),
            namespace: ns,
            path: abs_path.clone(),
            abs_path,
            status: "ok".to_string(),
        });
    }

    // Detect duplicate namespace names (default counts as a single bucket).
    let mut seen: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    for entry in &entries {
        if entry.status.starts_with("error") {
            continue;
        }
        let key = entry.namespace.as_deref().unwrap_or("default");
        if let Some(prev) = seen.insert(key, entry.abs_path.as_str()) {
            any_error = true;
            eprintln!(
                "error: namespace `{key}` declared by both {} and {}",
                prev, entry.abs_path
            );
        }
    }

    // Sort: `default` first, then alphabetic by namespace.
    entries.sort_by(|a, b| {
        let a_key = a.namespace.as_deref().unwrap_or("");
        let b_key = b.namespace.as_deref().unwrap_or("");
        match (a_key, b_key) {
            ("", "") => a.abs_path.cmp(&b.abs_path),
            ("", _) => std::cmp::Ordering::Less,
            (_, "") => std::cmp::Ordering::Greater,
            _ => a_key.cmp(b_key),
        }
    });

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&entries).into_diagnostic()?
        );
    } else {
        // Print a third tab column when status is not "ok" so parse errors are visible.
        for entry in &entries {
            let ns = entry.namespace.as_deref().unwrap_or("default");
            if entry.status == "ok" {
                println!("{}\t{}", ns, entry.abs_path);
            } else {
                println!("{}\t{}\t{}", ns, entry.abs_path, entry.status);
            }
        }
    }

    if any_error { Ok(1) } else { Ok(0) }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn canonicalize_or_self(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

fn find_descendant_tomls(repo_root: &Path) -> Vec<PathBuf> {
    let mut results = Vec::new();
    for entry in ignore::WalkBuilder::new(repo_root)
        .standard_filters(true)
        .build()
        .flatten()
    {
        if entry.file_name() == "wiki.toml" {
            let path = entry.into_path();
            if path.is_file() {
                results.push(canonicalize_or_self(&path));
            }
        }
    }
    results.sort();
    results.dedup();
    results
}


/// Parse a `wiki.toml` and return `(namespace, peers)`.
fn parse_wiki_toml(
    path: &Path,
) -> Result<(Option<String>, indexmap::IndexMap<String, String>)> {
    let raw = std::fs::read_to_string(path)
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to read {}", path.display()))?;
    let doc: DocumentMut = raw
        .parse()
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to parse {}", path.display()))?;

    let namespace = doc
        .get("namespace")
        .and_then(|i| i.as_str())
        .map(|s| s.to_string());

    let mut peers = indexmap::IndexMap::new();
    if let Some(item) = doc.get("peers")
        && let Some(table) = item.as_table_like()
    {
        for (k, v) in table.iter() {
            if let Some(s) = v.as_str() {
                peers.insert(k.to_string(), s.to_string());
            }
        }
    }

    Ok((namespace, peers))
}
