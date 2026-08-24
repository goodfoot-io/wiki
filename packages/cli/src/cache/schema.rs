//! Schema, probe, and open ordering for the consolidated wiki store
//! (plan decisions D1–D4).
//!
//! ## Storage layout (binding)
//!
//! Database file: `<common-dir>/wiki/store.sqlite`; init lock:
//! `<common-dir>/wiki/store.init.lock` (0-byte, fs4
//! `try_lock_exclusive` no-wait, held only during probe/quarantine/skew
//! repair/DDL). The lock is never deleted by the open path, and only by
//! `clear()`: it lives inside the deleted directory, so clearing the store
//! inherently removes it — `clear()` unlinks it while still holding the
//! flock. One repository — plain checkout or any linked worktree — resolves
//! one common dir, so every worktree shares one store. Every creation path
//! (the `wiki/` subtree, both lock files, the database file) goes through
//! [`crate::store::fd`] (plan D9).
//!
//! ## Open order (binding)
//!
//! 1. `busy_timeout(1000)` — set *before* any pragma that can contend; the
//!    WAL switch can surface `SQLITE_BUSY` without consulting the busy
//!    handler, and this ordering is empirically load-bearing (spike S1).
//! 2. `PRAGMA journal_mode = WAL`, then the reconciled durability policy
//!    (plan D4): `synchronous = NORMAL` and a 256 MiB `mmap_size`.
//! 3. Schema — the anchor tier's `CREATE TABLE` statements below,
//!    idempotent, plus the meta singleton seed.
//!
//! ## Probe: identity pair vs schema skew (binding)
//!
//! The open path probes the file read-only before any write pragma.
//! **Corruption-class** events quarantine the whole store: `SQLITE_NOTADB`,
//! `SQLITE_CORRUPT`, or a foreign identity pair — `meta.application_id` /
//! `meta.schema_version` differing from [`APPLICATION_ID`] /
//! [`SCHEMA_VERSION`], or a missing meta row/table. A missing *file* is a
//! fresh create, not a quarantine. `SQLITE_BUSY` is never a trigger — it is
//! retried.
//!
//! A matching identity pair with deviating **static** table shapes is
//! **schema skew, not corruption** (plan D2): the outcome is tier-scoped
//! structural invalidation ([`drop_tier_tables`] — drop that tier's static
//! tables plus, for [`Tier::Index`], every dynamic `fts_%` child), never
//! whole-store quarantine — one tier's evolution cannot discard the other
//! tier's data. Dynamic `fts_<gen_id>` tables are invisible to the registry
//! probe. The per-tier epochs in the meta singleton are consumer state:
//! they are checked after a valid probe, never by it.
//!
//! Quarantine (rename aside with a timestamp, delete `-wal`/`-shm`
//! companions, create fresh) runs under the no-wait init lock with a TOCTOU
//! re-probe of the rename target, and at most one recreate happens before
//! the run gives up: any error at any point disables the cache for the run.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, ErrorCode, OpenFlags, params};

use crate::cache::CacheError;

/// `meta.application_id` — identifies the consolidated wiki store. A
/// database whose identity pair does not match is foreign and quarantined.
pub const APPLICATION_ID: &str = "wkst";

/// The schema version this build reads and writes. Together with
/// [`APPLICATION_ID`] this forms the identity pair verified by
/// [`probe`].
pub const SCHEMA_VERSION: i64 = 1;

/// Seed value for the per-tier bulk-invalidation epochs (`anchor_epoch`,
/// `index_epoch`). Epochs are consumer state (plan D2): bumped to invalidate
/// one tier's rows, checked after a valid probe, never by it.
pub const INITIAL_TIER_EPOCH: i64 = 0;

/// Busy timeout, set before the WAL pragma (spike S1's ordering invariant).
pub const BUSY_TIMEOUT_MS: u32 = 1000;

/// The store directory name under the repository's common git dir.
pub const STORE_DIR_NAME: &str = "wiki";

/// Database file name under `<common-dir>/wiki/`.
pub const DB_FILE_NAME: &str = "store.sqlite";

/// Init lock file name under `<common-dir>/wiki/`.
pub const INIT_LOCK_FILE_NAME: &str = "store.init.lock";

/// Rendezvous lock file name under `<common-dir>/wiki/`
/// ([`crate::cache::rendezvous`], plan D7).
pub const RENDEZVOUS_LOCK_FILE_NAME: &str = "rendezvous.lock";

