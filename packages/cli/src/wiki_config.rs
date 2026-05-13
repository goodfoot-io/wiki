//! Loading and validation for `wiki.toml` files.
//!
//! A repo may host any number of independent wikis. Each directory containing
//! a `wiki.toml` defines one wiki. `WikiConfig::load` walks the repo tree (via
//! `find_descendant_tomls`), parses every `wiki.toml`, and stores all of them
//! in a map keyed by namespace name (`"default"` for the anonymous default).
//!
//! The legacy `[peers]` table is parsed and ignored: peers are obsolete now
//! that all wikis in the repo are auto-discovered.

use std::fs;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use miette::{IntoDiagnostic, Result, WrapErr, miette};
use toml_edit::DocumentMut;

// ── Constants ─────────────────────────────────────────────────────────────────

/// The map key used for the anonymous default-namespace wiki.
pub const DEFAULT_KEY: &str = "default";

// ── Shared validator ──────────────────────────────────────────────────────────

/// Validate a namespace string.
///
/// Returns `Ok(())` if the value is acceptable, or an error with a clear
/// message otherwise.  Called both from `WikiConfig::load` and from
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
    if !ns
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
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
    #[allow(dead_code)]
    pub namespace: Option<String>,
}

/// Fully resolved config holding every discovered wiki in the repo.
///
/// Keyed by namespace name. The default namespace uses the literal key
/// [`DEFAULT_KEY`] (`"default"`) — this is a sentinel for map lookup and is
/// distinct from a wiki that declares `namespace = "default"`, which is
/// rejected at load time by [`validate_namespace_value`].
#[derive(Debug, Clone)]
pub struct WikiConfig {
    pub wikis: IndexMap<String, WikiInfo>,
}

impl WikiConfig {
    /// Discover every `wiki.toml` under `repo_root`, parse each, and validate.
    ///
    /// Fail-closed on any parse error, on duplicate namespace names, on more
    /// than one default-namespace wiki, or on the reserved literal namespace
    /// value `"default"`.
    pub fn load(_cwd: &Path, repo_root: &Path) -> Result<Self> {
        let repo_root_abs = canonical_or_self(repo_root);

        let candidates = find_descendant_tomls(&repo_root_abs);
        if candidates.is_empty() {
            return Err(miette!(
                "no wiki.toml found under {}; run `wiki init` in your wiki root directory.",
                repo_root_abs.display()
            ));
        }

        let mut wikis: IndexMap<String, WikiInfo> = IndexMap::new();
        for toml_path in &candidates {
            let wiki_root = toml_path
                .parent()
                .ok_or_else(|| miette!("wiki.toml has no parent directory"))?
                .to_path_buf();

            let namespace = parse_wiki_toml(toml_path)?;

            // Reject the reserved literal "default" namespace value.
            if let Some(ns) = namespace.as_deref()
                && ns == "default"
            {
                return Err(miette!(
                    "wiki.toml at {} declares namespace 'default', which is reserved for the anonymous default — omit the `namespace` field instead",
                    toml_path.display()
                ));
            }

            let key = namespace.clone().unwrap_or_else(|| DEFAULT_KEY.to_string());

            if let Some(prev) = wikis.get(&key) {
                let label = if namespace.is_none() {
                    "default namespace".to_string()
                } else {
                    format!("namespace `{key}`")
                };
                return Err(miette!(
                    "duplicate {label} declared by both {} and {}",
                    prev.root.display(),
                    wiki_root.display()
                ));
            }

            wikis.insert(
                key,
                WikiInfo {
                    root: wiki_root,
                    namespace,
                },
            );
        }

        Ok(WikiConfig { wikis })
    }

    /// Return the default-namespace wiki, if one was declared.
    pub fn default(&self) -> Option<&WikiInfo> {
        self.wikis.get(DEFAULT_KEY)
    }

    /// Iterate every wiki in the repo, in discovery order.
    pub fn all(&self) -> impl Iterator<Item = &WikiInfo> {
        self.wikis.values()
    }

