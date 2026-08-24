//! The index freshness tier of the merged store: immutable generations
//! keyed by canonical state digests (plan [merged-store-generations] D5,
//! round-2 revision).
//!
//! One generation is a complete, frozen snapshot of the wiki index for one
//! canonical state: `(head_oid, head_tree_oid, index_checksum,
//! wikiignore_hash, worktree_sig)`. Publishing never deletes anything —
//! immutability means no physical row deletion can ever invalidate a
//! retained generation — so N linked worktrees sharing one store file can
//! each keep their own warm generation without stomping each other.
//!
//! # Store-foundation wiring (peer-delivered, live)
//!
//! * Store location — `crate::cache::schema::{STORE_DIR_NAME, DB_FILE_NAME,
//!   store_dir(common_dir), db_path(common_dir)}` →
//!   `<common>/wiki/store.sqlite`.
//! * Open order — [`crate::cache::schema::open_connection`] applies
//!   `busy_timeout(1000)` **before** `journal_mode=WAL`,
//!   `synchronous=NORMAL`, `mmap_size=256MB`, the meta singleton, and the
//!   anchor tier DDL; this module adds its own static DDL idempotently on
//!   top, and every write transaction wraps in
//!   `crate::cache::schema::retry_busy`.
//! * Creation paths — the `wiki/` subtree, init lock, and database file go
//!   through `crate::store::fd` hardening exactly like the anchor tier's
//!   open path.
//! * Meta singleton — `(application_id='wkst', schema_version=1,
//!   anchor_epoch, index_epoch)`; **this module owns reading/enforcing
//!   `index_epoch`** (mismatch ⇒ tier-scoped bulk invalidation via
//!   `drop_tier_tables(Tier::Index)`, never a quarantine).
//! * Table names — static tier tables are exactly `generations`,
//!   `gen_paths`, `blobs`; dynamic FTS children are exactly `fts_{gen_id}`
//!   (prefix `fts_`) — the peer's tier-scoped invalidation/clear drops
//!   precisely these names.
//! * Rendezvous (Phase 4 wiring) —
//!   `crate::cache::rendezvous::{acquire_shared, acquire_exclusive}`.
//!
//! # DDL (binding contract artifact, checkpoint-approved)
//!
//! Static tables (created by [`GenerationsStore::open`] inside a
//! `retry_busy`-wrapped transaction; registry-visible to the peer's probe):
//!
//! ```sql
//! CREATE TABLE IF NOT EXISTS generations (
//!     gen_id          INTEGER PRIMARY KEY,
//!     digest          BLOB    NOT NULL UNIQUE CHECK (length(digest) = 32),
//!     head_oid        TEXT    NOT NULL CHECK (length(head_oid) = 40),
//!     head_tree_oid   TEXT    NOT NULL CHECK (head_tree_oid = '' OR length(head_tree_oid) = 40),
//!     index_checksum  BLOB    NOT NULL CHECK (length(index_checksum) = 20),
//!     wikiignore_hash BLOB    NOT NULL CHECK (length(wikiignore_hash) = 20),
//!     worktree_sig    BLOB    NOT NULL CHECK (length(worktree_sig) = 32),
//!     publisher       TEXT,
//!     created_at      INTEGER NOT NULL,
//!     access_bucket   INTEGER NOT NULL DEFAULT 0,
//!     blob_count      INTEGER NOT NULL CHECK (blob_count >= 0)
//! ) STRICT;
//!
//! CREATE TABLE IF NOT EXISTS gen_paths (
//!     gen_id        INTEGER NOT NULL REFERENCES generations(gen_id),
//!     source        TEXT    NOT NULL CHECK (source IN ('tree', 'index', 'worktree')),
//!     path_rel      TEXT    NOT NULL,
//!     oid           TEXT    NOT NULL REFERENCES blobs(oid),
//!     parent_dir    TEXT    NOT NULL,
//!     stat_mtime_ns INTEGER,
//!     PRIMARY KEY (gen_id, source, path_rel)
//! ) STRICT;
//!
//! CREATE INDEX IF NOT EXISTS idx_gen_paths_oid
//!     ON gen_paths(oid);
//! CREATE INDEX IF NOT EXISTS idx_gen_paths_parent
//!     ON gen_paths(gen_id, parent_dir, source);
//!
//! CREATE TABLE IF NOT EXISTS blobs (
//!     oid           TEXT PRIMARY KEY,
//!     refcount      INTEGER NOT NULL,
//!     title         TEXT    NOT NULL,
//!     summary       TEXT    NOT NULL,
//!     body          TEXT    NOT NULL,
//!     aliases_text  TEXT    NOT NULL,
//!     tags_text     TEXT    NOT NULL,
//!     keywords_text TEXT    NOT NULL
//! ) STRICT;
//! ```
//!
//! Checkpoint rulings baked in: parameterized widths expressed as plain
//! types + `CHECK(length(...))` (STRICT forbids type parameters); `source`
//! is a self-describing TEXT literal (`'tree'|'index'|'worktree'`) rather
//! than a positional integer (hidden coupling №5 eliminated); `created_at`
//! is unix epoch **milliseconds** monotonically bumped inside publish
//! (`max(now, previous + 1)`) so retention ordering is total;
//! `access_bucket = created_at / 3_600_000`. `blobs.refcount` counts
//! retained-generation memberships and is reconciled set-based inside every
//! transaction that mutates `gen_paths`: after each publish/GC commit,
//! `refcount == COUNT(DISTINCT gen_id)` over `gen_paths` per oid holds by
//! construction.
//!
//! Dynamic per-generation FTS children (created/dropped at publish/GC;
//! invisible to the registry). Standalone fts5 — not external-content —
//! because global triggers cannot target per-generation tables; served BM25
//! statistics are therefore exactly what a cold rebuild of the same digest
//! produces:
//!
//! ```sql
//! CREATE VIRTUAL TABLE fts_{gen_id} USING fts5(
//!     title, aliases_text, tags_text, keywords_text, summary, body,
//!     tokenize='unicode61 remove_diacritics 2',
//!     prefix='2 3 4'
//! );
//!
//! -- Populated inside the publish write transaction, over members only.
//! -- Column ORDER is load-bearing: search.rs issues
//! -- bm25(fts_{g}, 5, 4, 3, 3, 2, 1) matching the served weights.
//! INSERT INTO fts_{gen_id}(rowid, title, aliases_text, tags_text,
//!                          keywords_text, summary, body)
//! SELECT b.rowid, b.title, b.aliases_text, b.tags_text, b.keywords_text,
//!        b.summary, b.body
//! FROM blobs b
//! WHERE b.oid IN (SELECT oid FROM gen_paths WHERE gen_id = {gen_id});
//! ```
//!
//! # Digest canonicalization encoding (binding contract artifact)
//!
//! The anchor cache's provably injective length-tagged framing
//! (`crate::cache::key` discipline):
//!
//! ```text
//! frame(bytes) = u64_le(byte_len) ++ bytes
//! frame(str)   = u64_le(utf8_len) ++ utf8_bytes
//!
//! canonical(fingerprint) =
//!     frame(head_oid) ++ frame(head_tree_oid) ++ frame(index_checksum)
//!   ++ frame(wikiignore_hash) ++ frame(worktree_sig)
//!
//! digest      = SHA-256(canonical)     // raw 32 bytes, stored in `generations.digest`
//! digest_hex  = sha256_hex(canonical)  // diagnostics/logging form only
//!
//! worktree_sig =
//!     SHA-256( frame(path₁) ++ frame(mtime₁.to_string()) ++ … )
//! ```
//!
//! Field order is fixed as listed; every field is framed uniformly —
//! including the fixed-width binary fields — so boundary splits across
//! adjacent fields (`("ab","c")` vs `("a","bc")`) can never collide.
//! `mtime_ns` contributes its decimal ASCII representation (the same
//! discipline key.rs uses for numeric range fields). [`worktree_signature`]
//! sorts its input pairs ascending by `(path, mtime_ns)` itself — the walk
//! order must not leak into the signature.
//!
//! Sentinels carried over verbatim from the single-row model (hidden
//! couplings №2/№3 made explicit and typed): unborn HEAD
//! [`UNBORN_HEAD_OID`], empty-tree diff base [`EMPTY_TREE_BASE`], 20-zero
//! [`ZERO_INDEX_CHECKSUM`] / [`ZERO_WIKIIGNORE_HASH`]. The fresh-zero guard
//! dissolves — no matching generation simply means a miss.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use fs4::fs_std::FileExt;
use rusqlite::{Connection, OptionalExtension};
use sha2::{Digest, Sha256};

