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

pub mod diagnostics;
pub mod key;
pub mod rendezvous;
pub mod schema;

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use fs4::fs_std::FileExt;
use rusqlite::params;

use crate::store::fd::DirFd;

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

/// A store fault, raised by machinery both tiers share (probe/quarantine/
/// skew repair/DDL, fd hardening, locks) or by either tier's connection.
/// The Display labels name the shared store layer — never one tier —
/// because the same value type serves both; tier context comes from each
/// caller's own wrapping (the anchor reporter's
/// `warning: anchor cache unavailable (...)` line, the index open path's
/// `failed to open wiki store ...`). Labeling shared faults after one tier
/// misattributed every other tier's failure to that tier.
///
/// For the anchor tier every variant means the cache is unavailable for the
/// run; the caller falls back to uncached computation with at most one
/// diagnostic line (plan decision 7). Transient `SQLITE_BUSY` is retried
/// internally by the bounded wrapper (plan decision 6) and never
/// quarantines — it surfaces here only after the retries are exhausted.
#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    /// Filesystem operation failed (lock, quarantine rename, sidecar delete,
    /// directory create).
    #[error("wiki store I/O failure: {0}")]
    Io(#[from] std::io::Error),
    /// SQLite operation failed (probe, open, statement).
    #[error("wiki store SQLite failure: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// The database was corrupt (NOTADB / CORRUPT / meta mismatch) and the
    /// single quarantine-and-recreate attempt also failed.
    #[error("wiki store is corrupt and could not be recreated: {0}")]
    Corrupt(String),
}

/// Invocation-scoped cache diagnostic budget. Clones share the same guard,
/// so separate stores and fix phases still emit at most one warning total.
#[derive(Clone, Copy, PartialEq, Eq)]
enum InjectedFault {
    Operational,
    #[cfg_attr(not(debug_assertions), allow(dead_code))]
    Schema,
    /// Overwrites the store file with garbage bytes, forcing the
    /// corruption-class quarantine path (plan D11's witness harness).
    #[cfg_attr(not(debug_assertions), allow(dead_code))]
    Corrupt,
}

#[derive(Default)]
struct ReporterState {
    reported: Cell<bool>,
    injected: RefCell<VecDeque<InjectedFault>>,
}

#[derive(Clone, Default)]
pub struct CacheReporter(Rc<ReporterState>);

impl CacheReporter {
    pub fn unavailable(&self, reason: &str) {
        if !self.0.reported.replace(true) {
            eprintln!("warning: anchor cache unavailable ({reason}); continuing uncached");
        }
    }

    pub fn rebuilt(&self) {
        if !self.0.reported.replace(true) {
            eprintln!("warning: anchor cache was corrupt; rebuilt");
        }
    }

    pub fn for_invocation() -> Self {
        let reporter = Self::default();
        #[cfg(debug_assertions)]
        if let Ok(script) = std::env::var("WIKI_ANCHOR_CACHE_TEST_FAULT_SEQUENCE") {
            reporter
                .0
                .injected
                .borrow_mut()
                .extend(script.split(',').filter_map(|item| match item {
                    "operational" => Some(InjectedFault::Operational),
                    "schema" => Some(InjectedFault::Schema),
                    "corrupt" => Some(InjectedFault::Corrupt),
                    _ => None,
                }));
        }
        reporter
    }

    fn next_injected(&self) -> Option<InjectedFault> {
        self.0.injected.borrow_mut().pop_front()
    }
}

/// The cache surface the drift seams consume (plan decision 5). Reads verify
/// before serving — the stored tuple is compared field-by-field against the
/// queried tuple and its `row_digest` checked; writes are computed strictly
/// outside any transaction by the caller and flushed per page.
pub trait AnchorCache {
    /// Start collecting writes for one classified page.
    fn begin_page(&self) {}

    /// Commit the current page's collected writes in one transaction.
    fn flush_page(&self) -> Result<(), CacheError> {
        Ok(())
    }

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

