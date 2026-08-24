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
//! # Pinned store-foundation wiring (Phase 3 imports; peer-delivered)
//!
//! These are the symbols this module consumes once the generalized
//! foundation lands. They are referenced in docs only at this bootstrap
//! phase — no compile-time dependency exists until Phase 3 fills the
//! bodies:
//!
//! * Store location — `crate::cache::schema::{STORE_DIR_NAME, DB_FILE_NAME,
//!   store_dir(common_dir), db_path(common_dir)}` →
//!   `<common>/wiki/store.sqlite`.
//! * Connection opening — the peer's generalized open
//!   (`busy_timeout(1000)` **before** `journal_mode=WAL`,
//!   `synchronous=NORMAL`, `mmap_size=268435456`); every write transaction
//!   wraps in `crate::cache::schema::retry_busy`.
//! * Meta singleton — `(application_id='wkst', schema_version=1,
//!   anchor_epoch, index_epoch)`; **this module owns reading/enforcing
//!   `index_epoch`** (mismatch ⇒ tier-scoped bulk invalidation, never a
//!   quarantine).
//! * Table names — static tier tables are exactly `generations`, 
//!   `gen_paths`, `blobs`; dynamic FTS children are exactly `fts_{gen_id}`
//!   (prefix `fts_`) — the peer's tier-scoped invalidation/clear drops
//!   precisely these names.
//! * Digest encoding — reuses `crate::cache::key::{sha256_hex, push_field}`
//!   length-tagged discipline.
//! * Rendezvous (later wiring) — `crate::cache::rendezvous::{acquire_shared,
//!   acquire_exclusive}`.
//! * fd-hardened creation — `crate::store::fd::*` routes every creation
//!   path under `.git/`.
//!
//! # DDL draft (binding contract artifact)
//!
//! Static tables (created by `open` inside a `retry_busy`-wrapped
//! transaction; registry-visible to the peer's probe):
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
//! `blobs` stays global and content-addressed (`oid` PK + parsed fields,
//! column-for-column today's shape); a unique oid is parsed/tokenized once
//! ever. `refcount` counts retained-generation memberships via `gen_paths`.
//!
//! Deliberate normalizations against the plan text (flagged at review):
//! the plan's `digest BLOB(20)`-style parameterized widths are expressed as
//! plain-typed columns with `CHECK(length(...))` because STRICT tables
//! forbid parameterized type names; `source` is a self-describing TEXT
//! literal (`'tree'|'index'|'worktree'`) rather than today's positional
//! integer, eliminating hidden coupling №5 (the coincidental alignment of
//! `passes::source_id` and `search::source_filter_id`); `created_at` is
//! unix epoch **milliseconds** with a monotonic bump guarantee inside
//! publish (max(now, previous + 1)) so retention ordering is deterministic
//! even for same-second publishes; `access_bucket` is that value divided by
//! 3_600_000 (hourly liveness buckets per D10).
//!
//! Dynamic per-generation FTS children (created by DDL at publish, dropped
//! whole at conflict-discard/GC/tier-invalidation; invisible to the
//! registry). Standalone fts5 — not external-content — because global
//! triggers cannot target per-generation tables; served BM25 statistics are
//! therefore exactly what a cold rebuild of the same digest produces:
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
//! -- bm25(fts_{g}, 5, 4, 3, 3, 2, 1) matching today's weights.
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
//! Reuses the anchor cache's provably injective length-tagged framing from
//! `crate::cache::key::{push_field, sha256_hex}`:
//!
//! ```text
//! frame(bytes) = u64_le(byte_len) ++ bytes
//! frame(str)   = u64_le(utf8_len) ++ utf8_bytes          // push_field form
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
//! Sentinels carried over verbatim from today's single-row model (hidden
//! couplings №2/№3 made explicit and typed): unborn HEAD
//! [`UNBORN_HEAD_OID`], empty-tree diff base [`EMPTY_TREE_BASE`], 20-zero
//! [`ZERO_INDEX_CHECKSUM`] / [`ZERO_WIKIIGNORE_HASH`]. The fresh-zero guard
//! dissolves — no matching generation simply means a miss.