use crate::cache::diagnostics;
use crate::cache::schema::{self, ProbeOutcome, Tier, retry_busy};
use crate::cache::CacheError;
use crate::store::fd::DirFd;

use super::ingest::WikiBlobFields;
use super::{BlobOid, Source};

/// HEAD oid sentinel for an unborn branch — `"0".repeat(40)`, byte-identical
/// to freshness's ZERO_OID and refresh's emission (coupling №2, now a named
/// constant both sides use).
pub const UNBORN_HEAD_OID: &str = "0000000000000000000000000000000000000000";

/// `head_tree_oid` sentinel: an empty string means "diff against the empty
/// tree" (first commit / no commits yet).
pub const EMPTY_TREE_BASE: &str = "";

/// 20-zero git-index checksum trailer sentinel (absent `.git/index`).
pub const ZERO_INDEX_CHECKSUM: [u8; 20] = [0u8; 20];

/// 20-zero wikiignore SHA-1 sentinel (no wikiignore file in this state).
#[allow(dead_code)] // consumed alongside the tier-scoped bump (clear-cache wave)
pub const ZERO_WIKIIGNORE_HASH: [u8; 20] = [0u8; 20];

/// Retention bound (plan D10): after the always-live newest generation, the
/// next-newest 8 candidates survive eviction — a recency-liveness rule where
/// being looked up keeps a worktree's generation warm.
pub const RETAINED_GENERATIONS: usize = 8;

/// Milliseconds per hour — the `access_bucket` granularity.
const MILLIS_PER_HOUR: i64 = 3_600_000;

