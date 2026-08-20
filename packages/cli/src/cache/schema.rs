//! Schema, probe, and open ordering for the anchor cache (plan decisions
//! 3–4; the git-span reference shape is recorded in the similar-implementation
//! note).
//!
//! ## Storage layout (binding)
//!
//! Database file: `<common-dir>/wiki/anchor-cache.sqlite`; init lock:
//! `<common-dir>/wiki/anchor-cache.init.lock` (0-byte, fs4
//! `try_lock_exclusive` no-wait, held only during probe/quarantine/DDL).
//! The lock is never deleted by the open path, and only by `clear()`: it
//! lives inside the deleted directory (plan decision 2), so clearing the
//! cache inherently removes it — `clear()` unlinks it while still holding
//! the flock (plan decision 8). One repository — plain checkout or any
//! linked worktree — resolves one common dir, so worktrees share one cache.
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

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, ErrorCode, OpenFlags};

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

/// Seed the meta singleton on fresh create — idempotent (`OR IGNORE`), so
/// re-running the seed over an already-seeded database is a no-op.
/// `created_at` is a unix timestamp (INTEGER, matching the git-span reference
/// shape).
pub const META_INSERT_SQL: &str = "INSERT OR IGNORE INTO meta (id, application_id, schema_version, semantic_epoch, created_at)
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

/// Probe `db_path` read-only and classify it (plan decision 4): a missing
/// file is [`ProbeOutcome::Missing`] — checked before any open, so a probe
/// never creates the file — then [`PROBE_SQL`] (the liveness leg) and
/// [`META_VERIFY_SQL`] (the identity leg). `SQLITE_BUSY` on either leg
/// surfaces as `Err` for the caller's bounded wrapper to retry — a busy
/// database is healthy, never suspect. A readable database that is not our
/// shape (meta row absent or differing, or no meta table at all) is
/// [`SuspectKind::MetaMismatch`]: foreign files are never adopted.
pub fn probe(db_path: &Path) -> Result<ProbeOutcome, CacheError> {
    if !db_path.exists() {
        return Ok(ProbeOutcome::Missing);
    }
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    // Liveness leg: forces SQLite to read the header and page 1, which is
    // where a non-database or truncated file announces itself.
    match conn.query_row(PROBE_SQL, [], |_| Ok(())) {
        Ok(()) => {}
        Err(rusqlite::Error::SqliteFailure(f, msg)) => match f.code {
            ErrorCode::NotADatabase => {
                return Ok(ProbeOutcome::Suspect(SuspectKind::NotADatabase));
            }
            ErrorCode::DatabaseCorrupt => {
                return Ok(ProbeOutcome::Suspect(SuspectKind::Corrupt));
            }
            _ => return Err(CacheError::Sqlite(rusqlite::Error::SqliteFailure(f, msg))),
        },
        Err(e) => return Err(e.into()),
    }
    // Identity leg: the file is a readable database — is it ours?
    let meta = conn.query_row(META_VERIFY_SQL, [], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
        ))
    });
    match meta {
        Ok((application_id, schema_version, semantic_epoch))
            if application_id == APPLICATION_ID
                && schema_version == SCHEMA_VERSION
                && semantic_epoch == SEMANTIC_EPOCH =>
        {
            Ok(ProbeOutcome::Valid)
        }
        Ok(_) => Ok(ProbeOutcome::Suspect(SuspectKind::MetaMismatch)),
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            Ok(ProbeOutcome::Suspect(SuspectKind::MetaMismatch))
        }
        Err(rusqlite::Error::SqliteFailure(f, msg)) => match f.code {
            ErrorCode::NotADatabase => Ok(ProbeOutcome::Suspect(SuspectKind::NotADatabase)),
            ErrorCode::DatabaseCorrupt => Ok(ProbeOutcome::Suspect(SuspectKind::Corrupt)),
            ErrorCode::DatabaseBusy => Err(CacheError::Sqlite(rusqlite::Error::SqliteFailure(
                f, msg,
            ))),
            _ => Ok(ProbeOutcome::Suspect(SuspectKind::MetaMismatch)),
        },
        Err(e) => Err(e.into()),
    }
}

