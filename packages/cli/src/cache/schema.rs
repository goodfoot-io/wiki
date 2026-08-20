//! Schema, probe, and open ordering for the anchor cache (plan decisions
//! 3–4; the git-span reference shape is recorded in the similar-implementation
//! note).
//!
//! ## Storage layout (binding)
//!
//! Database file: `<common-dir>/wiki/anchor-cache.sqlite`; init lock:
//! `<common-dir>/wiki/anchor-cache.init.lock` (0-byte, fs4
//! `try_lock_exclusive` no-wait, held only during probe/quarantine/DDL,
//! never deleted). One repository — plain checkout or any linked worktree —
//! resolves one common dir, so worktrees share one cache.
//!
//! ## Open order (binding)
//!
//! 1. `busy_timeout(1000)` — set *before* any pragma that can contend
//!    (git-span's ordering invariant).
//! 2. `PRAGMA journal_mode = WAL` — the mode switch can surface
//!    `SQLITE_BUSY` without consulting the busy handler (spike S1); the
//!    bounded retry wrapper covers it, the init lock prevents the four-way
//!    cold-start race.
//! 3. Schema — the three `CREATE TABLE` statements below, idempotent, plus
//!    the meta singleton seed.
//!
//! ## Probe and quarantine (binding)
//!
//! The open path probes the file read-only — `SELECT count(*) FROM
//! sqlite_master` — before any write pragma. Quarantine triggers are
//! *exactly* `SQLITE_NOTADB`, `SQLITE_CORRUPT`, and a meta mismatch
//! (`application_id` / `schema_version` / `semantic_epoch` differing from
//! [`APPLICATION_ID`] / [`SCHEMA_VERSION`] / [`SEMANTIC_EPOCH`], or a
//! missing meta row); a missing file is a fresh create, not a quarantine.
//! `SQLITE_BUSY` is never a quarantine trigger — it is retried. Quarantine
//! (rename aside with a timestamp, delete `-wal`/`-shm` companions, create
//! fresh) runs under the no-wait init lock with a TOCTOU re-probe of the
//! rename target, and at most one recreate happens before the run gives up:
//! any error at any point disables the cache for the run.

use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::cache::CacheError;

/// `meta.application_id` — the ASCII bytes "waca" ("wiki anchor cache").
pub const APPLICATION_ID: i64 = 0x7761_6361;

/// The schema version this build reads and writes.
pub const SCHEMA_VERSION: i64 = 1;

/// Bump to invalidate all cached rows without a schema change.
pub const SEMANTIC_EPOCH: i64 = 1;

/// Busy timeout, set before the WAL pragma (git-span's ordering invariant).
pub const BUSY_TIMEOUT_MS: u32 = 1000;

/// Database file name under `<common-dir>/wiki/`.
pub const DB_FILE_NAME: &str = "anchor-cache.sqlite";

/// Init lock file name under `<common-dir>/wiki/`.
pub const INIT_LOCK_FILE_NAME: &str = "anchor-cache.init.lock";

/// The cache directory under a repository's common git dir.
pub fn cache_dir(common_dir: &Path) -> PathBuf {
    common_dir.join("wiki")
}

/// The database file path.
pub fn db_path(common_dir: &Path) -> PathBuf {
    cache_dir(common_dir).join(DB_FILE_NAME)
}

/// The init lock file path.
pub fn init_lock_path(common_dir: &Path) -> PathBuf {
    cache_dir(common_dir).join(INIT_LOCK_FILE_NAME)
}

/// Meta singleton row: identity columns plus creation time. The `id = 1`
/// CHECK keeps the table single-row by construction.
pub const META_DDL: &str = "CREATE TABLE IF NOT EXISTS meta (
    id              INTEGER PRIMARY KEY CHECK (id = 1),
    application_id  INTEGER NOT NULL,
    schema_version  INTEGER NOT NULL,
    semantic_epoch  INTEGER NOT NULL,
    created_at      INTEGER NOT NULL
) STRICT;";