/// The five canonical freshness inputs of one repository state. Exactly the
/// tuple the canonical digest hashes; exactly the prefix columns of the
/// `generations` row (minus derived bookkeeping).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateFingerprint {
    /// 40-hex commit oid, or [`UNBORN_HEAD_OID`].
    pub head_oid: String,
    /// 40-hex tree oid of HEAD, or [`EMPTY_TREE_BASE`].
    pub head_tree_oid: String,
    /// SHA-1 trailer of `.git/index` (last 20 bytes), or
    /// [`ZERO_INDEX_CHECKSUM`].
    pub index_checksum: [u8; 20],
    /// SHA-1 of the wikiignore file contents, or [`ZERO_WIKIIGNORE_HASH`].
    pub wikiignore_hash: [u8; 20],
    /// [`worktree_signature`] over the walked `(path, mtime_ns)` pairs.
    pub worktree_sig: [u8; 32],
}

impl StateFingerprint {
    /// The canonical 32-byte digest over this fingerprint (encoding above).
    /// Injective over field values by the length-tagged framing.
    pub fn digest(&self) -> [u8; 32] {
        let mut buf = Vec::with_capacity(
            self.head_oid.len()
                + self.head_tree_oid.len()
                + 20
                + 20
                + 32
                + 5 * std::mem::size_of::<u64>(),
        );
        push_field(&mut buf, self.head_oid.as_bytes());
        push_field(&mut buf, self.head_tree_oid.as_bytes());
        push_field(&mut buf, &self.index_checksum);
        push_field(&mut buf, &self.wikiignore_hash);
        push_field(&mut buf, &self.worktree_sig);
        Sha256::digest(&buf).into()
    }

    /// Lowercase-hex diagnostics form of [`Self::digest`] (via
    /// `cache::key::sha256_hex`). Never used as a storage key.
    #[allow(dead_code)] // diagnostics/logging consumer lands with store events
    pub fn digest_hex(&self) -> String {
        crate::cache::key::sha256_hex(&self.digest())
    }
}

/// Append one length-tagged field: the byte length as a little-endian
/// `u64`, then the bytes. Byte-for-byte the framing of
/// `crate::cache::key::push_field` (which stays private to its module);
/// kept adjacent to this module's encoding spec so the two cannot drift
/// apart silently — the checkpoint contract pins them as equal.
fn push_field(out: &mut Vec<u8>, field: &[u8]) {
    let len = field.len() as u64;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(field);
}

/// Canonical worktree signature: SHA-256 over the sorted
/// `(repo-relative dir-or-md path, mtime_ns)` pairs of directories plus
/// `*.md` files (the `.git`/store prunes unchanged), replacing the old i64
/// `DefaultHasher` fold in `freshness::compute_worktree_dir_hash`. Sorts
/// internally — caller order never leaks into the signature.
pub fn worktree_signature(pairs: &[(String, i64)]) -> [u8; 32] {
    let mut sorted: Vec<(&str, i64)> =
        pairs.iter().map(|(p, m)| (p.as_str(), *m)).collect();
    sorted.sort_unstable();
    let mut buf = Vec::with_capacity(sorted.len() * 24);
    for (path, mtime) in sorted {
        push_field(&mut buf, path.as_bytes());
        // Numeric fields contribute their decimal ASCII representation,
        // framed like every other field (the key.rs numeric precedent).
        push_field(&mut buf, mtime.to_string().as_bytes());
    }
    Sha256::digest(&buf).into()
}

/// The SQL literal a [`Source`] is stored under in `gen_paths.source`
/// (`CHECK (source IN ('tree','index','worktree'))`). The single mapping
/// site between the merge-order enum and its storage form.
pub(crate) fn source_sql(source: Source) -> &'static str {
    match source {
        Source::Tree => "tree",
        Source::Index => "index",
        Source::Worktree => "worktree",
    }
}

/// Inverse of [`source_sql`]; `None` for anything outside the CHECK set.
pub(crate) fn source_from_sql(literal: &str) -> Option<Source> {
    match literal {
        "tree" => Some(Source::Tree),
        "index" => Some(Source::Index),
        "worktree" => Some(Source::Worktree),
        _ => None,
    }
}

/// One resolved path row of one generation — the reverse `(source, path) →
/// blob` mapping that replaces the old global mutable `paths` table, and
/// the delta base for path-granular dirty detection on the next refresh.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenPathRow {
    /// Which index pass contributed this row. Reuses the [`Source`]
    /// merge-order enum (`Tree → Index → Worktree`); stored as its lowercase
    /// SQL literal, never a positional integer (coupling №5 eliminated).
    pub source: Source,
    /// Repo-root-relative path.
    pub path_rel: String,
    pub oid: BlobOid,
    /// Denormalized parent directory for dir-scoped serving queries and
    /// carry-forward accounting (replaces `dir_mtimes`).
    pub parent_dir: String,
    /// Worktree rows only: walk mtime at ingest; `None` for tree/index rows
    /// and unstat-able worktree files. Carry-forward matches on the
    /// `(path_rel, stat_mtime_ns)` pair.
    pub stat_mtime_ns: Option<i64>,
}