// Phase 1+2 bootstrap: the API surface below is contract-only until Phase 3
// fills the bodies and wires the refresh/serving paths onto it. The binary
// crate compiles this file into its own private module tree, so every not-
// yet-called item trips `dead_code` there. Remove this allow in Phase 3.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::cache::CacheError;

use super::ingest::WikiBlobFields;
use super::{BlobOid, Source};

/// HEAD oid sentinel for an unborn branch — `"0".repeat(40)`, byte-identical
/// to freshness's ZERO_OID and passes' refresh emission (coupling №2, now a
/// named constant both sides will use).
pub const UNBORN_HEAD_OID: &str = "0000000000000000000000000000000000000000";

/// `head_tree_oid` sentinel: an empty string means "diff against the empty
/// tree" (first commit / no commits yet).
pub const EMPTY_TREE_BASE: &str = "";

/// 20-zero git-index checksum trailer sentinel (absent `.git/index`).
pub const ZERO_INDEX_CHECKSUM: [u8; 20] = [0u8; 20];

/// 20-zero wikiignore SHA-1 sentinel (no wikiignore file in this state).
pub const ZERO_WIKIIGNORE_HASH: [u8; 20] = [0u8; 20];

/// Retention bound (plan D10): after the always-live newest generation, the
/// next-newest 8 candidates survive eviction — a recency-liveness rule where
/// being looked up keeps a worktree's generation warm.
pub const RETAINED_GENERATIONS: usize = 8;

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
        todo!("StateFingerprint::digest({self:?}) — implemented in Phase 3")
    }

    /// Lowercase-hex diagnostics form of [`Self::digest`] (via
    /// `cache::key::sha256_hex`). Never used as a storage key.
    pub fn digest_hex(&self) -> String {
        todo!("StateFingerprint::digest_hex({self:?}) — implemented in Phase 3")
    }
}

/// Canonical worktree signature: SHA-256 over the sorted
/// `(repo-relative dir-or-md path, mtime_ns)` pairs of directories plus
/// `*.md` files (the `.git`/store prunes unchanged), replacing today's i64
/// `DefaultHasher` fold in `freshness::compute_worktree_dir_hash`. Sorts
/// internally — caller order never leaks into the signature.
pub fn worktree_signature(pairs: &[(String, i64)]) -> [u8; 32] {
    todo!("worktree_signature({pairs:?}) — implemented in Phase 3")
}

/// One resolved path row of one generation — the reverse `(source, path) →
/// blob` mapping that replaces today's global mutable `paths` table, and the
/// delta base for path-granular dirty detection on the next refresh.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenPathRow {
    /// Which index pass contributed this row. Reuses the existing [`Source`]
    /// merge-order enum (`Tree → Index → Worktree`); stored as its lowercase
    /// SQL literal (`'tree'|'index'|'worktree'`), never a positional integer
    /// (hidden coupling №5 eliminated).
    pub source: Source,
    /// Repo-relative path (`''` parent for root-level entries).
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
    /// Hourly liveness bucket (`created_at / 3_600_000`); refreshed
    /// best-effort on gate hits.
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
    /// An identical digest already existed: the just-built `fts_{gen}`
    /// table was dropped, all built rows discarded, and the existing
    /// generation is the one to serve (lost updates structurally
    /// impossible via `ON CONFLICT(digest) DO NOTHING` + zero-inserts check).
    ConflictDiscarded { existing: Generation },
}

/// Before/after accounting for the maintenance pass (plan D10): eviction is
/// visible, and its disk tradeoff measurable.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GcStats {
    pub generations_before: u64,
    pub generations_after: u64,
    /// Store file size in bytes before/after (including the
    /// `wal_checkpoint(TRUNCATE)` reclaim).
    pub bytes_before: u64,
    pub bytes_after: u64,
    /// Evicted generation ids in eviction order.
    pub evicted_gen_ids: Vec<i64>,
}

/// Handle to the index freshness tier of the merged store. One RW
/// connection, opened WAL; all writes flow through
/// `retry_busy`-wrapped transactions.
///
/// Open sequence (Phase 3): derive `<common>/wiki/store.sqlite` via the
/// pinned schema helpers, create through `store::fd` hardening, apply the
/// DDL draft above idempotently, read/enforce meta `index_epoch`
/// (mismatch ⇒ drop this tier's static tables plus every `fts_*` child,
/// recreate — schema skew, never quarantine).
pub struct GenerationsStore {
    #[allow(dead_code)] // owned from Phase 3 when bodies land
    conn: Connection,
    #[allow(dead_code)] // ditto
    db_file: PathBuf,
}