/// The store directory under a repository's common git dir.
pub fn store_dir(common_dir: &Path) -> PathBuf {
    common_dir.join(STORE_DIR_NAME)
}

/// The store directory under a repository's common git dir (historical
/// name kept for the clear-cache path printer in `commands/check.rs`).
pub fn cache_dir(common_dir: &Path) -> PathBuf {
    store_dir(common_dir)
}

/// The database file path.
pub fn db_path(common_dir: &Path) -> PathBuf {
    store_dir(common_dir).join(DB_FILE_NAME)
}

/// The init lock file path.
pub fn init_lock_path(common_dir: &Path) -> PathBuf {
    store_dir(common_dir).join(INIT_LOCK_FILE_NAME)
}

/// The rendezvous lock file path.
pub fn rendezvous_lock_path(common_dir: &Path) -> PathBuf {
    store_dir(common_dir).join(RENDEZVOUS_LOCK_FILE_NAME)
}

/// Which tier of the consolidated store a table, epoch, or invalidation
/// belongs to (plan D2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// Tier A/F: the anchor cache (`fingerprint`, `anchor_walk`).
    Anchor,
    /// The index cache (`generations`, `gen_paths`, `blobs`, dynamic
    /// `fts_<gen_id>` children); owned by the generations store.
    Index,
}

/// Meta singleton row: the identity pair plus the per-tier
/// bulk-invalidation epochs (plan D2). The `id = 1` CHECK keeps the table
/// single-row by construction.
pub const META_DDL: &str = "CREATE TABLE IF NOT EXISTS meta (
    id              INTEGER PRIMARY KEY CHECK (id = 1),
    application_id  TEXT    NOT NULL,
    schema_version  INTEGER NOT NULL,
    anchor_epoch    INTEGER NOT NULL,
    index_epoch     INTEGER NOT NULL
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
/// re-running the seed over an already-seeded database is a no-op. Both tier
/// epochs start at [`INITIAL_TIER_EPOCH`].
pub const META_INSERT_SQL: &str =
    "INSERT OR IGNORE INTO meta (id, application_id, schema_version, anchor_epoch, index_epoch)
VALUES (1, ?, ?, ?, ?);";

/// Read-only liveness probe, run before any write pragma.
pub const PROBE_SQL: &str = "SELECT count(*) FROM sqlite_master;";

/// Meta identity check; a missing row or a mismatched identity column is a
/// meta mismatch (quarantine trigger). The per-tier epochs are deliberately
/// absent — consumer state, never probe state (plan D2).
pub const META_VERIFY_SQL: &str = "SELECT application_id, schema_version FROM meta WHERE id = 1;";

/// Outcome of the read-only probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// The file is a healthy wiki store.
    Valid,
    /// The file does not exist — a fresh create, not a quarantine.
    Missing,
    /// The file is corruption-class suspect; see [`SuspectKind`]. Quarantine.
    Suspect(SuspectKind),
    /// A matching identity pair with deviating static table shapes for one
    /// tier — schema skew (plan D2): tier-scoped structural invalidation
    /// ([`drop_tier_tables`]), never whole-store quarantine.
    Skew(Tier),
}

/// Why a probed file is corruption-class suspect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuspectKind {
    /// `SQLITE_NOTADB` — not a database at all.
    NotADatabase,
    /// `SQLITE_CORRUPT` — damaged database file.
    Corrupt,
    /// Meta row absent, or the identity pair (`application_id`,
    /// `schema_version`) differs from [`APPLICATION_ID`] /
    /// [`SCHEMA_VERSION`] — a database written by a different tool or an
    /// incompatible schema version.
    MetaMismatch,
}

/// One registry entry: a binding static table and its required columns
/// `(name, declared type, nullable, primary key)` (plan D3).
type ColumnSpec = (&'static str, &'static str, bool, bool);

/// The anchor tier's static-table registry entries (plan D3).
const ANCHOR_REGISTRY: &[(&str, &[ColumnSpec])] = &[
    ("fingerprint", FINGERPRINT_COLUMNS),
    ("anchor_walk", WALK_COLUMNS),
];