/// One immutable freshness generation, mirroring its `generations` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generation {
    pub gen_id: i64,
    pub fingerprint: StateFingerprint,
    /// Canonical digest of `fingerprint` (unique across the table).
    pub digest: [u8; 32],
    pub publisher: Option<String>,
    /// Unix epoch milliseconds, monotonic within the store (publish bumps
    /// past any predecessor so retention ordering is total).
    pub created_at: i64,
    /// Hourly liveness bucket; refreshed best-effort on gate hits.
    pub access_bucket: i64,
    /// Member-oid count recorded at publish; verified cheaply before serve.
    pub blob_count: i64,
}

/// Fully computed publish input: inert data built entirely outside any write
/// transaction (parse/ingest happen during the refresh pass; publish only
/// materializes). The type system enforces the compute-outside-write-txn
/// invariant — there is no way to hand the store a producer instead of data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishCandidate {
    pub fingerprint: StateFingerprint,
    pub publisher: Option<String>,
    /// Complete member set across all three sources (the generation's full
    /// corpus mapping, not a delta).
    pub paths: Vec<GenPathRow>,
    /// Blobs newly ingested during this refresh — global content-addressed
    /// upserts keyed by oid.
    pub new_blobs: Vec<(BlobOid, WikiBlobFields)>,
}

/// Publish outcome. Both variants mean "serve": an identical generation
/// already existing is the CAS-analog success path, never an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublishOutcome {
    /// This run's candidate became the stored generation.
    Published { generation: Generation },
    /// An identical digest already existed: nothing was materialized and
    /// the existing generation is the one to serve (lost updates
    /// structurally impossible — the conflict check runs inside the same
    /// `BEGIN IMMEDIATE` that would insert, so a racing identical publisher
    /// serializes behind the winner and observes its committed row).
    ConflictDiscarded { existing: Generation },
}

/// Before/after accounting for the maintenance pass (plan D10): eviction is
/// visible, and its disk tradeoff measurable.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GcStats {
    pub generations_before: u64,
    pub generations_after: u64,
    /// Store file size in bytes before/after (main db + WAL sidecar,
    /// including the `wal_checkpoint(TRUNCATE)` reclaim).
    pub bytes_before: u64,
    pub bytes_after: u64,
    /// Evicted generation ids in eviction order.
    pub evicted_gen_ids: Vec<i64>,
}


/// Handle to the index freshness tier of the merged store. One RW
/// connection, opened WAL; all writes flow through
/// `retry_busy`-wrapped transactions routed via [`Self::with_write_txn`].
///
/// Single-handle, single-thread discipline: a held write transaction
/// borrows the connection exclusively, so reads through the same handle
/// wait for it to close. Production serves strictly after publish returns.
pub struct GenerationsStore {
    conn: RefCell<Connection>,
    #[allow(dead_code)] // read via path(); the binary reaches neither yet
    db_file: PathBuf,
    /// Depth of write transactions opened through this handle. Mirrors the
    /// connection's autocommit state because every write transaction routes
    /// through [`Self::with_write_txn`] / publish / maintain by construction.
    txn_depth: Cell<u32>,
}

impl GenerationsStore {
    /// Open (creating if absent) the merged store's index tier under
    /// `common_dir` — the repository's common git directory shared by every
    /// linked worktree.
    ///
    /// Mirrors the anchor tier's open flow: descriptor-hardened creation of
    /// the private `wiki/` subtree, the no-wait init lock across
    /// probe/quarantine/skew-repair/DDL, then the working connection. A held
    /// init lock means another process is mid-init; since every statement
    /// below is idempotent, this process proceeds directly to its working
    /// connection rather than degrading (index serving has no uncached mode).
    pub fn open(common_dir: &Path) -> Result<Self, CacheError> {
        let mut db_file = Some(schema::db_path(common_dir));
        retry_busy(|| {
            let db_file = db_file.take().expect("open runs once per store");
            // Every creation path goes through the descriptor-hardened
            // primitives (plan D9), exactly like the anchor tier's open.
            let common_fd = DirFd::open(common_dir)?;
            let wiki = common_fd.ensure_private_subtree(Path::new(schema::STORE_DIR_NAME))?;
            wiki.validate_private()?;
            let lock_file = wiki.create_file(schema::INIT_LOCK_FILE_NAME)?;
            let held = match lock_file.try_lock_exclusive() {
                Ok(true) => true,
                Ok(false) => false,
                // Some platforms surface `WouldBlock` as Err instead of
                // Ok(false) (index/lock.rs precedent).
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => false,
                Err(e) => return Err(e.into()),
            };

            if held {
                match schema::probe(&db_file)? {
                    ProbeOutcome::Valid => {}
                    // Missing is a fresh create, never a quarantine.
                    ProbeOutcome::Missing => {
                        schema::prepare_db_file(&db_file)?;
                        drop(schema::open_connection(&db_file)?);
                    }
                    ProbeOutcome::Suspect(_) => {
                        schema::quarantine(&db_file)?;
                    }
                    // Schema skew (plan D2): matching identity pair,
                    // deviating static shapes. Only this tier's skew is ours
                    // to repair; an anchor-tier skew is repaired by the
                    // anchor's own next use and does not block us.
                    ProbeOutcome::Skew(Tier::Index) => {
                        schema::prepare_db_file(&db_file)?;
                        let conn = schema::open_connection(&db_file)?;
                        schema::drop_tier_tables(&conn, Tier::Index)?;
                        drop(conn);
                        drop(schema::open_connection(&db_file)?);
                    }
                    ProbeOutcome::Skew(Tier::Anchor) => {}
                }
                FileExt::unlock(&lock_file)?;
            }
            drop(lock_file);

            let conn = schema::open_connection(&db_file)?;
            // This tier's static DDL, atomic and idempotent.
            apply_index_tier_ddl(&conn)?;

            // Enforce the meta singleton's index_epoch leg here: a store
            // missing or corrupting it fails this open rather than serving
            // unowned generations (plan D2 — consumer state, consumer
            // enforcement). The value itself matters only when a tier-scoped
            // bump lands (clear-cache wave).
            conn.query_row("SELECT index_epoch FROM meta WHERE id = 1", [], |row| {
                row.get::<_, i64>(0)
            })?;

            Ok(Self { conn: RefCell::new(conn), db_file, txn_depth: Cell::new(0) })
        })
    }

