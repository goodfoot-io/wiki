//! Wiki search index — gix + rusqlite FTS5 implementation.
//!
//! Phase 1 skeleton: this module declares the public surface consumed by
//! [`crate::commands::search`] and [`crate::commands::summary`] and the
//! internal building blocks called out by `plan/initial.md`. Every body is
//! `unimplemented!()`; the crate compiles but `wiki "x"` panics at runtime.
//! Phase 2 lands skipped tests against this contract; Phase 3 fills in
//! bodies in dependency order.

use std::path::{Path, PathBuf};

use miette::Result;
use serde::Serialize;

pub mod blob;
pub mod freshness;
pub mod fs_class;
pub mod ingest;
pub mod lock;
pub mod passes;
pub mod schema;
pub mod search;
pub mod state;

/// Hard cap on the number of results `wiki "<query>"` will print.
pub const SEARCH_LIMIT: i64 = 3;

const SUGGESTION_LIMIT: i64 = 3;

/// Selects which git snapshot `WikiIndex` reads from.
///
/// The variants are preserved verbatim from the pre-rewrite surface so
/// `commands/{search,summary,check,check_fix,mesh/scaffold,list,hook_check,mod}.rs`
/// keep compiling unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocSource {
    /// Read from the working tree (default, existing behaviour).
    WorkingTree,
    /// Read from the git index (staging area).
    Index,
    /// Read from the HEAD commit.
    Head,
}

impl DocSource {
    /// Return repo-relative paths that this source considers present.
    pub fn list_paths(&self, repo_root: &Path) -> Result<Vec<String>> {
        match self {
            DocSource::WorkingTree => crate::git::repo_inventory(repo_root),
            DocSource::Index => crate::git::index_tracked_paths(repo_root),
            DocSource::Head => crate::git::head_tracked_paths(repo_root),
        }
    }

    /// Return the UTF-8 content of `path_rel` from this source, or `None`
    /// when the path is absent in this source.
    pub fn read(&self, repo_root: &Path, path_rel: &str) -> Result<Option<String>> {
        match self {
            DocSource::WorkingTree => {
                let abs = repo_root.join(path_rel);
                match std::fs::read_to_string(&abs) {
                    Ok(s) => Ok(Some(s)),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                    Err(e) => Err(miette::miette!(e)
                        .wrap_err(format!("failed to read {}", abs.display()))),
                }
            }
            DocSource::Index => crate::git::read_index_blob(repo_root, path_rel),
            DocSource::Head => crate::git::read_head_blob(repo_root, path_rel),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Snippet {
    pub line: usize,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SearchResult {
    pub title: String,
    pub file: String,
    pub summary: String,
    #[serde(skip_serializing)]
    pub alias: Option<String>,
    pub snippets: Vec<Snippet>,
}

impl From<ResolvedPage> for SearchResult {
    fn from(page: ResolvedPage) -> Self {
        SearchResult {
            title: page.title,
            file: page.file,
            summary: page.summary,
            alias: page.alias,
            snippets: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ResolvedPage {
    pub title: String,
    pub file: String,
    pub summary: String,
    pub content: String,
    #[serde(skip_serializing)]
    pub alias: Option<String>,
    #[serde(skip_serializing)]
    pub document_id: i64,
}

/// Which of the three passes contributed a `paths` row.
///
/// Strict ordering for the merge pipeline is `Tree → Index → Worktree`;
/// later sources override earlier sources for the same `path_rel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Tree,
    Index,
    Worktree,
}

/// Git blob object id (sha1 of `"blob " + len + "\0" + content`).
///
/// Uniform identity across committed, staged, untracked, and gitignored
/// markdown. A rename keeps the same `BlobOid`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BlobOid(pub String);

/// Result of a refresh attempt: did we walk, was the lock contended, did
/// the CAS lose?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshOutcome {
    /// Triple-stat gate matched; no refresh required.
    Clean,
    /// We acquired the lock and committed a new `state.generation`.
    Refreshed,
    /// Another process held the refresh lock; we served the existing
    /// snapshot.
    LockContended,
    /// We refreshed but lost the CAS at commit; rolled back.
    CasLost,
}

/// Classification of the filesystem hosting `.git/`. Hostile filesystems
/// (overlayfs / NFS / CIFS) disable the dir-mtime Merkle optimization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostileFs {
    No,
    Yes,
}

/// Public handle to the wiki search index.
///
/// The pre-rewrite version held a Tokio current-thread runtime plus a
/// Turso connection. The new version owns a rusqlite connection opened
/// `SQLITE_OPEN_READONLY` and the repo root used to resolve relative
/// paths during result rendering.
pub struct WikiIndex {
    _repo_root: PathBuf,
    _source: DocSource,
}

impl WikiIndex {
    /// Convenience constructor used by tests that default to the working
    /// tree.
    pub fn prepare(repo_root: &Path) -> Result<Self> {
        Self::prepare_for_source(repo_root, DocSource::WorkingTree)
    }

    /// Open the index database for `source`, run the freshness fast
    /// triple, optionally refresh, and return a read-only handle.
    pub fn prepare_for_source(_repo_root: &Path, _source: DocSource) -> Result<Self> {
        unimplemented!("Phase 1 skeleton: WikiIndex::prepare_for_source")
    }

    /// Resolve a single page by title or alias.
    pub fn resolve_page(&self, _input: &str) -> Result<Option<ResolvedPage>> {
        unimplemented!("Phase 1 skeleton: WikiIndex::resolve_page")
    }

    /// BM25-weighted search, paginated, returning `(rows, total)`.
    pub fn search_weighted(
        &self,
        _query: &str,
        _limit: i64,
        _offset: usize,
    ) -> Result<(Vec<SearchResult>, usize)> {
        unimplemented!("Phase 1 skeleton: WikiIndex::search_weighted")
    }

    /// Up to [`SUGGESTION_LIMIT`] BM25 suggestions for a missed lookup.
    pub fn suggest(&self, _query: &str) -> Result<Vec<SearchResult>> {
        unimplemented!("Phase 1 skeleton: WikiIndex::suggest")
    }
}