impl GenerationsStore {
    /// Open (creating if absent) the merged store under `common_dir` — the
    /// repository's common git directory shared by every linked worktree.
    pub fn open(common_dir: &Path) -> Result<Self, CacheError> {
        todo!("GenerationsStore::open({}) — implemented in Phase 3", common_dir.display())
    }

    /// Path of the underlying database file (tests and diagnostics open it
    /// read-only to verify physical layout contracts).
    pub fn path(&self) -> &Path {
        todo!("GenerationsStore::path() — implemented in Phase 3")
    }

    /// Fast-gate lookup (plan D5): hit ⇒ serve. On a hit, performs the
    /// best-effort hourly `access_bucket` update whose failure is swallowed
    /// — hot reads are never failed or slowed by bookkeeping (errors
    /// ignored, single attempt, no retry). A row failing serve-time
    /// verification — `blob_count` mismatching its actual `gen_paths`
    /// member count, or its `fts_{gen}` table missing — is reported as a
    /// miss (fail-open toward rehash), never served wrong.
    pub fn lookup_digest(
        &self,
        fingerprint: &StateFingerprint,
    ) -> Result<Option<Generation>, CacheError> {
        todo!("lookup_digest({fingerprint:?}) — implemented in Phase 3")
    }

    /// Materialize one candidate generation atomically: one
    /// `BEGIN IMMEDIATE` inserting the `generations` row, `gen_paths` rows,
    /// new-blob upserts, and the created/populated `fts_{gen}` child;
    /// `ON CONFLICT(digest) DO NOTHING` with a zero-inserts check yielding
    /// [`PublishOutcome::ConflictDiscarded`] (which drops the just-built
    /// `fts_` table first). Deletes nothing physical.
    pub fn publish(&self, candidate: PublishCandidate) -> Result<PublishOutcome, CacheError> {
        todo!("publish({candidate:?}) — implemented in Phase 3")
    }

    /// Maintenance pass (plan D10, runs in the publisher's exclusive window):
    /// the newest-by-`created_at` generation always survives; among the rest,
    /// sorted by `(access_bucket ASC, created_at ASC)`, the newest
    /// [`RETAINED_GENERATIONS`] survive and the rest are evicted in order —
    /// drop `fts_{g}` → delete its `gen_paths` rows → delete blobs now
    /// unreferenced by any retained generation → `wal_checkpoint(TRUNCATE)`
    /// → account into [`GcStats`].
    pub fn maintain(&self) -> Result<GcStats, CacheError> {
        todo!("maintain() — implemented in Phase 3")
    }

    /// Non-FTS serving mediation leg: the exact `gen_paths` rows of
    /// `generation_id` for `source` (what `list_pages` and exact-title/path
    /// resolution join through, scoped to the served generation instead of
    /// the old global `paths` table).
    pub fn generation_paths(
        &self,
        generation_id: i64,
        source: Source,
    ) -> Result<Vec<GenPathRow>, CacheError> {
        todo!("generation_paths({generation_id}, {source:?}) — implemented in Phase 3")
    }

    /// True iff this handle currently sits inside a write transaction
    /// (`BEGIN IMMEDIATE`/…, i.e. `!is_autocommit()`). Exposes the
    /// compute-outside-write-txn invariant to checks and integration
    /// assertions: heavy parse/ingest must be observable outside the
    /// publish window.
    pub fn is_in_write_txn(&self) -> bool {
        todo!("is_in_write_txn() — implemented in Phase 3")
    }

    /// Run `f` inside one `retry_busy`-wrapped `BEGIN IMMEDIATE`
    /// transaction on this handle's connection — the only sanctioned way to
    /// take a write transaction, so every writer inherits the bounded-busy
    /// contract by construction.
    pub fn with_write_txn<T>(
        &self,
        f: impl FnOnce(&Self) -> Result<T, CacheError>,
    ) -> Result<T, CacheError> {
        let _ = f;
        todo!("with_write_txn(_) — implemented in Phase 3")
    }
}