    /// Path of the underlying database file (tests and diagnostics open it
    /// read-only to verify physical-layout contracts).
    #[allow(dead_code)] // physical-layout checks; no binary caller this phase
    pub fn path(&self) -> &Path {
        &self.db_file
    }

    /// Shared read access for serve-phase statements. Multi-statement
    /// reads wrap in a deferred transaction via [`Self::read_txn`] so a
    /// concurrent publish's commits never leak between statements of one
    /// command (plan D5 serving mediation).
    pub(crate) fn conn(&self) -> std::cell::Ref<'_, Connection> {
        self.conn.borrow()
    }

    /// Run `f` inside one deferred (read) transaction — the sanctioned way
    /// to issue multi-statement reads, giving them one consistent WAL
    /// snapshot so a concurrent publish's commits never leak between
    /// statements of one command.
    pub fn read_txn<T>(
        &self,
        f: impl FnOnce(&Connection) -> rusqlite::Result<T>,
    ) -> Result<T, CacheError> {
        let conn = self.conn.borrow();
        let tx = conn.unchecked_transaction()?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }

    /// Fast-gate lookup (plan D5): hit ⇒ serve. On a hit, performs the
    /// best-effort hourly `access_bucket` update whose failure is swallowed
    /// — hot reads are never failed or slowed by bookkeeping (errors
    /// ignored, single attempt, no retry). A row failing serve-time
    /// verification — `blob_count` mismatching its actual distinct
    /// `gen_paths` member count, or its `fts_{gen}` child missing — is
    /// reported as a miss (fail-open toward rehash), never served wrong.
    pub fn lookup_digest(
        &self,
        fingerprint: &StateFingerprint,
    ) -> Result<Option<Generation>, CacheError> {
        let digest = fingerprint.digest();
        let generation = {
            let conn = self.conn.borrow();
            let row = conn.query_row(
                generation_select("WHERE digest = ?1").as_str(),
                [digest.as_slice()],
                map_generation_row,
            );
            match row {
                Ok(g) => Some(g),
                Err(rusqlite::Error::QueryReturnedNoRows) => None,
                Err(e) => return Err(e.into()),
            }
        };
        let Some(generation) = generation else {
            return Ok(None);
        };

        // Serve-time verification: the cheap integrity gates before trust.
        let verified = {
            let conn = self.conn.borrow();
            let members: i64 = conn.query_row(
                "SELECT COUNT(DISTINCT oid) FROM gen_paths WHERE gen_id = ?1",
                [generation.gen_id],
                |r| r.get(0),
            )?;
            let has_fts: bool = conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type = 'table' AND name = ?1",
                [format!("fts_{}", generation.gen_id)],
                |r| r.get::<_, i64>(0).map(|n| n > 0),
            )?;
            members == generation.blob_count && has_fts
        };
        if !verified {
            return Ok(None);
        }

        // Best-effort hourly liveness touch (plan D5): single attempt,
        // errors swallowed — hot reads are not writes.
        let bucket = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| (d.as_millis() / MILLIS_PER_HOUR as u128) as i64)
            .unwrap_or(0);
        let _ = self.conn.borrow().execute(
            "UPDATE generations SET access_bucket = ?1 WHERE gen_id = ?2",
            rusqlite::params![bucket, generation.gen_id],
        );

        Ok(Some(generation))
    }

    /// Materialize one candidate generation atomically: one
    /// `BEGIN IMMEDIATE` inserting the `generations` row, `gen_paths` rows,
    /// new-blob upserts, and the created/populated `fts_{gen}` child. The
    /// conflict check runs inside the same transaction as the insert, so a
    /// racing identical publisher serializes behind the winner and observes
    /// its committed row ([`PublishOutcome::ConflictDiscarded`]). Deletes
    /// nothing physical; `blobs.refcount` is reconciled set-based inside the
    /// transaction so it always equals each oid's
    /// `COUNT(DISTINCT gen_id)` membership count.
    pub fn publish(&self, candidate: PublishCandidate) -> Result<PublishOutcome, CacheError> {
        let mut candidate = Some(candidate);
        retry_busy(|| {
            let candidate = candidate.take().expect("publish runs once per call");
            let publisher = candidate.publisher.clone();
            let digest = candidate.fingerprint.digest();
            let mut conn = self.conn.borrow_mut();
            let _guard = TxnGuard::new(&self.txn_depth);
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;

            // Conflict check inside the write transaction: an identical
            // generation already existing is a discard-and-serve-existing,
            // never an error.
            let existing = tx
                .query_row(generation_select("WHERE digest = ?1").as_str(), [digest.as_slice()], map_generation_row)
                .optional()?;
            if let Some(existing) = existing {
                return Ok(PublishOutcome::ConflictDiscarded { existing });
            }

            // Monotonic created_at: wall clock bumped past any predecessor,
            // making the retention sort total even within one millisecond.
            let now_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            let max_created: i64 = tx
                .query_row("SELECT COALESCE(MAX(created_at), 0) FROM generations", [], |r| {
                    r.get(0)
                })?;
            let created_at = now_ms.max(max_created + 1);

            // Member count = distinct oids in the candidate's path set.
            let member_oids: std::collections::HashSet<&str> = candidate
                .paths
                .iter()
                .map(|p| p.oid.0.as_str())
                .collect();
            let blob_count = member_oids.len() as i64;

            // Global content-addressed blob upserts: content is immutable
            // per oid, so an already-known oid is left untouched.
            for (oid, fields) in &candidate.new_blobs {
                tx.execute(
                    "INSERT INTO blobs (oid, refcount, title, summary, body, aliases_text, tags_text, keywords_text)
                     VALUES (?1, 0, ?2, ?3, ?4, ?5, ?6, ?7)
                     ON CONFLICT(oid) DO NOTHING",
                    rusqlite::params![
                        oid.0,
                        fields.title,
                        fields.summary,
                        fields.body,
                        fields.aliases_text,
                        fields.tags_text,
                        fields.keywords_text,
                    ],
                )?;
            }

            tx.execute(
                "INSERT INTO generations (digest, head_oid, head_tree_oid, index_checksum,
                                          wikiignore_hash, worktree_sig, publisher,
                                          created_at, access_bucket, blob_count)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                rusqlite::params![
                    digest,
                    candidate.fingerprint.head_oid,
                    candidate.fingerprint.head_tree_oid,
                    candidate.fingerprint.index_checksum,
                    candidate.fingerprint.wikiignore_hash,
                    candidate.fingerprint.worktree_sig,
                    publisher,
                    created_at,
                    created_at / MILLIS_PER_HOUR,
                    blob_count,
                ],
            )?;
            let gen_id = tx.last_insert_rowid();

            {
                let mut stmt = tx.prepare(
                    "INSERT INTO gen_paths (gen_id, source, path_rel, oid, parent_dir, stat_mtime_ns)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                )?;
                for row in &candidate.paths {
                    stmt.execute(rusqlite::params![
                        gen_id,
                        source_sql(row.source),
                        row.path_rel,
                        row.oid.0,
                        row.parent_dir,
                        row.stat_mtime_ns,
                    ])?;
                }
            }

            // Membership refcounts, reconciled set-based inside the same
            // transaction: refcount == COUNT(DISTINCT gen_id) over
            // gen_paths, for every blob — including oids no longer
            // referenced by anyone (left at zero; GC reclaims them).
            tx.execute_batch(
                "UPDATE blobs SET refcount = COALESCE(
                     (SELECT COUNT(DISTINCT gp.gen_id) FROM gen_paths gp WHERE gp.oid = blobs.oid),
                     0);",
            )?;

            // Per-generation FTS materialization, over members only.
            let fts_name = format!("fts_{gen_id}");
            tx.execute_batch(&format!(
                "CREATE VIRTUAL TABLE \"{fts_name}\" USING fts5(
                     title, aliases_text, tags_text, keywords_text, summary, body,
                     tokenize='unicode61 remove_diacritics 2',
                     prefix='2 3 4'
                 );",
            ))?;
            tx.execute_batch(&format!(
                "INSERT INTO \"{fts_name}\"(rowid, title, aliases_text, tags_text, keywords_text, summary, body)
                 SELECT b.rowid, b.title, b.aliases_text, b.tags_text, b.keywords_text, b.summary, b.body
                 FROM blobs b
                 WHERE b.oid IN (SELECT oid FROM gen_paths WHERE gen_id = {gen_id});",
            ))?;

            tx.commit()?;
            Ok(PublishOutcome::Published {
                generation: Generation {
                    gen_id,
                    fingerprint: candidate.fingerprint,
                    digest,
                    publisher,
                    created_at,
                    access_bucket: created_at / MILLIS_PER_HOUR,
                    blob_count,
                },
            })
        })
    }

    /// Maintenance pass (plan D10, runs in the publisher's exclusive window):
    /// the newest-by-`created_at` generation always survives; among the rest,
    /// sorted by `(access_bucket ASC, created_at ASC)`, the newest
    /// [`RETAINED_GENERATIONS`] survive and the rest are evicted in order —
    /// drop `fts_{g}` → delete its `gen_paths` rows → delete blobs no longer
    /// referenced by any retained generation → `wal_checkpoint(TRUNCATE)` →
    /// account into [`GcStats`]. Refcounts reconcile set-based in the same
    /// transaction as the deletions.
    pub fn maintain(&self) -> Result<GcStats, CacheError> {
        let bytes_before = store_bytes(&self.db_file);
        let stats = retry_busy(|| {
            let mut conn = self.conn.borrow_mut();
            let _guard = TxnGuard::new(&self.txn_depth);
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;

            let generations_before: i64 =
                tx.query_row("SELECT COUNT(*) FROM generations", [], |r| r.get(0))?;

            // Protected: the single newest by (created_at, gen_id).
            let protected: Option<i64> = tx
                .query_row(
                    "SELECT gen_id FROM generations
                     ORDER BY created_at DESC, gen_id DESC LIMIT 1",
                    [],
                    |r| r.get(0),
                )
                .optional()?;

            // Candidates: everything else, eviction-worst first.
            let candidates: Vec<i64> = {
                let mut stmt = tx.prepare(
                    "SELECT gen_id FROM generations
                     WHERE gen_id IS NOT ?1
                     ORDER BY access_bucket ASC, created_at ASC, gen_id ASC",
                )?;
                let rows = stmt.query_map(rusqlite::params![protected], |r| r.get(0))?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            };

            let evict_count = candidates.len().saturating_sub(RETAINED_GENERATIONS);
            let evicted: Vec<i64> = candidates.into_iter().take(evict_count).collect();

            for gen_id in &evicted {
                // Ordered teardown: the FTS child dies first, then the
                // generation's path rows; blobs reclaim set-based below.
                tx.execute_batch(&format!("DROP TABLE IF EXISTS \"fts_{gen_id}\";"))?;
                tx.execute("DELETE FROM gen_paths WHERE gen_id = ?1", [gen_id])?;
                tx.execute("DELETE FROM generations WHERE gen_id = ?1", [gen_id])?;
            }

            // Reclaim blobs no retained generation references, and settle
            // their membership refcounts in the same statement scope.
            tx.execute_batch(
                "UPDATE blobs SET refcount = COALESCE(
                     (SELECT COUNT(DISTINCT gp.gen_id) FROM gen_paths gp WHERE gp.oid = blobs.oid),
                     0);
                 DELETE FROM blobs WHERE refcount = 0;",
            )?;

            let generations_after: i64 =
                tx.query_row("SELECT COUNT(*) FROM generations", [], |r| r.get(0))?;
            tx.commit()?;

            // GC diagnostics (plan D11), inside the caller's exclusive
            // window: one countable row per evicted generation, then the
            // run's JSON-lines payload picks up the refreshed counts.
            for _gen_id in &evicted {
                // One countable row per eviction; the id itself lives in
                // the GcStats accounting, not the ledger.
                diagnostics::record(&conn, "gc_generations_pruned");
            }
            diagnostics::publish_counts(&conn);

            Ok(GcStats {
                generations_before: generations_before.max(0) as u64,
                generations_after: generations_after.max(0) as u64,
                bytes_before,
                bytes_after: 0, // filled in by the caller after checkpoint
                evicted_gen_ids: evicted,
            })
        });

        let mut stats = stats?;
        // Truncate the WAL so reclaimed pages return to the OS before the
        // after-accounting (plan D10). Best-effort: busy contention keeps
        // the WAL but never fails the maintenance pass.
        let _ = self.conn.borrow().execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
        stats.bytes_after = store_bytes(&self.db_file);
        Ok(stats)
    }

    /// Non-FTS serving mediation leg: the exact `gen_paths` rows of
    /// `generation_id` for `source`, ordered by path — what `list_pages`,
    /// exact-title/path resolution, and the refresh delta base join
    /// through, scoped to the served generation.
    #[allow(dead_code)] // store-level spec surface; serving goes through search.rs joins
    pub fn generation_paths(
        &self,
        generation_id: i64,
        source: Source,
    ) -> Result<Vec<GenPathRow>, CacheError> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(
            "SELECT source, path_rel, oid, parent_dir, stat_mtime_ns
             FROM gen_paths WHERE gen_id = ?1 AND source = ?2
             ORDER BY path_rel ASC",
        )?;
        let literal = source_sql(source);
        let rows = stmt.query_map(rusqlite::params![generation_id, literal], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<i64>>(4)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (src, path_rel, oid, parent_dir, stat_mtime_ns) = row?;
            out.push(GenPathRow {
                source: source_from_sql(&src).unwrap_or(Source::Worktree),
                path_rel,
                oid: BlobOid(oid),
                parent_dir,
                stat_mtime_ns,
            });
        }
        Ok(out)
    }

    /// All three sources' rows of one generation, keyed for the refresh
    /// merge pipeline. Delta-base loading helper for the orchestrator.
    pub(crate) fn all_generation_paths(
        &self,
        generation_id: i64,
    ) -> Result<Vec<GenPathRow>, CacheError> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(
            "SELECT source, path_rel, oid, parent_dir, stat_mtime_ns
             FROM gen_paths WHERE gen_id = ?1",
        )?;
        let rows = stmt.query_map([generation_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<i64>>(4)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (src, path_rel, oid, parent_dir, stat_mtime_ns) = row?;
            out.push(GenPathRow {
                source: source_from_sql(&src).unwrap_or(Source::Worktree),
                path_rel,
                oid: BlobOid(oid),
                parent_dir,
                stat_mtime_ns,
            });
        }
        Ok(out)
    }

    /// The newest generation by `(created_at, gen_id)` — the refresh delta
    /// base (plan D5). `None` on a cold store.
    pub fn newest(&self) -> Result<Option<Generation>, CacheError> {
        let conn = self.conn.borrow();
        let row = conn.query_row(
            &generation_select("ORDER BY created_at DESC, gen_id DESC LIMIT 1"),
            [],
            map_generation_row,
        );
        match row {
            Ok(g) => Ok(Some(g)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// True iff this handle currently sits inside a write transaction.
    /// Exposes the compute-outside-write-txn invariant to checks and
    /// integration assertions: heavy parse/ingest must be observable
    /// outside the publish window. Accurate because every write
    /// transaction routes through this handle's guarded entry points.
    #[allow(dead_code)] // invariant probe consumed by acceptance checks
    pub fn is_in_write_txn(&self) -> bool {
        self.txn_depth.get() > 0
    }

    /// Run `f` inside one `retry_busy`-wrapped `BEGIN IMMEDIATE`
    /// transaction on this handle's connection — the only sanctioned way to
    /// take a generic write transaction, so every writer inherits the
    /// bounded-busy contract by construction. While the transaction is
    /// open, `f` must not touch the connection except through
    /// [`Self::is_in_write_txn`] (the borrow is exclusive).
    #[allow(dead_code)] // sanctioned generic writer; first production caller is the journals wave
    pub fn with_write_txn<T>(
        &self,
        f: impl FnOnce(&Self) -> Result<T, CacheError>,
    ) -> Result<T, CacheError> {
        let mut f = Some(f);
        retry_busy(|| {
            let mut conn = self.conn.borrow_mut();
            let _guard = TxnGuard::new(&self.txn_depth);
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let outcome = f.take().expect("with_write_txn runs once")(self);
            match outcome {
                Ok(value) => {
                    tx.commit()?;
                    Ok(value)
                }
                Err(e) => {
                    drop(tx); // rollback on drop
                    Err(e)
                }
            }
        })
    }
}

/// RAII depth marker for [`GenerationsStore::txn_depth`]; panic-safe on the
/// early-return paths inside a transaction body.
struct TxnGuard<'a> {
    depth: &'a Cell<u32>,
}

impl<'a> TxnGuard<'a> {
    fn new(depth: &'a Cell<u32>) -> Self {
        depth.set(depth.get() + 1);
        Self { depth }
    }
}

impl Drop for TxnGuard<'_> {
    fn drop(&mut self) {
        self.depth.set(self.depth.get().saturating_sub(1));
    }
}

