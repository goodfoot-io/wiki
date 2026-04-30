//! Loading, validation, and peer resolution for `wiki.toml` files.
//!
//! Each directory containing a `wiki.toml` is an independent wiki. The CLI
//! walks up from `cwd` to find the nearest `wiki.toml`; that directory is the
//! current wiki. The `[peers]` table enumerates reachable wikis by local alias.

use std::fs;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use miette::{IntoDiagnostic, Result, WrapErr, miette};
use toml_edit::DocumentMut;

// ── Shared validator ──────────────────────────────────────────────────────────

/// Validate a namespace string.
///
/// Returns `Ok(())` if the value is acceptable, or an error with a clear
/// message otherwise.  Called both from `WikiConfig::validate` and from
/// `commands::init`.
pub fn validate_namespace_value(ns: &str) -> Result<()> {
    if ns.is_empty() {
        return Err(miette!("namespace must not be empty"));
    }
    if ns == "default" {
        return Err(miette!(
            "the literal namespace 'default' is reserved for the anonymous default — omit the `namespace` field instead"
        ));
    }
    if !ns.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return Err(miette!(
            "namespace `{ns}` is invalid: only ASCII letters, digits, `_`, and `-` are allowed"
        ));
    }
    Ok(())
}

// ── Types ─────────────────────────────────────────────────────────────────────

/// Runtime identity for one wiki.
#[derive(Debug, Clone)]
pub struct WikiInfo {
    /// Absolute path to the wiki root (the directory containing `wiki.toml`).
    pub root: PathBuf,
    /// `None` indicates the default namespace.
    pub namespace: Option<String>,
}

/// Fully resolved config for the current wiki and its declared peers.
#[derive(Debug, Clone)]
pub struct WikiConfig {
    pub current: WikiInfo,
    /// alias → resolved peer info.
    pub peers: IndexMap<String, WikiInfo>,
}

impl WikiConfig {
    /// Walk up from `cwd` (capped at `repo_root`) to find the nearest
    /// `wiki.toml`, parse it, resolve peers, and validate.
    pub fn load(cwd: &Path, repo_root: &Path) -> Result<Self> {
        let cwd_abs = canonical_or_self(cwd);
        let repo_root_abs = canonical_or_self(repo_root);

        let toml_path = walk_up_for_toml(&cwd_abs, &repo_root_abs).ok_or_else(|| {
            miette!(
                "no wiki.toml found between {} and {}; run `wiki init` in your wiki root directory.",
                cwd_abs.display(),
                repo_root_abs.display()
            )
        })?;

        let wiki_root = toml_path
            .parent()
            .ok_or_else(|| miette!("wiki.toml has no parent directory"))?
            .to_path_buf();

        let (namespace, peer_paths) = parse_wiki_toml(&toml_path)?;

        let mut peers: IndexMap<String, WikiInfo> = IndexMap::new();
        for (alias, rel) in peer_paths {
            let peer_root = if Path::new(&rel).is_absolute() {
                PathBuf::from(&rel)
            } else {
                wiki_root.join(&rel)
            };
            let peer_root = canonical_or_self(&peer_root);
            let peer_toml = peer_root.join("wiki.toml");
            if !peer_toml.is_file() {
                return Err(miette!(
                    "peer `{alias}` resolves to {} but no wiki.toml exists there",
                    peer_root.display()
                ));
            }
            let (peer_ns, _) = parse_wiki_toml(&peer_toml)?;
            peers.insert(
                alias,
                WikiInfo {
                    root: peer_root,
                    namespace: peer_ns,
                },
            );
        }

        let cfg = WikiConfig {
            current: WikiInfo {
                root: wiki_root,
                namespace,
            },
            peers,
        };
        cfg.validate()?;
        Ok(cfg)
    }

    /// Return current + all peer wikis in declaration order (current first).
    /// Used by multi-namespace dispatch (`-n '*'`).
    pub fn all_wikis(&self) -> Vec<&WikiInfo> {
        let mut out = Vec::with_capacity(1 + self.peers.len());
        out.push(&self.current);
        for info in self.peers.values() {
            out.push(info);
        }
        out
    }