/// The index tier's static-table registry entries. Registered by the
/// generations store (plan Phase 3; orchestrator-authorized data addition):
/// the shapes mirror `index::generations`' binding DDL exactly — column
/// order, declared types, nullability, and primary-key membership. Dynamic
/// `fts_<gen_id>` children are invisible to this registry by construction.
const INDEX_REGISTRY: &[(&str, &[ColumnSpec])] = &[
    ("generations", GENERATIONS_COLUMNS),
    ("gen_paths", GEN_PATHS_COLUMNS),
    ("blobs", BLOBS_COLUMNS),
];

/// The index tier's binding static table names, dropped by
/// [`drop_tier_tables`] alongside every dynamic `fts_%` child (plan D2).
/// The index tier's static DDL — the exact text [`crate::index::generations`]
/// pins as its binding contract artifact (checkpoint-approved): the three
/// registry tables plus their two indexes. Dynamic `fts_<gen_id>` children
/// are deliberately absent: they are publish-time, per-generation state
/// (plan D5), created and dropped by the generations store alone. Applied
/// by [`open_connection`] so a fresh store always materializes both tiers'
/// statics (orchestrator-authorized Phase 3 wiring).
pub(crate) const INDEX_TIER_DDL: &str = "
CREATE TABLE IF NOT EXISTS generations (
    gen_id          INTEGER PRIMARY KEY NOT NULL,
    digest          BLOB    NOT NULL UNIQUE CHECK (length(digest) = 32),
    head_oid        TEXT    NOT NULL CHECK (length(head_oid) = 40),
    head_tree_oid   TEXT    NOT NULL CHECK (head_tree_oid = '' OR length(head_tree_oid) = 40),
    index_checksum  BLOB    NOT NULL CHECK (length(index_checksum) = 20),
    wikiignore_hash BLOB    NOT NULL CHECK (length(wikiignore_hash) = 20),
    worktree_sig    BLOB    NOT NULL CHECK (length(worktree_sig) = 32),
    publisher       TEXT,
    created_at      INTEGER NOT NULL DEFAULT 0,
    access_bucket   INTEGER NOT NULL DEFAULT 0,
    blob_count      INTEGER NOT NULL CHECK (blob_count >= 0)
) STRICT;

CREATE TABLE IF NOT EXISTS gen_paths (
    gen_id        INTEGER NOT NULL REFERENCES generations(gen_id),
    source        TEXT    NOT NULL CHECK (source IN ('tree', 'index', 'worktree')),
    path_rel      TEXT    NOT NULL,
    oid           TEXT    NOT NULL REFERENCES blobs(oid),
    parent_dir    TEXT    NOT NULL,
    stat_mtime_ns INTEGER,
    PRIMARY KEY (gen_id, source, path_rel)
) STRICT;

CREATE INDEX IF NOT EXISTS idx_gen_paths_oid ON gen_paths(oid);
CREATE INDEX IF NOT EXISTS idx_gen_paths_parent ON gen_paths(gen_id, parent_dir, source);

CREATE TABLE IF NOT EXISTS blobs (
    oid           TEXT PRIMARY KEY,
    refcount      INTEGER NOT NULL,
    title         TEXT    NOT NULL,
    summary       TEXT    NOT NULL,
    body          TEXT    NOT NULL,
    aliases_text  TEXT    NOT NULL,
    tags_text     TEXT    NOT NULL,
    keywords_text TEXT    NOT NULL
) STRICT;
";

const INDEX_STATIC_TABLES: &[&str] = &["generations", "gen_paths", "blobs"];

fn registry_for(tier: Tier) -> &'static [(&'static str, &'static [ColumnSpec])] {
    match tier {
        Tier::Anchor => ANCHOR_REGISTRY,
        Tier::Index => INDEX_REGISTRY,
    }
}