    /// Clear the store's derived data — tier-scoped (plan D10/D7): drop
    /// BOTH tiers' static tables plus every dynamic `fts_%` child inside
    /// one transaction under the exclusive rendezvous lock and the init
    /// lock, and delete stale `.quarantine` asides. Nothing else is
    /// removed: the directory survives (it holds journals, both lock
    /// files, and the log), as do the meta singleton and the cross-tier
    /// `store_events` ledger. Used by `wiki check --clear-cache`, plan
    /// decision 8.
    fn clear(&self) -> Result<(), CacheError>;
}

/// The real anchor cache: an SQLite database under the repository's common
/// git directory, shared by every linked worktree of one repo (plan
/// decision 2). See [`schema`] for the storage layout and the
/// probe/quarantine/open ordering.
pub struct CacheStore {
    /// Open SQLite connection (always `Some` after [`CacheStore::open`]).
    conn: RefCell<Option<rusqlite::Connection>>,
    /// Common git dir — derives the db and init-lock paths via [`schema`].
    common_dir: PathBuf,
    pending: RefCell<Vec<PendingWrite>>,
    collecting_page: Cell<bool>,
    disabled: Cell<bool>,
    reporter: CacheReporter,
    inject_operational: Cell<bool>,
}

enum PendingWrite {
    Fingerprint {
        key: String,
        page_path: String,
        anchor_sha: String,
        target_path: String,
        range_start: u32,
        range_end: u32,
        fp: String,
        digest: Vec<u8>,
    },
    Walk {
        key: String,
        page_path: String,
        log_output_sha: String,
        anchor_sha: String,
        path_at_commit: String,
        value: Option<String>,
        digest: Vec<u8>,
    },
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
    /// The whole sequence runs under the bounded `SQLITE_BUSY` retry wrapper
    /// (plan decision 6). The no-wait init lock is held only across
    /// probe/quarantine/DDL (git-span's shape), then released before the
    /// working connection opens; a held lock means another process is mid-
    /// init, so this run serves uncached rather than waiting.
    pub fn open(common_dir: &Path) -> Result<Option<Self>, CacheError> {
        Self::open_with_reporter(common_dir, CacheReporter::default())
    }

