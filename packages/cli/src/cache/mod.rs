//! The anchor cache: a disposable, never-authoritative SQLite store under
//! `$GIT_COMMON_DIR/wiki/` that memoizes main-3's drift-engine costs (plan
//! decision 1).
//!
//! Two tiers share one database file: tier F memoizes the per-link rk64
//! fingerprint of a certified target range at the anchor commit; tier A
//! memoizes the per-commit blob-read + YAML-parse leg of the per-page
//! anchor-commit walk. Deleting, corrupting, or racing the cache never
//! changes a check's result, exit code, or diagnostics — only the next run's
//! speed. Every served row is re-verified before it is trusted: the stored
//! tuple is compared field-by-field against the queried tuple and its sha256
//! `row_digest` checked — any mismatch is a miss, never a wrong serve (plan
//! decisions 1 and 5), so the cache is never authoritative.
//!
//! [`AnchorCache`] is the surface the drift seams consume. [`CacheStore`] is
//! the real implementation; [`NoopCache`] is the compile-time kill-switch
//! type (`WIKI_ANCHOR_CACHE=0`, plan decision 8), so "cache off" can never
//! silently start caching later. Any error from any method means the cache
//! is unavailable for the run — the caller falls back to uncached
//! computation with at most one diagnostic line (plan decisions 4 and 7).

pub mod key;
pub mod schema;

use std::path::{Path, PathBuf};

/// One cached tier-A row: the anchor epoch (`LinkEpoch::Commit` in the drift
/// engine) of a page's `git log --follow --name-status` walk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkRow {
    /// The anchor commit SHA the walk selected.
    pub anchor_sha: String,
    /// The page's path in effect at the anchor commit (rename-tracked).
    pub path_at_commit: String,
    /// The parsed `links-reviewed:` value at the anchor commit. `None` when
    /// the field is absent there — a field-less anchor is a legitimate
    /// cached state and is distinct from an empty value.
    pub value: Option<String>,
}

/// A cache fault. Every variant means the cache is unavailable for the run;
/// the caller falls back to uncached computation with at most one diagnostic
/// line (plan decision 7). Transient `SQLITE_BUSY` is retried internally by
/// the bounded wrapper (plan decision 6) and never quarantines — it surfaces
/// here only after the retries are exhausted.
#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    /// Filesystem operation failed (lock, quarantine rename, sidecar delete,
    /// directory create).
    #[error("anchor cache I/O failure: {0}")]
    Io(#[from] std::io::Error),
    /// SQLite operation failed (probe, open, statement).
    #[error("anchor cache SQLite failure: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// The database was corrupt (NOTADB / CORRUPT / meta mismatch) and the
    /// single quarantine-and-recreate attempt also failed.
    #[error("anchor cache is corrupt and could not be recreated: {0}")]
    Corrupt(String),
}

/// The cache surface the drift seams consume (plan decision 5). Reads verify
/// before serving — the stored tuple is compared field-by-field against the
/// queried tuple and its `row_digest` checked; writes are computed strictly
/// outside any transaction by the caller and flushed per page.
pub trait AnchorCache {
    /// Return the cached 16-hex rk64 fingerprint for the queried tuple, or
    /// `None` on a miss. The row is served only after the stored tuple is
    /// compared field-by-field against the queried tuple (all five fields)
    /// and its `row_digest` verified; any mismatch — or any error during the
    /// lookup — is a miss, never an error (plan decision 5).
    fn lookup_fingerprint(
        &self,
        key: &str,
        page_path: &str,
        anchor_sha: &str,
        target_path: &str,
        range_start: u32,
        range_end: u32,
    ) -> Result<Option<String>, CacheError>;

    /// Store (or overwrite) one fingerprint row. `key` must be
    /// [`key::fingerprint_key`] over the same tuple; `fp` is the 16-hex
    /// `rk64_to_hex` form, including `"0000000000000000"` for a zero
    /// fingerprint.
    #[allow(clippy::too_many_arguments)]
    fn upsert_fingerprint(
        &self,
        key: &str,
        page_path: &str,
        anchor_sha: &str,
        target_path: &str,
        range_start: u32,
        range_end: u32,
        fp: &str,
    ) -> Result<(), CacheError>;

    /// Return the cached anchor-walk epoch for the queried tuple, or `None`
    /// on a miss. The row is served only after the stored `page_path` and
    /// `log_output_sha` — the latter compared against
    /// [`key::sha256_hex`] of the queried `log_output` — match the queried
    /// tuple and its `row_digest` verifies; any mismatch — or any error
    /// during the lookup — is a miss, never an error (plan decision 5).
    fn lookup_walk(
        &self,
        key: &str,
        page_path: &str,
        log_output: &str,
    ) -> Result<Option<WalkRow>, CacheError>;

