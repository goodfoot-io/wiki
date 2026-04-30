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
    pub status: String,
}

// ── Runner ────────────────────────────────────────────────────────────────────

/// Walk up from `cwd` (capped at `repo_root`) to find a `wiki.toml`, then
/// report the current wiki's namespace and each declared peer with validation
/// status. Exit 1 if any peer fails rule 1 or rule 2.
pub fn run(cwd: &Path, repo_root: &Path, json: bool) -> Result<i32> {
    let cwd_abs = canonicalize_or_self(cwd);
    let repo_root_abs = canonicalize_or_self(repo_root);

    let toml_path = walk_up_for_toml(&cwd_abs, &repo_root_abs).ok_or_else(|| {
        miette::miette!(
            "no wiki.toml found; run `wiki init` in your wiki root directory."
        )
    })?;

    let wiki_root = toml_path
        .parent()
        .ok_or_else(|| miette::miette!("wiki.toml has no parent directory"))?
        .to_path_buf();

    let (current_ns, peer_map) = parse_wiki_toml(&toml_path)?;

    let mut peer_entries: Vec<NamespaceEntry> = Vec::new();
    let mut any_error = false;

    for (alias, rel) in &peer_map {
        let peer_root = if Path::new(rel.as_str()).is_absolute() {
            PathBuf::from(rel)
        } else {
            wiki_root.join(rel)
        };
        let peer_root = canonicalize_or_self(&peer_root);
        let display_path = relative_display(&wiki_root, &peer_root);

        let peer_toml_path = peer_root.join("wiki.toml");
        if !peer_toml_path.is_file() {
            any_error = true;
            peer_entries.push(NamespaceEntry {
                alias: alias.clone(),
                namespace: None,
                path: display_path,
                status: "error: no wiki.toml".to_string(),
            });
            continue;
        }

        // Rule 1 passed — read the peer's namespace.
        let (peer_ns, _) = match parse_wiki_toml(&peer_toml_path) {
            Ok(v) => v,
            Err(e) => {
                any_error = true;
                peer_entries.push(NamespaceEntry {
                    alias: alias.clone(),
                    namespace: None,
                    path: display_path,
                    status: format!("error: {e}"),
                });
                continue;
            }
        };

        // Rule 2: namespace must match alias (or peer omits namespace and
        // the alias is "default").
        let status = match &peer_ns {
            Some(ns) if ns == alias => "ok".to_string(),
            None if alias == "default" => "ok".to_string(),
            Some(ns) => {
                any_error = true;
                format!("error: namespace mismatch (declared '{ns}' but alias is '{alias}')")
            }
            None => {
                any_error = true;
                format!(
                    "error: namespace mismatch (no namespace declared but alias is '{alias}')"
                )
            }
        };

        peer_entries.push(NamespaceEntry {
            alias: alias.clone(),
            namespace: peer_ns,
            path: display_path,
            status,
        });
    }

    if json {
        let current_path = relative_display(&wiki_root, &wiki_root);
        let mut all: Vec<NamespaceEntry> = Vec::new();
        all.push(NamespaceEntry {
            alias: "current".to_string(),
            namespace: current_ns,
            path: current_path,
            status: "ok".to_string(),
        });
        all.extend(peer_entries);
        println!(
            "{}",
            serde_json::to_string_pretty(&all).into_diagnostic()?
        );
    } else {
        let current_ns_str = current_ns.as_deref().unwrap_or("default");
        let current_path = relative_display(&wiki_root, &wiki_root);
        println!("current: {current_ns_str} ({current_path})");

        if peer_entries.is_empty() {
            println!("peers: (none)");
        } else {
            println!("peers:");
            let alias_width = peer_entries
                .iter()
                .map(|e| e.alias.len())
                .max()
                .unwrap_or(0);
            for entry in &peer_entries {
                println!(
                    "  {:<width$} \u{2192} {}  [{}]",
                    entry.alias,
                    entry.path,
                    entry.status,
                    width = alias_width
                );
            }
        }
    }

    if any_error {
        Ok(1)
    } else {
        Ok(0)
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn canonicalize_or_self(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

fn walk_up_for_toml(start: &Path, repo_root: &Path) -> Option<PathBuf> {
    let mut current: &Path = start;
    loop {
        let candidate = current.join("wiki.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
        if current == repo_root {
            return None;
        }
        match current.parent() {
            Some(parent) => {
                if !parent.starts_with(repo_root) && parent != repo_root {
                    return None;
                }
                current = parent;
            }
            None => return None,
        }
    }
}

/// Return a display path for `target` relative to `base`.
///
/// Always starts with `.` or `..` to be unambiguous. Falls back to the
/// absolute path when no common prefix exists.
fn relative_display(base: &Path, target: &Path) -> String {
    if base == target {
        return ".".to_string();
    }

    let base_components: Vec<_> = base.components().collect();
    let target_components: Vec<_> = target.components().collect();

    let common_len = base_components
        .iter()
        .zip(target_components.iter())
        .take_while(|(a, b)| a == b)
        .count();

    if common_len == 0 {
        return target.to_string_lossy().to_string();
    }

    let up_count = base_components.len() - common_len;
    let mut result = PathBuf::new();
    for _ in 0..up_count {
        result.push("..");
    }
    for component in &target_components[common_len..] {
        result.push(component);
    }

    let s = result.to_string_lossy().to_string();
    if s.starts_with('.') {
        s
    } else {
        format!("./{s}")
    }
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