/// Sum of the main database file and its WAL sidecar, for GcStats.
fn store_bytes(db_file: &Path) -> u64 {
    let mut wal = db_file.as_os_str().to_os_string();
    wal.push("-wal");
    let main = std::fs::metadata(db_file).map(|m| m.len()).unwrap_or(0);
    let sidecar = std::fs::metadata(PathBuf::from(wal)).map(|m| m.len()).unwrap_or(0);
    main + sidecar
}

/// Column list shared by every generation-row select; ordering matches
/// [`map_generation_row`].
const GENERATION_COLUMNS: &str = "gen_id, digest, head_oid, head_tree_oid, index_checksum, \
     wikiignore_hash, worktree_sig, publisher, created_at, access_bucket, blob_count";

fn generation_select(where_clause: &str) -> String {
    format!("SELECT {GENERATION_COLUMNS} FROM generations {where_clause}")
}

/// Map one `generations` row (in [`GENERATION_COLUMNS`] order) to its
/// typed value.
fn map_generation_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<Generation> {
    let digest_blob: Vec<u8> = row.get(1)?;
    let mut digest = [0u8; 32];
    digest.copy_from_slice(&digest_blob);
    let head_oid: String = row.get(2)?;
    let head_tree_oid: String = row.get(3)?;
    let checksum_blob: Vec<u8> = row.get(4)?;
    let ignore_blob: Vec<u8> = row.get(5)?;
    let sig_blob: Vec<u8> = row.get(6)?;
    let mut index_checksum = [0u8; 20];
    index_checksum.copy_from_slice(&checksum_blob);
    let mut wikiignore_hash = [0u8; 20];
    wikiignore_hash.copy_from_slice(&ignore_blob);
    let mut worktree_sig = [0u8; 32];
    worktree_sig.copy_from_slice(&sig_blob);
    Ok(Generation {
        gen_id: row.get(0)?,
        fingerprint: StateFingerprint {
            head_oid,
            head_tree_oid,
            index_checksum,
            wikiignore_hash,
            worktree_sig,
        },
        digest,
        publisher: row.get(7)?,
        created_at: row.get(8)?,
        access_bucket: row.get(9)?,
        blob_count: row.get(10)?,
    })
}

/// Apply the index tier's static DDL atomically and idempotently.
fn apply_index_tier_ddl(conn: &Connection) -> Result<(), CacheError> {
    // Belt-and-braces over `open_connection`, which applies the same const:
    // this covers stores materialized before the index tier's registration,
    // atomically and idempotently.
    conn.execute_batch(&format!(
        "BEGIN IMMEDIATE;\n{}\nCOMMIT;",
        schema::INDEX_TIER_DDL
    ))?;
    Ok(())
}