    /// Store (or overwrite) one anchor-walk row. `key` must be
    /// [`key::walk_key`] over the page path and the exact untrimmed log
    /// output the walk parses; `log_output_sha` is [`key::sha256_hex`] over
    /// that same string. `value` is the parsed `links-reviewed:` value —
    /// `None` for a field-less anchor commit.
    fn upsert_walk(
        &self,
        key: &str,
        page_path: &str,
        log_output_sha: &str,
        anchor_sha: &str,
        path_at_commit: &str,
        value: Option<&str>,
    ) -> Result<(), CacheError>;

    /// Delete the cache contents — the database file, its WAL sidecars, and
    /// the `wiki/` directory itself (used by `wiki check --clear-cache`,
    /// plan decision 8).
    fn clear(&self) -> Result<(), CacheError>;
}

/// The real anchor cache: an SQLite database under the repository's common
/// git directory, shared by every linked worktree of one repo (plan
/// decision 2). See [`schema`] for the storage layout and the
/// probe/quarantine/open ordering.
//
// P1 stub: the fields are first read by Phase 1's open/probe/quarantine
// path; the allow is removed there.
#[allow(dead_code)]
pub struct CacheStore {
    /// Open SQLite connection; `None` while the P1 stub is in effect.
    conn: Option<rusqlite::Connection>,
    /// Common git dir — derives the db and init-lock paths via [`schema`].
    common_dir: PathBuf,
}

impl CacheStore {
    /// Open the anchor cache under `common_dir` (the repository's common
    /// git directory — resolve it via [`git::common_dir`], which the caller
    /// turns into "cache disabled for the run" on failure).
    ///
    /// * `Ok(None)` — the init lock is held by another process; this run
    ///   proceeds uncached (plan decision 4).
    /// * `Err` — the cache is unavailable for the run (one diagnostic, then
    ///   uncached computation).
    ///
    /// P1 stub: returns an unopened store. Phase 1 implements the
    /// probe → quarantine → open ordering of plan decision 4.
    pub fn open(common_dir: &Path) -> Result<Option<Self>, CacheError> {
        Ok(Some(CacheStore {
            conn: None,
            common_dir: common_dir.to_path_buf(),
        }))
    }
}

impl AnchorCache for CacheStore {
    fn lookup_fingerprint(
        &self,
        _key: &str,
        _page_path: &str,
        _anchor_sha: &str,
        _target_path: &str,
        _range_start: u32,
        _range_end: u32,
    ) -> Result<Option<String>, CacheError> {
        // P1 stub — Phase 2 compares the stored tuple field-by-field and
        // verifies row_digest on serve; any mismatch is a miss.
        Ok(None)
    }

    #[allow(clippy::too_many_arguments)]
    fn upsert_fingerprint(
        &self,
        _key: &str,
        _page_path: &str,
        _anchor_sha: &str,
        _target_path: &str,
        _range_start: u32,
        _range_end: u32,
        _fp: &str,
    ) -> Result<(), CacheError> {
        // P1 stub.
        Ok(())
    }

    fn lookup_walk(
        &self,
        _key: &str,
        _page_path: &str,
        _log_output: &str,
    ) -> Result<Option<WalkRow>, CacheError> {
        // P1 stub — Phase 3 compares the stored page_path/log_output_sha and
        // verifies row_digest on serve; any mismatch is a miss.
        Ok(None)
    }

    fn upsert_walk(
        &self,
        _key: &str,
        _page_path: &str,
        _log_output_sha: &str,
        _anchor_sha: &str,
        _path_at_commit: &str,
        _value: Option<&str>,
    ) -> Result<(), CacheError> {
        // P1 stub.
        Ok(())
    }

    fn clear(&self) -> Result<(), CacheError> {
        // P1 stub.
        Ok(())
    }
}

/// The compile-time kill-switch cache: every method is a permanent miss that
/// stores nothing (`WIKI_ANCHOR_CACHE=0`, plan decision 8). The drift seams
/// type their disabled path with this so "cache off" is a distinct type, not
/// a flag that can rot.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopCache;

impl AnchorCache for NoopCache {
    fn lookup_fingerprint(
        &self,
        _key: &str,
        _page_path: &str,
        _anchor_sha: &str,
        _target_path: &str,
        _range_start: u32,
        _range_end: u32,
    ) -> Result<Option<String>, CacheError> {
        Ok(None)
    }

    #[allow(clippy::too_many_arguments)]
    fn upsert_fingerprint(
        &self,
        _key: &str,
        _page_path: &str,
        _anchor_sha: &str,
        _target_path: &str,
        _range_start: u32,
        _range_end: u32,
        _fp: &str,
    ) -> Result<(), CacheError> {
        Ok(())
    }

    fn lookup_walk(
        &self,
        _key: &str,
        _page_path: &str,
        _log_output: &str,
    ) -> Result<Option<WalkRow>, CacheError> {
        Ok(None)
    }

    fn upsert_walk(
        &self,
        _key: &str,
        _page_path: &str,
        _log_output_sha: &str,
        _anchor_sha: &str,
        _path_at_commit: &str,
        _value: Option<&str>,
    ) -> Result<(), CacheError> {
        Ok(())
    }

    fn clear(&self) -> Result<(), CacheError> {
        Ok(())
    }
}