    pub fn open_with_reporter(
        common_dir: &Path,
        reporter: CacheReporter,
    ) -> Result<Option<Self>, CacheError> {
        let injected = reporter.next_injected();
        let db = schema::db_path(common_dir);
        schema::retry_busy(|| {
            // Every creation path — the `wiki/` subtree, the init lock, the
            // database file — goes through the descriptor-hardened
            // primitives (plan D9; the Phase-2 adoption site).
            let common = DirFd::open(common_dir)?;
            let wiki = common.ensure_private_subtree(Path::new(schema::STORE_DIR_NAME))?;
            wiki.validate_private()?;
            let lock_file = wiki.create_file(schema::INIT_LOCK_FILE_NAME)?;
            let held = match lock_file.try_lock_exclusive() {
                Ok(true) => true,
                Ok(false) => false,
                // Some platforms surface `WouldBlock` as Err instead of
                // Ok(false) (index/lock.rs precedent).
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => false,
                Err(e) => return Err(e.into()),
            };
            if !held {
                return Ok(None);
            }
            #[cfg(debug_assertions)]
            if injected == Some(InjectedFault::Schema) && db.exists() {
                let conn = rusqlite::Connection::open(&db)?;
                conn.execute_batch("DROP TABLE IF EXISTS fingerprint; CREATE TABLE fingerprint (key_digest TEXT PRIMARY KEY) STRICT;")?;
            }
            #[cfg(debug_assertions)]
            if injected == Some(InjectedFault::Corrupt) {
                std::fs::write(&db, "wiki test fault: deliberately not a database")?;
            }
            match schema::probe(&db)? {
                schema::ProbeOutcome::Valid => {}
                // Missing is a fresh create, never a quarantine; the DDL leg
                // runs under the lock.
                schema::ProbeOutcome::Missing => {
                    schema::prepare_db_file(&db)?;
                    drop(schema::open_connection(&db)?);
                }
                schema::ProbeOutcome::Suspect(_) => {
                    schema::quarantine(&db)?;
                    // The rebuild succeeded and this run continues cached —
                    // a notice, not an unavailability fault (the caller's
                    // `warning: anchor cache unavailable` line must NOT fire
                    // for this case). Emitted once per successful
                    // quarantine-and-recreate, in every mode (plain text,
                    // no JSON); a quarantine that failed never reaches this
                    // line — the Err propagation and the caller's fault
                    // line cover it.
                    reporter.rebuilt();
                }
                // Schema skew (plan D2): matching identity pair, deviating
                // static shapes for one tier. Tier-scoped structural
                // invalidation — never quarantine, never a diagnostic; the
                // tier rebuilds silently on next use. The repair is still a
                // countable store event (plan D11).
                schema::ProbeOutcome::Skew(tier) => {
                    schema::prepare_db_file(&db)?;
                    let conn = schema::open_connection(&db)?;
                    schema::drop_tier_tables(&conn, tier)?;
                    let event = match tier {
                        schema::Tier::Anchor => "skew_repair:anchor",
                        schema::Tier::Index => "skew_repair:index",
                    };
                    diagnostics::record(&conn, event);
                    drop(conn);
                    drop(schema::open_connection(&db)?);
                }
            }
            // Release the init lock before the working connection opens.
            // Explicit unlock matters on filesystems where closing a cloned
            // or internally duplicated descriptor does not promptly release
            // the advisory lock for a same-process clear.
            FileExt::unlock(&lock_file)?;
            drop(lock_file);
            let conn = schema::open_connection(&db)?;
            // Surface the run's store_events tallies through the perf
            // JSON-lines channel (plan D11): everything recorded during this
            // open — quarantine/rebuild/skew repair included — plus any
            // earlier ledger rows. No-op for the kill-switch and disabled
            // paths, where the payload's diagnostics field stays omitted.
            diagnostics::publish_counts(&conn);
            Ok(Some(CacheStore {
                conn: RefCell::new(Some(conn)),
                common_dir: common_dir.to_path_buf(),
                pending: RefCell::new(Vec::new()),
                collecting_page: Cell::new(false),
                disabled: Cell::new(false),
                reporter: reporter.clone(),
                inject_operational: Cell::new(injected == Some(InjectedFault::Operational)),
            }))
        })
    }

    /// Construct a cache-management handle without opening SQLite. This is
    /// required on Windows, where an open database cannot be unlinked.
    pub fn for_clear(common_dir: &Path) -> Self {
        Self::for_clear_with_reporter(common_dir, CacheReporter::default())
    }

    pub fn for_clear_with_reporter(common_dir: &Path, reporter: CacheReporter) -> Self {
        Self {
            conn: RefCell::new(None),
            common_dir: common_dir.to_path_buf(),
            pending: RefCell::new(Vec::new()),
            collecting_page: Cell::new(false),
            disabled: Cell::new(false),
            reporter,
            inject_operational: Cell::new(false),
        }
    }

    /// Whether this handle currently retains SQLite. Exposed as a narrow
    /// ordering seam for platform-sensitive clear tests.
    #[doc(hidden)]
    pub fn connection_is_open(&self) -> bool {
        self.conn.borrow().is_some()
    }

    fn operational_fault(&self, error: &CacheError) {
        self.disabled.set(true);
        self.pending.borrow_mut().clear();
        self.reporter.unavailable(&error.to_string());
    }

    fn inject_operational_fault(&self) -> bool {
        if self.inject_operational.replace(false) {
            self.operational_fault(&CacheError::Sqlite(rusqlite::Error::InvalidQuery));
            true
        } else {
            false
        }
    }
}

impl AnchorCache for CacheStore {
    fn begin_page(&self) {
        if !self.disabled.get() {
            self.pending.borrow_mut().clear();
            self.collecting_page.set(true);
        }
    }