    /// Enforce validation rules 1–4 at load time, fail-closed.
    pub fn validate(&self) -> Result<()> {
        // Rule 1 already enforced during load: every peer entry resolves to a
        // directory containing wiki.toml. Re-check defensively.
        for (alias, info) in &self.peers {
            if !info.root.join("wiki.toml").is_file() {
                return Err(miette!(
                    "peer `{alias}` at {} has no wiki.toml",
                    info.root.display()
                ));
            }
        }

        // Rule 2: each peer's namespace matches its alias key (or the peer
        // omits namespace and the alias is "default").
        for (alias, info) in &self.peers {
            match &info.namespace {
                Some(ns) if ns == alias => {}
                None if alias == "default" => {}
                Some(ns) => {
                    return Err(miette!(
                        "peer `{alias}` declares namespace `{ns}` but its alias key is `{alias}`"
                    ));
                }
                None => {
                    return Err(miette!(
                        "peer `{alias}` has no `namespace` field but its alias key is `{alias}` (only the `default` alias may omit namespace)"
                    ));
                }
            }
        }

        // Rule 2b: reject the literal "default" namespace value.
        if let Some(ns) = self.current.namespace.as_deref()
            && ns == "default"
        {
            return Err(miette!(
                "the literal namespace 'default' is reserved for the anonymous default — omit the `namespace` field instead"
            ));
        }
        for (alias, info) in &self.peers {
            if let Some(ns) = info.namespace.as_deref()
                && ns == "default"
            {
                return Err(miette!(
                    "peer `{alias}` has namespace 'default' which is reserved for the anonymous default — omit the `namespace` field instead"
                ));
            }
        }

        // Rule 3: at most one default namespace across current + peers.
        let mut default_count = 0;
        if self.current.namespace.is_none() {
            default_count += 1;
        }
        for info in self.peers.values() {
            if info.namespace.is_none() {
                default_count += 1;
            }
        }
        if default_count > 1 {
            return Err(miette!(
                "more than one default namespace declared across current wiki and peers"
            ));
        }

        // Rule 4: no duplicate namespace values across current + peers.
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        if let Some(ns) = self.current.namespace.as_deref() {
            seen.insert(ns);
        }
        for info in self.peers.values() {
            if let Some(ns) = info.namespace.as_deref()
                && !seen.insert(ns)
            {
                return Err(miette!("duplicate namespace `{ns}` across current + peers"));
            }
        }

        Ok(())
    }
}

fn canonical_or_self(p: &Path) -> PathBuf {
    fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
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
                // Stop if we've climbed above repo_root.
                if !parent.starts_with(repo_root) && parent != repo_root {
                    return None;
                }
                current = parent;
            }
            None => return None,
        }
    }
}