    /// Resolve a `-n NS` argument (or its absence) to a single wiki.
    ///
    /// `target = None` returns the default-namespace wiki.
    /// `target = Some(name)` looks up the wiki with that namespace.
    /// The literal `"*"` is *not* accepted here — multi-namespace callers
    /// iterate [`Self::all`] themselves.
    pub fn resolve(&self, target: Option<&str>) -> Result<&WikiInfo> {
        match target {
            None => self.default().ok_or_else(|| {
                let known = self.known_list();
                miette!(
                    "no default wiki declared in this repo. Declare one (omit `namespace` in a wiki.toml) or pass `-n <namespace>`. Known namespaces: [{known}]"
                )
            }),
            Some("*") => Err(miette!(
                "internal error: WikiConfig::resolve does not handle `*`; iterate `all()` instead"
            )),
            Some(name) => self.wikis.get(name).ok_or_else(|| {
                let known = self.known_list();
                miette!("unknown namespace `{name}`. Known: [{known}]")
            }),
        }
    }

    fn known_list(&self) -> String {
        self.wikis.keys().cloned().collect::<Vec<_>>().join(", ")
    }
}

fn canonical_or_self(p: &Path) -> PathBuf {
    fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Search the repo tree for `wiki.toml` files.
///
/// Respects `.gitignore` and standard filters via the `ignore` crate, so build
/// outputs (`target/`, `node_modules/`, etc.) and hidden directories are
/// skipped. Returns absolute paths in deterministic walk order.
pub fn find_descendant_tomls(repo_root: &Path) -> Vec<PathBuf> {
    let mut results = Vec::new();
    for entry in ignore::WalkBuilder::new(repo_root)
        .standard_filters(true)
        .build()
        .flatten()
    {
        if entry.file_name() == "wiki.toml" {
            let path = entry.into_path();
            if path.is_file() {
                results.push(canonical_or_self(&path));
            }
        }
    }
    results.sort();
    results.dedup();
    results
}

/// Parse a `wiki.toml` and return the declared namespace (if any).
///
/// The `[peers]` table is parsed loosely (just to detect malformed types) but
/// otherwise ignored — peers are obsolete now that the loader auto-discovers
/// every wiki in the repo.
fn parse_wiki_toml(path: &Path) -> Result<Option<String>> {
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

    Ok(namespace)
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
    fn load_finds_wiki_toml_anywhere_in_repo() {
        let dir = TempDir::new().unwrap();
        let repo = dir.path();
        let wiki_root = repo.join("wiki");
        write(&wiki_root.join("wiki.toml"), "");
        let unrelated = repo.join("packages/cli/src/foo");
        fs::create_dir_all(&unrelated).unwrap();
        let cfg = WikiConfig::load(&unrelated, repo).expect("load");
        let canon = fs::canonicalize(&wiki_root).unwrap();
        let def = cfg.default().expect("default wiki");
        assert_eq!(def.root, canon);
        assert!(def.namespace.is_none());
    }

    #[test]
    fn load_errors_when_no_wiki_toml() {
        let dir = TempDir::new().unwrap();
        let repo = dir.path();
        let err = WikiConfig::load(repo, repo).unwrap_err();
        assert!(err.to_string().contains("wiki init"), "got: {err}");
    }

    #[test]
    fn load_succeeds_with_multiple_wikis() {
        let dir = TempDir::new().unwrap();
        let repo = dir.path();
        write(&repo.join("wiki/wiki.toml"), "");
        write(&repo.join("mesh/wiki.toml"), "namespace = \"mesh\"\n");
        let cfg = WikiConfig::load(repo, repo).expect("load");
        assert_eq!(cfg.wikis.len(), 2);
        assert!(cfg.default().is_some());
        assert!(cfg.wikis.contains_key("mesh"));
    }

    #[test]
    fn load_errors_on_duplicate_namespace_name() {
        let dir = TempDir::new().unwrap();
        let repo = dir.path();
        write(&repo.join("alpha/wiki.toml"), "namespace = \"shared\"\n");
        write(&repo.join("beta/wiki.toml"), "namespace = \"shared\"\n");
        let err = WikiConfig::load(repo, repo).unwrap_err();
        let s = err.to_string();
        assert!(
            s.contains("namespace `shared`") || s.contains("duplicate"),
            "got: {s}"
        );
    }

    #[test]
    fn load_errors_on_two_defaults() {
        let dir = TempDir::new().unwrap();
        let repo = dir.path();
        write(&repo.join("a/wiki.toml"), "");
        write(&repo.join("b/wiki.toml"), "");
        let err = WikiConfig::load(repo, repo).unwrap_err();
        assert!(
            err.to_string().contains("default namespace") || err.to_string().contains("duplicate"),
            "got: {err}"
        );
    }

    #[test]
    fn resolve_returns_default_for_none() {
        let dir = TempDir::new().unwrap();
        let repo = dir.path();
        write(&repo.join("wiki/wiki.toml"), "");
        write(&repo.join("mesh/wiki.toml"), "namespace = \"mesh\"\n");
        let cfg = WikiConfig::load(repo, repo).expect("load");
        let def = cfg.resolve(None).expect("default");
        assert!(def.namespace.is_none());
    }

    #[test]
    fn resolve_returns_named_for_some() {
        let dir = TempDir::new().unwrap();
        let repo = dir.path();
        write(&repo.join("wiki/wiki.toml"), "");
        write(&repo.join("mesh/wiki.toml"), "namespace = \"mesh\"\n");
        let cfg = WikiConfig::load(repo, repo).expect("load");
        let mesh = cfg.resolve(Some("mesh")).expect("mesh");
        assert_eq!(mesh.namespace.as_deref(), Some("mesh"));
    }

    #[test]
    fn resolve_errors_on_unknown_namespace() {
        let dir = TempDir::new().unwrap();
        let repo = dir.path();
        write(&repo.join("wiki/wiki.toml"), "");
        let cfg = WikiConfig::load(repo, repo).expect("load");
        let err = cfg.resolve(Some("nope")).unwrap_err();
        assert!(err.to_string().contains("unknown namespace"), "got: {err}");
    }

    #[test]
    fn resolve_errors_when_no_default_declared() {
        let dir = TempDir::new().unwrap();
        let repo = dir.path();
        write(&repo.join("mesh/wiki.toml"), "namespace = \"mesh\"\n");
        let cfg = WikiConfig::load(repo, repo).expect("load");
        let err = cfg.resolve(None).unwrap_err();
        assert!(err.to_string().contains("no default wiki"), "got: {err}");
    }

    #[test]
    fn parse_errors_on_non_string_namespace_integer() {
        let dir = TempDir::new().unwrap();
        let repo = dir.path();
        write(&repo.join("wiki/wiki.toml"), "namespace = 42\n");
        let err = WikiConfig::load(repo, repo).unwrap_err();
        let s = err.to_string();
        assert!(
            s.contains("non-string") || s.contains("must be a string"),
            "got: {s}"
        );
    }

    #[test]
    fn parse_errors_on_non_string_namespace_array() {
        let dir = TempDir::new().unwrap();
        let repo = dir.path();
        write(&repo.join("wiki/wiki.toml"), "namespace = [\"foo\"]\n");
        let err = WikiConfig::load(repo, repo).unwrap_err();
        let s = err.to_string();
        assert!(
            s.contains("non-string") || s.contains("must be a string"),
            "got: {s}"
        );
    }

    #[test]
    fn load_rejects_literal_default_namespace() {
        let dir = TempDir::new().unwrap();
        let repo = dir.path();
        write(&repo.join("wiki/wiki.toml"), "namespace = \"default\"\n");
        let err = WikiConfig::load(repo, repo).unwrap_err();
        let s = err.to_string();
        assert!(
            s.contains("reserved") || s.contains("'default'"),
            "got: {s}"
        );
    }

    #[test]
    fn load_ignores_obsolete_peers_table() {
        let dir = TempDir::new().unwrap();
        let repo = dir.path();
        write(&repo.join("wiki/wiki.toml"), "[peers]\nfoo = \"../foo\"\n");
        // Note: no foo/wiki.toml created — peers used to require this.
        let cfg = WikiConfig::load(repo, repo).expect("load ignores peers");
        assert_eq!(cfg.wikis.len(), 1);
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
}