    fn flush_page(&self) -> Result<(), CacheError> {
        if self.disabled.get() {
            return Ok(());
        }
        let writes = std::mem::take(&mut *self.pending.borrow_mut());
        self.collecting_page.set(false);
        if writes.is_empty() {
            return Ok(());
        }
        let result = schema::retry_busy(|| {
            let mut conn_ref = self.conn.borrow_mut();
            let Some(conn) = conn_ref.as_mut() else {
                return Ok(());
            };
            let tx = conn.transaction()?;
            for write in &writes {
                match write {
                    PendingWrite::Fingerprint {
                        key,
                        page_path,
                        anchor_sha,
                        target_path,
                        range_start,
                        range_end,
                        fp,
                        digest,
                    } => {
                        tx.execute("INSERT INTO fingerprint (key_digest,page_path,anchor_sha,target_path,range_start,range_end,fp,row_digest) VALUES (?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(key_digest) DO UPDATE SET page_path=excluded.page_path,anchor_sha=excluded.anchor_sha,target_path=excluded.target_path,range_start=excluded.range_start,range_end=excluded.range_end,fp=excluded.fp,row_digest=excluded.row_digest", params![key,page_path,anchor_sha,target_path,*range_start as i64,*range_end as i64,fp,digest])?;
                    }
                    PendingWrite::Walk {
                        key,
                        page_path,
                        log_output_sha,
                        anchor_sha,
                        path_at_commit,
                        value,
                        digest,
                    } => {
                        tx.execute("INSERT INTO anchor_walk (key_digest,page_path,log_output_sha,anchor_sha,path_at_commit,value,row_digest) VALUES (?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(key_digest) DO UPDATE SET page_path=excluded.page_path,log_output_sha=excluded.log_output_sha,anchor_sha=excluded.anchor_sha,path_at_commit=excluded.path_at_commit,value=excluded.value,row_digest=excluded.row_digest", params![key,page_path,log_output_sha,anchor_sha,path_at_commit,value,digest])?;
                    }
                }
            }
            tx.commit()?;
            Ok(())
        });
        if let Err(ref error) = result {
            self.operational_fault(error);
        }
        result
    }