const FINGERPRINT_COLUMNS: &[(&str, &str, bool, bool)] = &[
    ("key_digest", "TEXT", false, true),
    ("page_path", "TEXT", false, false),
    ("anchor_sha", "TEXT", false, false),
    ("target_path", "TEXT", false, false),
    ("range_start", "INTEGER", false, false),
    ("range_end", "INTEGER", false, false),
    ("fp", "TEXT", false, false),
    ("row_digest", "BLOB", false, false),
];
const WALK_COLUMNS: &[(&str, &str, bool, bool)] = &[
    ("key_digest", "TEXT", false, true),
    ("page_path", "TEXT", false, false),
    ("log_output_sha", "TEXT", false, false),
    ("anchor_sha", "TEXT", false, false),
    ("path_at_commit", "TEXT", false, false),
    ("value", "TEXT", true, false),
    ("row_digest", "BLOB", false, false),
];
/// Index tier: `generations` (PK is just `gen_id`; digest uniqueness and
/// the sentinel-length CHECKs live in the DDL, which the registry does not
/// model).
const GENERATIONS_COLUMNS: &[(&str, &str, bool, bool)] = &[
    ("gen_id", "INTEGER", false, true),
    ("digest", "BLOB", false, false),
    ("head_oid", "TEXT", false, false),
    ("head_tree_oid", "TEXT", false, false),
    ("index_checksum", "BLOB", false, false),
    ("wikiignore_hash", "BLOB", false, false),
    ("worktree_sig", "BLOB", false, false),
    ("publisher", "TEXT", true, false),
    ("created_at", "INTEGER", false, false),
    ("access_bucket", "INTEGER", false, false),
    ("blob_count", "INTEGER", false, false),
];
/// Index tier: `gen_paths`, composite PK `(gen_id, source, path_rel)`.
const GEN_PATHS_COLUMNS: &[(&str, &str, bool, bool)] = &[
    ("gen_id", "INTEGER", false, true),
    ("source", "TEXT", false, true),
    ("path_rel", "TEXT", false, true),
    ("oid", "TEXT", false, false),
    ("parent_dir", "TEXT", false, false),
    ("stat_mtime_ns", "INTEGER", true, false),
];
/// Index tier: `blobs`, PK `oid`; content-addressed and shared across
/// generations.
const BLOBS_COLUMNS: &[(&str, &str, bool, bool)] = &[
    ("oid", "TEXT", false, true),
    ("refcount", "INTEGER", false, false),
    ("title", "TEXT", false, false),
    ("summary", "TEXT", false, false),
    ("body", "TEXT", false, false),
    ("aliases_text", "TEXT", false, false),
    ("tags_text", "TEXT", false, false),
    ("keywords_text", "TEXT", false, false),
];

fn table_matches(
    conn: &Connection,
    table: &str,
    expected: &[(&str, &str, bool, bool)],
) -> Result<bool, rusqlite::Error> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)? == 0,
                row.get::<_, i64>(5)? != 0,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows.len() == expected.len()
        && rows
            .iter()
            .zip(expected)
            .all(|((name, ty, nullable, pk), expected)| {
                name == expected.0
                    && ty.eq_ignore_ascii_case(expected.1)
                    && *nullable == expected.2
                    && *pk == expected.3
            }))
}

/// Probe `db_path` read-only and classify it: a missing file is
/// [`ProbeOutcome::Missing`] — checked before any open, so a probe never
/// creates the file — then [`PROBE_SQL`] (the liveness leg), the identity
/// pair (the identity leg), and the per-tier static-table registry (plan
/// D3). `SQLITE_BUSY` on any leg surfaces as `Err` for the caller's bounded
/// wrapper to retry — a busy database is healthy, never suspect. A readable
/// database that is not ours (meta row absent or identity pair differing,
/// or no meta table at all) is [`SuspectKind::MetaMismatch`]: foreign files
/// are never adopted. A matching identity pair with deviating static shapes
/// is [`ProbeOutcome::Skew`] for the owning tier — structural invalidation,
/// not corruption.
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
    // Identity leg: the file is a readable database — is it ours? The pair
    // only; epochs are consumer state (plan D2).
    let meta = conn.query_row(META_VERIFY_SQL, [], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    });
    match meta {
        Ok((application_id, schema_version))
            if application_id == APPLICATION_ID && schema_version == SCHEMA_VERSION =>
        {
            // Registry leg: verify every registered tier's static tables;
            // dynamic `fts_%` children are invisible here by construction.
            for (tier, tables) in [(Tier::Anchor, ANCHOR_REGISTRY), (Tier::Index, INDEX_REGISTRY)]
            {
                for (name, expected) in tables {
                    if !table_matches(&conn, name, expected)? {
                        return Ok(ProbeOutcome::Skew(tier));
                    }
                }
            }
            Ok(ProbeOutcome::Valid)
        }
        Ok(_) => Ok(ProbeOutcome::Suspect(SuspectKind::MetaMismatch)),
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            Ok(ProbeOutcome::Suspect(SuspectKind::MetaMismatch))
        }
        Err(rusqlite::Error::SqliteFailure(f, msg)) => match f.code {
            ErrorCode::NotADatabase => Ok(ProbeOutcome::Suspect(SuspectKind::NotADatabase)),
            ErrorCode::DatabaseCorrupt => Ok(ProbeOutcome::Suspect(SuspectKind::Corrupt)),
            ErrorCode::DatabaseBusy => {
                Err(CacheError::Sqlite(rusqlite::Error::SqliteFailure(f, msg)))
            }
            _ => Ok(ProbeOutcome::Suspect(SuspectKind::MetaMismatch)),
        },
        Err(e) => Err(e.into()),
    }
}