fn parse_wiki_toml(path: &Path) -> Result<(Option<String>, IndexMap<String, String>)> {
    let raw = fs::read_to_string(path)
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to read {}", path.display()))?;
    let doc: DocumentMut = raw
        .parse()
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to parse {}", path.display()))?;

    let namespace = match doc.get("namespace") {
        None => None,
        Some(item) => match item.as_str() {
            Some(s) => Some(s.to_string()),
            None => {
                return Err(miette!(
                    "`namespace` in {} must be a string, found a non-string value",
                    path.display()
                ));
            }
        },
    };

    let mut peers = IndexMap::new();
    if let Some(item) = doc.get("peers") {
        if let Some(table) = item.as_table_like() {
            for (key, value) in table.iter() {
                let path_str = value.as_str().ok_or_else(|| {
                    miette!(
                        "peer `{key}` in {} must be a string path",
                        path.display()
                    )
                })?;
                peers.insert(key.to_string(), path_str.to_string());
            }
        } else {
            return Err(miette!(
                "`peers` in {} must be a table",
                path.display()
            ));
        }
    }

    Ok((namespace, peers))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write(p: &Path, content: &str) {
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, content).unwrap();
    }

    #[test]
    fn load_finds_wiki_toml_by_walk_up() {
        let dir = TempDir::new().unwrap();
        let repo = dir.path();
        let wiki_root = repo.join("wiki");
        write(&wiki_root.join("wiki.toml"), "");
        let deep = wiki_root.join("a/b/c");
        fs::create_dir_all(&deep).unwrap();
        let cfg = WikiConfig::load(&deep, repo).expect("load");
        let canon = fs::canonicalize(&wiki_root).unwrap();
        assert_eq!(cfg.current.root, canon);
        assert!(cfg.current.namespace.is_none());
    }

    #[test]
    fn load_errors_when_no_wiki_toml() {
        let dir = TempDir::new().unwrap();
        let repo = dir.path();
        let err = WikiConfig::load(repo, repo).unwrap_err();
        let s = err.to_string();
        assert!(s.contains("wiki init"), "got: {s}");
    }

    #[test]
    fn validate_rejects_peer_namespace_mismatch() {
        let dir = TempDir::new().unwrap();
        let repo = dir.path();
        let wiki_root = repo.join("wiki");
        write(
            &wiki_root.join("wiki.toml"),
            "[peers]\nfoo = \"../foo\"\n",
        );
        let foo_root = repo.join("foo");
        write(&foo_root.join("wiki.toml"), "namespace = \"bar\"\n");

        let err = WikiConfig::load(&wiki_root, repo).unwrap_err();
        assert!(err.to_string().contains("alias key"), "got: {err}");
    }

    #[test]
    fn validate_rejects_duplicate_namespace() {
        // current = foo, peer = foo (alias mismatch first, but duplicate ns
        // detected when peer namespace matches alias and current namespace).
        // Use different alias names so rule 2 passes; but we reuse a namespace.
        let mut cfg = WikiConfig {
            current: WikiInfo {
                root: PathBuf::from("/w"),
                namespace: Some("foo".into()),
            },
            peers: IndexMap::new(),
        };
        // Peer alias "foo" with namespace "foo" — duplicate of current.
        cfg.peers.insert(
            "foo".into(),
            WikiInfo {
                root: PathBuf::from("/p"),
                namespace: Some("foo".into()),
            },
        );
        // Bypass rule 1 by skipping load and calling validate directly; we
        // expect duplicate-namespace error after rule 2 passes.
        // Construct so rule 1 also passes (skip): provide a real toml.
        // Since validate() first checks rule 1, write the toml.
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("p");
        write(&p.join("wiki.toml"), "");
        cfg.peers.get_mut("foo").unwrap().root = p;
        let err = cfg.validate().unwrap_err();
        // Rule 4 fires (after rule 2 since alias matches namespace).
        assert!(err.to_string().contains("duplicate"), "got: {err}");
    }

    // ── F4: non-string namespace ──────────────────────────────────────────────

    #[test]
    fn parse_errors_on_non_string_namespace_integer() {
        let dir = TempDir::new().unwrap();
        let repo = dir.path();
        let wiki_root = repo.join("wiki");
        write(&wiki_root.join("wiki.toml"), "namespace = 42\n");
        let err = WikiConfig::load(&wiki_root, repo).unwrap_err();
        let s = err.to_string();
        assert!(s.contains("non-string") || s.contains("must be a string"), "got: {s}");
    }

    #[test]
    fn parse_errors_on_non_string_namespace_array() {
        let dir = TempDir::new().unwrap();
        let repo = dir.path();
        let wiki_root = repo.join("wiki");
        write(&wiki_root.join("wiki.toml"), "namespace = [\"foo\"]\n");
        let err = WikiConfig::load(&wiki_root, repo).unwrap_err();
        let s = err.to_string();
        assert!(s.contains("non-string") || s.contains("must be a string"), "got: {s}");
    }

    #[test]
    fn parse_errors_on_non_string_namespace_in_peer() {
        let dir = TempDir::new().unwrap();
        let repo = dir.path();
        let wiki_root = repo.join("wiki");
        write(
            &wiki_root.join("wiki.toml"),
            "[peers]\nfoo = \"../foo\"\n",
        );
        let foo_root = repo.join("foo");
        write(&foo_root.join("wiki.toml"), "namespace = true\n");
        let err = WikiConfig::load(&wiki_root, repo).unwrap_err();
        let s = err.to_string();
        assert!(s.contains("non-string") || s.contains("must be a string"), "got: {s}");
    }

    // ── F8: literal "default" namespace ──────────────────────────────────────

    #[test]
    fn validate_rejects_literal_default_namespace_on_current() {
        let dir = TempDir::new().unwrap();
        let repo = dir.path();
        let wiki_root = repo.join("wiki");
        write(&wiki_root.join("wiki.toml"), "namespace = \"default\"\n");
        let err = WikiConfig::load(&wiki_root, repo).unwrap_err();
        let s = err.to_string();
        assert!(s.contains("reserved") || s.contains("'default'"), "got: {s}");
    }

    #[test]
    fn validate_rejects_literal_default_namespace_on_peer() {
        let dir = TempDir::new().unwrap();
        let repo = dir.path();
        let wiki_root = repo.join("wiki");
        write(
            &wiki_root.join("wiki.toml"),
            "namespace = \"host\"\n[peers]\ndefault = \"../def\"\n",
        );
        let def_root = repo.join("def");
        write(&def_root.join("wiki.toml"), "namespace = \"default\"\n");
        let err = WikiConfig::load(&wiki_root, repo).unwrap_err();
        let s = err.to_string();
        assert!(s.contains("reserved") || s.contains("'default'"), "got: {s}");
    }

    // ── validate_namespace_value helper ──────────────────────────────────────

    #[test]
    fn namespace_value_rejects_empty() {
        let err = validate_namespace_value("").unwrap_err();
        assert!(err.to_string().contains("empty"), "got: {err}");
    }

    #[test]
    fn namespace_value_rejects_default() {
        let err = validate_namespace_value("default").unwrap_err();
        assert!(err.to_string().contains("reserved"), "got: {err}");
    }

    #[test]
    fn namespace_value_rejects_bad_chars() {
        let err = validate_namespace_value("foo bar").unwrap_err();
        assert!(err.to_string().contains("invalid"), "got: {err}");
    }

    #[test]
    fn namespace_value_accepts_valid() {
        assert!(validate_namespace_value("foo").is_ok());
        assert!(validate_namespace_value("foo-bar_1").is_ok());
        assert!(validate_namespace_value("A9").is_ok());
    }

    #[test]
    fn validate_rejects_two_default_namespaces() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("p");
        write(&p.join("wiki.toml"), "");
        let mut peers = IndexMap::new();
        peers.insert(
            "default".into(),
            WikiInfo {
                root: p,
                namespace: None,
            },
        );
        let cfg = WikiConfig {
            current: WikiInfo {
                root: PathBuf::from("/w"),
                namespace: None,
            },
            peers,
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("default namespace"), "got: {err}");
    }
}
