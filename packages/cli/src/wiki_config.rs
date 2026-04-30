//! Loading, validation, and peer resolution for `wiki.toml` files.
//!
//! Each directory containing a `wiki.toml` is an independent wiki. The CLI
//! walks up from `cwd` to find the nearest `wiki.toml`; that directory is the
//! current wiki. The `[peers]` table enumerates reachable wikis by local alias.
//!
//! Phase 1: type surface and stubs only — `load` and `validate` are
//! `unimplemented!()` placeholders. Bodies arrive in Phase 2. The types are
//! intentionally unused at call sites until then.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use miette::Result;
use serde::Deserialize;

// ── Types ─────────────────────────────────────────────────────────────────────

/// The deserialized shape of a `wiki.toml` file.
#[derive(Debug, Deserialize)]
pub(crate) struct WikiToml {
    pub namespace: Option<String>,
    #[serde(default)]
    pub peers: IndexMap<String, String>, // alias → relative path string
}

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
    pub fn load(_cwd: &Path, _repo_root: &Path) -> Result<Self> {
        unimplemented!("WikiConfig::load — implemented in Phase 2")
    }

    /// Enforce validation rules 1–4 (peer toml exists, namespace alias match,
    /// at-most-one default, no duplicate namespaces).
    pub fn validate(&self) -> Result<()> {
        unimplemented!("WikiConfig::validate — implemented in Phase 2")
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    #[ignore = "Phase 2: implement WikiConfig::load"]
    fn load_finds_wiki_toml_by_walk_up() {
        // Phase 2: create a temp dir tree where `wiki.toml` sits two parents
        // above `cwd`, call `WikiConfig::load`, and assert `current.root`
        // points at the toml directory.
        let cwd = PathBuf::from("/nonexistent/a/b/c");
        let repo_root = PathBuf::from("/nonexistent");
        let _ = WikiConfig::load(&cwd, &repo_root);
    }

    #[test]
    #[ignore = "Phase 2: implement WikiConfig::load"]
    fn load_errors_when_no_wiki_toml() {
        // Phase 2: load() in a tree with no `wiki.toml` returns an Err whose
        // message mentions running `wiki init`.
        let cwd = PathBuf::from("/nonexistent");
        let repo_root = PathBuf::from("/nonexistent");
        let _ = WikiConfig::load(&cwd, &repo_root);
    }

    #[test]
    #[ignore = "Phase 2: implement WikiConfig::validate"]
    fn validate_rejects_peer_namespace_mismatch() {
        // Rule 2: peer's `namespace` field must match its alias key in
        // `[peers]` (or the peer omits `namespace` and the alias is "default").
        let cfg = WikiConfig {
            current: WikiInfo {
                root: PathBuf::from("/w"),
                namespace: None,
            },
            peers: IndexMap::new(),
        };
        let _ = cfg.validate();
    }

    #[test]
    #[ignore = "Phase 2: implement WikiConfig::validate"]
    fn validate_rejects_duplicate_namespace() {
        // Rule 4: no duplicate namespace values across current + peers.
        let cfg = WikiConfig {
            current: WikiInfo {
                root: PathBuf::from("/w"),
                namespace: Some("foo".into()),
            },
            peers: IndexMap::new(),
        };
        let _ = cfg.validate();
    }

    #[test]
    #[ignore = "Phase 2: implement WikiConfig::validate"]
    fn validate_rejects_two_default_namespaces() {
        // Rule 3: at most one default namespace across current + all peers.
        let cfg = WikiConfig {
            current: WikiInfo {
                root: PathBuf::from("/w"),
                namespace: None,
            },
            peers: IndexMap::new(),
        };
        let _ = cfg.validate();
    }
}