/// Tier-scoped structural invalidation (plan D2): drop `tier`'s static
/// registry tables plus — for [`Tier::Index`] — every dynamic `fts_%` child
/// table. Rows die with their tables, so this one primitive serves schema-skew
/// repair, tier-epoch bulk invalidation, and the later tier-scoped
/// clear-cache. Dropping one tier's tables never touches the other tier's.
pub fn drop_tier_tables(conn: &Connection, tier: Tier) -> Result<(), CacheError> {
    for (name, _) in registry_for(tier) {
        conn.execute_batch(&format!("DROP TABLE IF EXISTS \"{name}\";"))?;
    }
    if tier == Tier::Index {
        // Static tables by name (the index tier's registration in the
        // verified-shape registry lands with the generations store) plus
        // every dynamic `fts_<gen_id>` child. The `\_` escape keeps the
        // pattern on the literal underscore: `fts_%` unescaped would also
        // match names like `ftsX`.
        for name in INDEX_STATIC_TABLES {
            conn.execute_batch(&format!("DROP TABLE IF EXISTS \"{name}\";"))?;
        }
        let mut stmt = conn.prepare(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name LIKE 'fts\\_%' ESCAPE '\\';",
        )?;
        let names = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        for name in names {
            let quoted = name.replace('"', "\"\"");
            conn.execute_batch(&format!("DROP TABLE IF EXISTS \"{quoted}\";"))?;
        }
    }
    Ok(())
}

/// Route the database file's creation through the descriptor-hardened
/// primitives (plan D9): validate the containing private subtree and create
/// the file owner-only with symlink refusal. An existing file is opened,
/// never truncated — SQLite adopts it as-is.
pub(crate) fn prepare_db_file(db_path: &Path) -> Result<(), CacheError> {
    let dir = db_path.parent().expect("db path has a parent");
    let dir_fd = crate::store::fd::DirFd::open(dir)?;
    dir_fd.validate_private()?;
    dir_fd.create_file(DB_FILE_NAME)?;
    Ok(())
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
    // meta singleton. The file is pre-created through the hardened
    // primitives; the connection is dropped — the caller opens its own.
    prepare_db_file(db_path)?;
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
/// D4): `busy_timeout(1000)` → `PRAGMA journal_mode = WAL` →
/// `synchronous = NORMAL` → `mmap_size = 268435456` → the anchor tier's DDL
/// statements → the meta singleton seed. The timeout is set before the WAL
/// switch (spike S1's empirically load-bearing ordering invariant); the
/// switch itself can surface `SQLITE_BUSY` without consulting the busy
/// handler, which is why the open path runs under the bounded retry wrapper.
pub fn open_connection(db_path: &Path) -> Result<Connection, CacheError> {
    let conn = Connection::open(db_path)?;
    conn.busy_timeout(Duration::from_millis(BUSY_TIMEOUT_MS as u64))?;
    conn.execute_batch("PRAGMA journal_mode = WAL;")?;
    conn.execute_batch("PRAGMA synchronous = NORMAL; PRAGMA mmap_size = 268435456;")?;
    for ddl in [META_DDL, FINGERPRINT_DDL, ANCHOR_WALK_DDL, INDEX_TIER_DDL] {
        conn.execute_batch(ddl)?;
    }
    conn.execute(
        META_INSERT_SQL,
        params![
            APPLICATION_ID,
            SCHEMA_VERSION,
            INITIAL_TIER_EPOCH,
            INITIAL_TIER_EPOCH
        ],
    )?;
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
        locker
            .execute_batch("BEGIN EXCLUSIVE;")
            .expect("take write lock");

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