/// Fingerprint tier (tier F): `key_digest` is the 64-hex sha256 of the
/// canonical (page, anchor_sha, target, start, end) tuple. Every row stores
/// its full canonical tuple plus a sha256 `row_digest` over (tuple + value),
/// so a served row is re-derived and re-verified — never trusted blind.
pub const FINGERPRINT_DDL: &str = "CREATE TABLE IF NOT EXISTS fingerprint (
    key_digest   TEXT    PRIMARY KEY,
    page_path    TEXT    NOT NULL,
    anchor_sha   TEXT    NOT NULL,
    target_path  TEXT    NOT NULL,
    range_start  INTEGER NOT NULL,
    range_end    INTEGER NOT NULL,
    fp           TEXT    NOT NULL,
    row_digest   BLOB    NOT NULL
) STRICT;";

/// Anchor-walk tier (tier A): `key_digest` is the 64-hex sha256 of the
/// canonical (page, log_output) tuple — the exact untrimmed walk input.
/// `value` is nullable: a field-less anchor commit is a legitimate cached
/// epoch, distinct from an empty value.
pub const ANCHOR_WALK_DDL: &str = "CREATE TABLE IF NOT EXISTS anchor_walk (
    key_digest     TEXT    PRIMARY KEY,
    page_path      TEXT    NOT NULL,
    log_output_sha TEXT    NOT NULL,
    anchor_sha     TEXT    NOT NULL,
    path_at_commit TEXT    NOT NULL,
    value          TEXT,
    row_digest     BLOB    NOT NULL
) STRICT;";

/// Seed the meta singleton on fresh create. `created_at` is a unix timestamp
/// (INTEGER, matching the git-span reference shape).
pub const META_INSERT_SQL: &str = "INSERT INTO meta (id, application_id, schema_version, semantic_epoch, created_at)
VALUES (1, ?, ?, ?, strftime('%s', 'now'));";

/// Read-only liveness probe, run before any write pragma.
pub const PROBE_SQL: &str = "SELECT count(*) FROM sqlite_master;";

/// Meta identity check; a missing row or any mismatched column is a
/// meta mismatch (quarantine trigger).
pub const META_VERIFY_SQL: &str =
    "SELECT application_id, schema_version, semantic_epoch FROM meta WHERE id = 1;";

/// Outcome of the read-only probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// The file is a healthy wiki anchor cache.
    Valid,
    /// The file does not exist — a fresh create, not a quarantine.
    Missing,
    /// The file exists but is suspect; see [`SuspectKind`].
    Suspect(SuspectKind),
}

/// Why a probed file is suspect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuspectKind {
    /// `SQLITE_NOTADB` — not a database at all.
    NotADatabase,
    /// `SQLITE_CORRUPT` — damaged database file.
    Corrupt,
    /// Meta row absent, or `application_id` / `schema_version` /
    /// `semantic_epoch` mismatch — a database written by a different tool or
    /// schema.
    MetaMismatch,
}

/// Probe `db_path` read-only and classify it (plan decision 4). `SQLITE_BUSY`
/// is retried by the caller's wrapper, never reported as a suspect kind.
///
/// P1 stub — Phase 1 opens read-only, runs [`PROBE_SQL`] then
/// [`META_VERIFY_SQL`] (missing file → [`ProbeOutcome::Missing`]).
pub fn probe(_db_path: &Path) -> Result<ProbeOutcome, CacheError> {
    Ok(ProbeOutcome::Missing)
}

/// Quarantine a suspect database: rename it aside with a timestamp, delete
/// the `-wal`/`-shm` companions, and create a fresh file in its place
/// (plan decision 4). Runs under the init lock, at most once per run.
///
/// P1 stub — Phase 1 implements the rename-aside + sidecar delete + recreate.
pub fn quarantine(_db_path: &Path) -> Result<(), CacheError> {
    Ok(())
}

/// Open the database at `db_path` applying the binding open order:
/// `busy_timeout(1000)` → `PRAGMA journal_mode = WAL` → schema + meta seed.
///
/// P1 stub — Phase 1 applies the ordering; the stub returns an in-memory
/// connection as the fresh-empty sentinel.
pub fn open_connection(_db_path: &Path) -> Result<Connection, CacheError> {
    Ok(Connection::open_in_memory()?)
}