/// Quarantine a suspect database: rename it aside with a timestamp, delete
/// the `-wal`/`-shm` companions, and create a fresh, fully seeded file in
/// its place (plan decision 4). Runs under the init lock, at most once per
/// run — the caller does not loop.
///
/// The first step is a TOCTOU re-probe: between the open path's probe and
/// this quarantine's turn under the lock, another process may have recreated
/// the file — a file that is now valid must never be renamed aside.
pub fn quarantine(db_path: &Path) -> Result<(), CacheError> {
    // TOCTOU re-probe of the rename target (plan decision 4).
    if probe(db_path)? == ProbeOutcome::Valid {
        return Ok(());
    }

    // Rename aside with a timestamp, preserving the suspect content.
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_nanos();
    let aside = db_path.with_file_name(format!("{DB_FILE_NAME}.{stamp}.quarantine"));
    fs::rename(db_path, &aside)?;

    // Delete the -wal/-shm companions (NotFound is fine — they may never
    // have existed); a fresh file must start with no stale sidecars.
    for sidecar in [format!("{DB_FILE_NAME}-wal"), format!("{DB_FILE_NAME}-shm")] {
        let path = db_path.with_file_name(sidecar);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }

    // Fresh create: the binding open order seeds the full schema and the
    // meta singleton. The connection is dropped — the caller opens its own.
    drop(open_connection(db_path)?);
    Ok(())
}

/// The bounded `SQLITE_BUSY` retry wrapper (plan decision 6): at most five
/// attempts with a short linear backoff (10 ms × attempt), retrying only
/// `SQLITE_BUSY` — which includes the WAL-switch pragma's BUSY that bypasses
/// the busy handler (spike S1). Anything else, or five consecutive BUSY
/// results, falls through as `Err`: the cache is unavailable for the run.
/// BUSY is never a quarantine trigger and never treated as corruption.
pub(crate) fn retry_busy<T>(
    mut run: impl FnMut() -> Result<T, CacheError>,
) -> Result<T, CacheError> {
    let mut last_busy = None;
    for attempt in 0..5 {
        match run() {
            Err(e @ CacheError::Sqlite(rusqlite::Error::SqliteFailure(f, _)))
                if f.code == ErrorCode::DatabaseBusy =>
            {
                last_busy = Some(e);
                std::thread::sleep(Duration::from_millis(10 * attempt as u64));
            }
            other => return other,
        }
    }
    // Five consecutive BUSY results: fall through to the fail-open contract
    // (plan decision 6) — the fifth BUSY is returned as the failure, the
    // cache is unavailable for the run. Never a panic, never a quarantine.
    Err(last_busy.expect("the loop only exits after five BUSY results"))
}

/// Open the database at `db_path` applying the binding open order (plan
/// decision 4): `busy_timeout(1000)` → `PRAGMA journal_mode = WAL` → the
/// three DDL statements → the meta singleton seed. The timeout is set before
/// the WAL switch (git-span's ordering invariant); the switch itself can
/// surface `SQLITE_BUSY` without consulting the busy handler (spike S1),
/// which is why the open path runs under the bounded retry wrapper.
pub fn open_connection(db_path: &Path) -> Result<Connection, CacheError> {
    let conn = Connection::open(db_path)?;
    conn.busy_timeout(Duration::from_millis(BUSY_TIMEOUT_MS as u64))?;
    conn.execute_batch("PRAGMA journal_mode = WAL;")?;
    for ddl in [META_DDL, FINGERPRINT_DDL, ANCHOR_WALK_DDL] {
        conn.execute_batch(ddl)?;
    }
    conn.execute(META_INSERT_SQL, params![APPLICATION_ID, SCHEMA_VERSION, SEMANTIC_EPOCH])?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// The fail-open witness (plan decision 6): five consecutive
    /// `SQLITE_BUSY` results return the fifth as `Err` — never a panic. A
    /// writer holding an exclusive lock makes every attempt fail fast
    /// (zero busy timeout on the victim), so the test runs inside the
    /// retry wrapper's own backoff window.
    #[test]
    fn retry_busy_falls_through_to_err_after_five_busy_results() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("witness.sqlite");

        let locker = Connection::open(&db).expect("open locker");
        locker.execute_batch("BEGIN EXCLUSIVE;").expect("take write lock");

        let victim = Connection::open(&db).expect("open victim");
        victim
            .busy_timeout(Duration::from_millis(0))
            .expect("set zero timeout");

        let result = retry_busy(|| {
            victim
                .execute_batch("CREATE TABLE witness (id INTEGER);")
                .map_err(CacheError::from)
        });
        let err = result.expect_err("five BUSY results must return Err, not panic");
        assert!(
            matches!(
                err,
                CacheError::Sqlite(rusqlite::Error::SqliteFailure(f, _)) if f.code == ErrorCode::DatabaseBusy
            ),
            "the fifth BUSY must be the returned error: {err:?}"
        );
    }
}