    fn lookup_fingerprint(
        &self,
        key: &str,
        page_path: &str,
        anchor_sha: &str,
        target_path: &str,
        range_start: u32,
        range_end: u32,
    ) -> Result<Option<String>, CacheError> {
        if self.inject_operational_fault() {
            return Ok(None);
        }
        if self.disabled.get() {
            return Ok(None);
        }
        let conn_ref = self.conn.borrow();
        let Some(conn) = conn_ref.as_ref() else {
            return Ok(None);
        };
        let row = conn.query_row(
            "SELECT page_path, anchor_sha, target_path, range_start, range_end, fp, row_digest
               FROM fingerprint WHERE key_digest = ?1",
            [key],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Vec<u8>>(6)?,
                ))
            },
        );
        // Any error — a missing row included — is a miss, never an error
        // (plan decision 5).
        let (stored_page, stored_anchor, stored_target, stored_start, stored_end, fp, row_digest) =
            match row {
                Ok(row) => row,
                Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
                Err(e) => {
                    drop(conn_ref);
                    let error = CacheError::Sqlite(e);
                    self.operational_fault(&error);
                    return Ok(None);
                }
            };
        // The stored tuple must match the queried tuple field by field...
        if stored_page != page_path
            || stored_anchor != anchor_sha
            || stored_target != target_path
            || stored_start != range_start as i64
            || stored_end != range_end as i64
        {
            return Ok(None);
        }
        // ... and its row_digest must re-derive over (tuple + fp).
        let digest = key::row_digest(
            &[
                page_path,
                anchor_sha,
                target_path,
                &range_start.to_string(),
                &range_end.to_string(),
            ],
            Some(&fp),
        );
        if digest != row_digest {
            return Ok(None);
        }
        Ok(Some(fp))
    }

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
    ) -> Result<(), CacheError> {
        if self.disabled.get() {
            return Ok(());
        }
        let digest = key::row_digest(
            &[
                page_path,
                anchor_sha,
                target_path,
                &range_start.to_string(),
                &range_end.to_string(),
            ],
            Some(fp),
        );
        // Last-write-wins on the key; the row_digest covers the whole row so
        // a served row always re-derives (plan decision 5).
        self.pending.borrow_mut().push(PendingWrite::Fingerprint {
            key: key.into(),
            page_path: page_path.into(),
            anchor_sha: anchor_sha.into(),
            target_path: target_path.into(),
            range_start,
            range_end,
            fp: fp.into(),
            digest,
        });
        if self.collecting_page.get() {
            Ok(())
        } else {
            self.flush_page()
        }
    }

    fn lookup_walk(
        &self,
        key: &str,
        page_path: &str,
        log_output: &str,
    ) -> Result<Option<WalkRow>, CacheError> {
        if self.inject_operational_fault() {
            return Ok(None);
        }
        if self.disabled.get() {
            return Ok(None);
        }
        let conn_ref = self.conn.borrow();
        let Some(conn) = conn_ref.as_ref() else {
            return Ok(None);
        };
        let row = conn.query_row(
            "SELECT page_path, log_output_sha, anchor_sha, path_at_commit, value, row_digest
               FROM anchor_walk WHERE key_digest = ?1",
            [key],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Vec<u8>>(5)?,
                ))
            },
        );
        // Any error — a missing row included — is a miss, never an error
        // (plan decision 5).
        let (stored_page, log_sha, anchor, path_at_commit, value, row_digest) = match row {
            Ok(row) => row,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(e) => {
                drop(conn_ref);
                let error = CacheError::Sqlite(e);
                self.operational_fault(&error);
                return Ok(None);
            }
        };
        // The stored tuple must match the queried tuple: page_path verbatim,
        // log_output re-derived to its sha (the full log output is never
        // stored)...
        if stored_page != page_path || log_sha != key::sha256_hex(log_output.as_bytes()) {
            return Ok(None);
        }
        // ... and the row_digest must re-derive over (tuple + value).
        let digest = key::row_digest(
            &[page_path, &log_sha, &anchor, &path_at_commit],
            value.as_deref(),
        );
        if digest != row_digest {
            return Ok(None);
        }
        Ok(Some(WalkRow {
            anchor_sha: anchor,
            path_at_commit,
            value,
        }))
    }

    fn upsert_walk(
        &self,
        key: &str,
        page_path: &str,
        log_output_sha: &str,
        anchor_sha: &str,
        path_at_commit: &str,
        value: Option<&str>,
    ) -> Result<(), CacheError> {
        if self.disabled.get() {
            return Ok(());
        }
        let digest = key::row_digest(
            &[page_path, log_output_sha, anchor_sha, path_at_commit],
            value,
        );
        // Last-write-wins on the key; the row_digest covers the whole row so
        // a served row always re-derives (plan decision 5).
        self.pending.borrow_mut().push(PendingWrite::Walk {
            key: key.into(),
            page_path: page_path.into(),
            log_output_sha: log_output_sha.into(),
            anchor_sha: anchor_sha.into(),
            path_at_commit: path_at_commit.into(),
            value: value.map(str::to_owned),
            digest,
        });
        if self.collecting_page.get() {
            Ok(())
        } else {
            self.flush_page()
        }
    }

    fn clear(&self) -> Result<(), CacheError> {
        // Close before acquiring the locks: a general handle may retain an
        // open database (Windows refuses removal while a descriptor is out).
        self.pending.borrow_mut().clear();
        drop(self.conn.borrow_mut().take());
        // Exclusive rendezvous first (plan D7/D10): tier drops are
        // destructive work that must exclude readers and publishers.
        // Bounded wait; timeout ⇒ Err ⇒ the caller's single fault line.
        let _rendezvous = rendezvous::acquire_exclusive(&self.common_dir)?;
        // Then the same no-wait init lock the open path holds across
        // probe/quarantine/skew repair/DDL — never interleave with those
        // windows (plan decision 8's racing clause is an error here, which
        // the caller reports as its one fault line).
        let common = DirFd::open(&self.common_dir)?;
        let wiki = common.ensure_private_subtree(Path::new(schema::STORE_DIR_NAME))?;
        wiki.validate_private()?;
        let lock_file = wiki.create_file(schema::INIT_LOCK_FILE_NAME)?;
        match lock_file.try_lock_exclusive() {
            Ok(true) => {}
            Ok(false) => {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "anchor cache init lock held",
                )
                .into());
            }
            // Some platforms surface `WouldBlock` as Err instead of Ok(false)
            // (index/lock.rs precedent).
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Err(e.into()),
            Err(e) => return Err(e.into()),
        }
        // Heal-or-open, then drop BOTH tiers' static tables plus every
        // dynamic `fts_%` child inside one transaction (plan D10). A suspect
        // file quarantines into a fresh store with no tier data to drop; a
        // skewed or valid store clears wholesale — recreating the empty
        // shapes afterwards subsumes skew repair. The meta singleton and the
        // cross-tier store_events ledger survive by construction: neither is
        // a tier table, so [`schema::drop_tier_tables`] never touches them.
        let db = schema::db_path(&self.common_dir);
        match schema::probe(&db)? {
            schema::ProbeOutcome::Missing => {}
            schema::ProbeOutcome::Suspect(_) => schema::quarantine(&db)?,
            schema::ProbeOutcome::Valid | schema::ProbeOutcome::Skew(_) => {
                let conn = schema::open_connection(&db)?;
                let tx = conn.unchecked_transaction()?;
                schema::drop_tier_tables(&tx, schema::Tier::Anchor)?;
                schema::drop_tier_tables(&tx, schema::Tier::Index)?;
                tx.commit()?;
                drop(conn);
                drop(schema::open_connection(&db)?);
            }
        }
        // Delete every stale quarantine rename-aside (`store.sqlite.<stamp>.
        // quarantine`, as produced by schema::quarantine). Best-effort:
        // NotFound is fine (there may be none); anything else surfaces as
        // an error.
        let dir = schema::store_dir(&self.common_dir);
        let aside_prefix = format!("{}.", schema::DB_FILE_NAME);
        match fs::read_dir(&dir) {
            Ok(entries) => {
                for entry in entries {
                    let path = entry.map_err(CacheError::from)?.path();
                    let name = path
                        .file_name()
                        .expect("dir entry has a name")
                        .to_string_lossy();
                    if name.starts_with(&aside_prefix) && name.ends_with(".quarantine") {
                        match fs::remove_file(&path) {
                            Ok(()) => {}
                            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                            Err(e) => return Err(e.into()),
                        }
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        // Explicit unlock mirrors the open path: on some filesystems
        // closing a descriptor does not promptly release the advisory lock
        // for a same-process opener.
        FileExt::unlock(&lock_file)?;
        drop(lock_file);
        // Nothing else goes: the directory itself, the database file and
        // WAL sidecars, and both lock files all stay — the directory hosts
        // fix journals, rendezvous state, and wiki.log (plan D10), so a
        // clear empties tables and never deletes paths.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Shared-layer fault labels never claim a tier (finding F8): both
    /// tiers construct these values, so "anchor cache ..." on an index-tier
    /// failure misattributed every index fault to the anchor cache. The
    /// tier context is each caller's own wrapping.
    #[test]
    fn shared_fault_labels_never_claim_a_tier() {
        let io = CacheError::from(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "gone",
        ));
        let sqlite = CacheError::from(rusqlite::Error::InvalidQuery);
        let corrupt = CacheError::Corrupt("witness".into());
        for error in [io.to_string(), sqlite.to_string(), corrupt.to_string()] {
            assert!(
                !error.contains("anchor"),
                "shared faults must not claim the anchor tier: {error}"
            );
            assert!(
                !error.contains("index"),
                "shared faults must not claim the index tier: {error}"
            );
            assert!(
                error.starts_with("wiki store "),
                "shared faults must name the store layer: {error}"
            );
        }
    }
}
