//! Wiki search index — gix + the merged store's freshness generations
//! (plan merged-store-generations D5/D6).
//!
//! Serving is generation-scoped: every query resolves against one immutable
//! generation (`fts_<gen_id>` children plus `(gen_id, source)`-scoped
//! `gen_paths` joins). Freshness is a canonical-digest lookup; a miss
//! refreshes against the newest generation as delta base and publishes an
//! entirely new one.

use std::path::{Path, PathBuf};

use miette::Result;
use serde::Serialize;

use generations::GenerationsStore;

/// The promoted wikiignore location: a repo-root-relative tracked input,
/// anchored identically to the pre-promotion location (plan D14). The one
/// literal for every index-side consumer (freshness fold, refresh hash).
pub(crate) const WIKIIGNORE_RELPATH: &str = ".wikiignore";

pub mod blob;
pub mod freshness;
pub mod fs_class;
pub mod generations;
pub mod ingest;
pub mod passes;
pub mod search;

/// Walk `start` and its ancestors looking for a `.git` entry (file or dir).
/// Returns the resolved dotgit path (the `.git` dir itself, or the path the
/// `.git` file points at) when found, or `None`.
pub(crate) fn find_dot_git(start: &Path) -> Option<PathBuf> {
    let mut cur = start;
    loop {
        let candidate = cur.join(".git");
        if candidate.is_dir() {
            return Some(candidate);
        }
        if candidate.is_file() {
            // Worktree / submodule: read the `gitdir: <path>` pointer.
            if let Ok(contents) = std::fs::read_to_string(&candidate) {
                let line = contents.trim();
                if let Some(rest) = line.strip_prefix("gitdir:") {
                    let target = PathBuf::from(rest.trim());
                    let resolved = if target.is_absolute() {
                        target
                    } else {
                        cur.join(target)
                    };
                    return Some(resolved);
                }
            }
        }
        cur = cur.parent()?;
    }
}

/// Collapse `.`/`..` segments lexically, preserving the leading root.
///
/// A linked worktree's common dir carries `..` segments — gix returns
/// `…/.git/worktrees/N/../..` (spike S2) — and path joins under it need the
/// normalized form. Mirrors `git::normalize_lexically`, which stays private
/// to its module.
fn normalize_lexically(path: PathBuf) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push(component);
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// Resolve the repository's common git directory for `repo_root`
/// (normalized; spike S2). `None` when discovery fails — callers degrade.
fn resolve_common_dir(repo_root: &Path) -> Option<PathBuf> {
    gix::open(repo_root)
        .ok()
        .map(|repo| normalize_lexically(repo.common_dir().to_path_buf()))
}

/// Process-wide one-line budget for rendezvous degradation warnings: many
/// handles may degrade during one run, but the run prints at most one line
/// (the same diagnostic discipline as the anchor cache reporter).
static RENDEZVOUS_WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn warn_rendezvous_unavailable_once(context: &str, e: &std::io::Error) {
    if !RENDEZVOUS_WARNED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        eprintln!("warning: rendezvous lock unavailable for {context} ({e}); proceeding unordered");
    }
}

/// Process-wide one-line budget for store-availability degradation (plan
/// F6): a mid-repair window in another process makes this run serve from an
/// ephemeral in-memory tier; the run prints at most one line about it.
static STORE_DEGRADED_WARNED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

fn warn_store_degraded_once(reason: &str) {
    if !STORE_DEGRADED_WARNED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        eprintln!("warning: wiki store unavailable ({reason}); serving uncached for this run");
    }
}

fn is_busy_error(err: &crate::cache::CacheError) -> bool {
    matches!(
        err,
        crate::cache::CacheError::Sqlite(rusqlite::Error::SqliteFailure(f, _))
            if f.code == rusqlite::ErrorCode::DatabaseBusy
    )
}

/// Process-wide one-line budget for losing the pinned generation between
/// the gate pin and query time (plan F-B): the run says so once, then
/// answers from a fresh rebuild instead of silently going empty.
static SERVED_LOST_WARNED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

fn warn_served_lost_once() {
    if !SERVED_LOST_WARNED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        eprintln!(
            "warning: served generation no longer readable; rebuilding for this run"
        );
    }
}

/// Stat-only check that the working tree is unchanged since the last verified
/// index state. Resolves `.git`, opens the index DB read-only, and runs
/// Read-only digest gate for callers that only need the freshness verdict:
/// compute the canonical fingerprint and look it up in the merged store.
///
/// Returns `true` only when an unverified-row-safe lookup confirms the
/// exact state as a retained generation. Any error, missing `.git`, missing
/// store file, or stale state returns `false` (fail-open toward doing the
/// work — callers must re-hash when this is `false`). A missing store never
/// gets created here.
///
/// Retained as a public stat helper and exercised by the worktree-freshness
/// integration tests.
#[allow(dead_code)]
pub fn tree_unchanged(repo_root: &Path) -> bool {
    let Some(dot_git) = find_dot_git(repo_root) else {
        return false;
    };
    let Some(common_dir) = resolve_common_dir(repo_root) else {
        return false;
    };
    if !crate::cache::schema::db_path(&common_dir).exists() {
        return false;
    }
    let wikiignore_hash =
        passes::compute_wikiignore_hash(repo_root);
    let fingerprint = match freshness::current_fingerprint(
        repo_root,
        &dot_git,
        None,
        &wikiignore_hash,
    ) {
        Ok(Some(fp)) => fp,
        _ => return false,
    };
    match GenerationsStore::open(&common_dir) {
        Ok(store) => matches!(store.lookup_digest(&fingerprint), Ok(Some(_))),
        Err(_) => false,
    }
}

/// Hard cap on the number of results `wiki "<query>"` will print.
pub const SEARCH_LIMIT: i64 = 3;

#[allow(dead_code)]
const SUGGESTION_LIMIT: i64 = 3;

/// Selects which git snapshot `WikiIndex` reads from.
///
/// The variants are preserved verbatim from the pre-rewrite surface so
/// `commands/{search,summary,check,check_fix,list,mod}.rs` keep compiling
/// unchanged.
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
                    Err(e) => {
                        Err(miette::miette!(e)
                            .wrap_err(format!("failed to read {}", abs.display())))
                    }
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
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

/// Classification of the filesystem hosting `.git/`. Hostile filesystems
/// (overlayfs / NFS / CIFS) disable the dir-mtime Merkle optimization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostileFs {
    No,
    #[allow(dead_code)]
    Yes,
}

/// Public handle to the wiki search index.
///
/// Owns one [`GenerationsStore`] handle (the merged store's index tier),
/// the generation currently being served, and the repo root used to
/// resolve relative paths during result rendering.
pub struct WikiIndex {
    repo_root: PathBuf,
    dot_git: PathBuf,
    common_dir: PathBuf,
    source: DocSource,
    #[allow(dead_code)]
    repo: Option<gix::Repository>,
    store: GenerationsStore,
    served_gen: Option<i64>,
    last_stats: IndexStats,
}

impl WikiIndex {
    /// Convenience constructor used by tests that default to the working
    /// tree.
    #[allow(dead_code)]
    pub fn prepare(repo_root: &Path) -> Result<Self> {
        Self::prepare_for_source(repo_root, DocSource::WorkingTree)
    }

    /// Open the index database for `source`, run the freshness fast
    /// triple, optionally refresh, and return a read-only handle.
    pub fn prepare_for_source(repo_root: &Path, source: DocSource) -> Result<Self> {
        Self::prepare_with_fs_class_inner(repo_root, source, None)
    }

    fn prepare_with_fs_class_inner(
        repo_root: &Path,
        source: DocSource,
        forced_fs_class: Option<HostileFs>,
    ) -> Result<Self> {
        // Locate the .git directory by walking up from repo_root.
        let dot_git = find_dot_git(repo_root).ok_or_else(|| {
            miette::miette!(
                "could not find .git directory under {}",
                repo_root.display()
            )
        })?;

        // All derived state lives in the merged store under the common git
        // dir — one file shared by every linked worktree.
        let common_dir = resolve_common_dir(repo_root).ok_or_else(|| {
            miette::miette!("could not resolve the common git dir of {}", repo_root.display())
        })?;
        let store = GenerationsStore::open(&common_dir)
            .map_err(|e| miette::miette!("failed to open wiki store at {}: {e}",
                crate::cache::schema::db_path(&common_dir).display()))?;
        if store.is_degraded() {
            warn_store_degraded_once("busy mid-init in another process");
        }

        let fs_class = forced_fs_class.unwrap_or_else(|| fs_class::detect(&dot_git));

        let mut index = WikiIndex {
            repo_root: repo_root.to_path_buf(),
            dot_git: dot_git.clone(),
            common_dir: common_dir.clone(),
            source,
            repo: None,
            store,
            served_gen: None,
            last_stats: IndexStats::default(),
        };

        // Digest gate: hash the canonical freshness inputs and look the
        // generation up. A forced HostileFs::Yes skips the gate so the test
        // can observe a Pass 3 full rescan. Any error degrades to a miss —
        // fail-open toward rehash, exactly like the old triple gate.
        if forced_fs_class != Some(HostileFs::Yes) {
            let gate = crate::perf::scope_result(
                "index.fast_gate",
                serde_json::json!({}),
                || -> Result<Option<i64>> {
                    let wikiignore_hash = passes::compute_wikiignore_hash(repo_root);
                    let fingerprint = match freshness::current_fingerprint(
                        repo_root,
                        &dot_git,
                        None,
                        &wikiignore_hash,
                    ) {
                        Ok(Some(fp)) => fp,
                        _ => return Ok(None),
                    };
                    Ok(index
                        .store
                        .lookup_digest(&fingerprint)
                        .ok()
                        .flatten()
                        .map(|generation| generation.gen_id))
                },
            );
            if let Ok(Some(gen_id)) = gate {
                index.served_gen = Some(gen_id);
                return Ok(index);
            }
        }

        // Gate miss ⇒ exclusive rendezvous (plan D7 mode table: refresh
        // publication is an exclusive holder), bounded ~10 s wait.
        let exclusive =
            match crate::cache::rendezvous::acquire_exclusive(&common_dir) {
                Ok(guard) => Some(guard),
                Err(e) => {
                    warn_rendezvous_unavailable_once("index refresh", &e);
                    None
                }
            };
        if exclusive.is_none() {
            // Timeout floor (plan F3): NEVER serve a foreign `newest()` —
            // another worktree's uncommitted corpus is not this run's
            // answer. Scope any served generation to this worktree's own
            // canonical digest; if none is retained, fall back to uncached
            // computation: a full refresh against an ephemeral in-memory
            // tier, answering exactly what an uncontended cold run would.
            let wikiignore_hash = passes::compute_wikiignore_hash(repo_root);
            let own = freshness::current_fingerprint(repo_root, &dot_git, None, &wikiignore_hash)
                .ok()
                .flatten()
                .and_then(|fingerprint| index.store.lookup_digest(&fingerprint).ok().flatten());
            if let Some(generation) = own {
                index.served_gen = Some(generation.gen_id);
                return Ok(index);
            }
            return Self::finish_uncached(index, fs_class);
        }

        // Double-checked locking: a sibling process may have published our
        // exact state while we waited for the lock. A hit skips the refresh
        // entirely; the guard drops before we return either way.
        {
            let wikiignore_hash = passes::compute_wikiignore_hash(repo_root);
            let double_checked = freshness::current_fingerprint(
                repo_root,
                &dot_git,
                None,
                &wikiignore_hash,
            )
            .ok()
            .flatten()
            .and_then(|fingerprint| index.store.lookup_digest(&fingerprint).ok().flatten());
            if let Some(generation) = double_checked {
                index.served_gen = Some(generation.gen_id);
                drop(exclusive);
                return Ok(index);
            }
        }

        let repo = crate::perf::scope_result("index.gix_open", serde_json::json!({}), || {
            gix::open(&index.repo_root).map_err(Box::new)
        })
        .map_err(|e| miette::miette!("gix::open({}) failed: {e}", index.repo_root.display()))?;
        let outcome =
            crate::perf::scope_result("index.refresh", serde_json::json!({}), || {
                passes::refresh(
                    &repo,
                    &index.repo_root,
                    &index.dot_git,
                    &index.store,
                    fs_class,
                )
            });
        let outcome = match outcome {
            Ok(outcome) => outcome,
            // Store-level fault at refresh/publish time (plan F-A): the
            // command's answers must not depend on cache state. One line, a
            // best-effort forced quarantine so the NEXT run recovers on disk
            // too, then this run answers from the uncached ephemeral tier.
            // Non-store faults (gix, parse) still propagate.
            Err(e) => {
                let store_fault = e
                    .downcast_ref::<crate::cache::CacheError>()
                    .is_some_and(|ce| !is_busy_error(ce));
                if store_fault {
                    generations::quarantine_forced(&crate::cache::schema::db_path(
                        &common_dir,
                    ));
                }
                warn_store_degraded_once(&e.to_string());
                return Self::finish_uncached(index, fs_class);
            }
        };
        index.served_gen = Some(outcome.served_gen_id);
        // A conflict-discard means the identical generation already existed:
        // the refresh did no ingest work, so the counters stay at zero
        // (the old CAS-lost contract, expressed through publish).
        if !outcome.conflict_discarded {
            index.last_stats = IndexStats {
                pass3_full_rescans: outcome.pass3_full_rescans,
                fts_retokenizations: outcome.fts_retokenizations,
                pass3_dir_walks: outcome.pass3_dir_walks,
            };

            // Maintenance pass in the same exclusive window (plan D10):
            // recency-liveness eviction, ordered teardown, WAL truncation.
            // Best-effort — a GC failure never fails a successful refresh.
            let gc_started = std::time::Instant::now();
            let gc = index.store.maintain().ok();
            let evicted = gc.as_ref().map(|stats| stats.evicted_gen_ids.len()).unwrap_or(0);
            if gc.is_some() && evicted > 0 {
                crate::perf::log_event(
                    "index.gc",
                    gc_started.elapsed().as_secs_f64() * 1000.0,
                    "ok",
                    serde_json::json!({
                        "evicted": evicted,
                        "generations_after":
                            gc.as_ref().map(|stats| stats.generations_after).unwrap_or(0),
                        "bytes_after": gc.as_ref().map(|stats| stats.bytes_after).unwrap_or(0),
                    }),
                );
            }
        }
        drop(exclusive); // release before serving; no shared is held afterwards
        index.repo = Some(repo);
        Ok(index)
    }

    /// The uncached floor for contended runs (plan F3, cold-store leg):
    /// swap to an ephemeral in-memory tier and run the full refresh against
    /// it — no shared-file contact, no rendezvous needed — so the answers
    /// derive solely from this worktree's git/filesystem state and equal
    /// what an uncontended run would print.
    fn finish_uncached(mut index: WikiIndex, fs_class: HostileFs) -> Result<Self> {
        let mem = GenerationsStore::open_ephemeral()
            .map_err(|e| miette::miette!("ephemeral wiki store failed: {e}"))?;
        index.store = mem;
        index.served_gen = None;

        let repo = crate::perf::scope_result("index.gix_open", serde_json::json!({}), || {
            gix::open(&index.repo_root).map_err(Box::new)
        })
        .map_err(|e| miette::miette!("gix::open({}) failed: {e}", index.repo_root.display()))?;
        let outcome = crate::perf::scope_result("index.refresh_uncached", serde_json::json!({}), || {
            passes::refresh(
                &repo,
                &index.repo_root,
                &index.dot_git,
                &index.store,
                fs_class,
            )
        })
        .map_err(|e| miette::miette!("uncached refresh failed: {e}"))?;
        index.served_gen = Some(outcome.served_gen_id);
        if !outcome.conflict_discarded {
            index.last_stats = IndexStats {
                pass3_full_rescans: outcome.pass3_full_rescans,
                fts_retokenizations: outcome.fts_retokenizations,
                pass3_dir_walks: outcome.pass3_dir_walks,
            };
        }
        index.repo = Some(repo);
        Ok(index)
    }

    /// The generation this handle serves, if any (a contended cold start
    /// may have nothing to serve).
    fn served(&self) -> Option<i64> {
        self.served_gen
    }

    /// Serve one query under the shared rendezvous (plan D7: search/list/
    /// summary are shared holders, acquired briefly around query serving
    /// only — never held across a refresh this run triggered, and never
    /// upgraded to exclusive in-process). On bounded-wait timeout the floor
    /// is serving unordered: the deferred read transaction still gives one
    /// consistent WAL snapshot, and generations are immutable so a
    /// concurrent publish cannot invalidate the served generation.
    fn serve_shared_with<T>(
        &self,
        context: &str,
        f: impl FnOnce(&rusqlite::Connection) -> rusqlite::Result<Option<T>>,
    ) -> Result<Option<T>> {
        let Some(gen_id) = self.served() else {
            return Ok(None);
        };
        // Try-once for the shared courtesy: a held exclusive means a
        // publisher is mid-refresh; waiting out its full budget would stall
        // every query by design-latency. Generations are immutable and the
        // deferred read txn gives a consistent WAL snapshot, so on
        // contention this run proceeds unordered immediately (one-line
        // budget applies).
        let _guard = match crate::cache::rendezvous::try_acquire_shared(&self.common_dir) {
            Ok(Some(guard)) => Some(guard),
            Ok(None) => {
                warn_rendezvous_unavailable_once(context, &std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "held by a publisher",
                ));
                None
            }
            Err(e) => {
                warn_rendezvous_unavailable_once(context, &e);
                None
            }
        };
        // Plan D5 fail-open, verified inside the same read transaction as
        // the query: a generation evicted between the lock-free gate pin
        // and this acquisition (or whose fts child was torn down) is an
        // ordinary miss — the caller degrades to its empty shape — never a
        // `no such table` command error.
        self.store
            .read_txn(|conn| {
                if !GenerationsStore::verify_served(conn, gen_id)? {
                    return Ok(None);
                }
                f(conn)
            })
            .map_err(|e| miette::miette!("{context}: {e}"))
    }

    /// The F-B floor: a serve-time verification miss (generation evicted
    /// or unreadable between gate pin and query) is a REBUILD, not a silent
    /// empty. One diagnostic line, then a fresh prepare — whose own
    /// gate/refresh republishes this worktree's state — answers the query.
    fn rebuild_floor(&self, context: &str) -> Result<Option<WikiIndex>> {
        warn_served_lost_once();
        let rebuilt = Self::prepare_for_source(&self.repo_root, self.source)?;
        if rebuilt.served().is_none() {
            let _ = context;
            return Ok(None);
        }
        Ok(Some(rebuilt))
    }

    /// Resolve a single page by title or alias (case-insensitive), or by a
    /// repo-relative path / `.md` file reference.
    pub fn resolve_page(&self, input: &str) -> Result<Option<ResolvedPage>> {
        let Some(gen_id) = self.served() else {
            return Ok(None);
        };
        // The closure wraps its result in Some (search/list/suggest
        // pattern): a genuine title/alias/path no-match surfaces as
        // Some(None) — an ORDINARY answer — so the helper's outer None can
        // mean exclusively a serve-time verification miss (plan F-B round
        // 2): interactive misses must never take the rebuild floor.
        if let Some(resolved) = self.serve_shared_with("resolve_page", |conn| {
            search::resolve_page(conn, &self.repo_root, gen_id, self.source, input).map(Some)
        })? {
            return Ok(resolved);
        }
        let Some(rebuilt) = self.rebuild_floor("resolve_page")? else {
            return Ok(None);
        };
        let gen_id = rebuilt.served().expect("rebuild_floor checked");
        rebuilt
            .store
            .read_txn(|conn| search::resolve_page(conn, &self.repo_root, gen_id, self.source, input))
            .map_err(|e| miette::miette!("resolve_page: {e}"))
    }

    /// BM25-weighted search, paginated, returning `(rows, total)`.
    pub fn search_weighted(
        &self,
        query: &str,
        limit: i64,
        offset: usize,
    ) -> Result<(Vec<SearchResult>, usize)> {
        let Some(gen_id) = self.served() else {
            return Ok((Vec::new(), 0));
        };
        let limit_usize = if limit < 0 { 0 } else { limit as usize };
        let served = self
            .serve_shared_with("search_weighted", |conn| {
                search::search_weighted(conn, gen_id, self.source, query, limit_usize, offset)
                    .map(Some)
            })?;
        let (mut rows, total) = match served {
            Some(pair) => pair,
            None => {
                let Some(rebuilt) = self.rebuild_floor("search_weighted")? else {
                    return Ok((Vec::new(), 0));
                };
                let gen_id = rebuilt.served().expect("rebuild_floor checked");
                rebuilt
                    .store
                    .read_txn(|conn| {
                        search::search_weighted(
                            conn,
                            gen_id,
                            self.source,
                            query,
                            limit_usize,
                            offset,
                        )
                    })
                    .map_err(|e| miette::miette!("search_weighted: {e}"))?
            }
        };
        // Render `file` as an absolute path so `format_search_result` can
        // `strip_prefix(repo_root)` to produce repo-relative output.
        for r in &mut rows {
            let p = std::path::Path::new(&r.file);
            if !p.is_absolute() {
                r.file = self.repo_root.join(&r.file).to_string_lossy().into_owned();
            }
        }
        Ok((rows, total))
    }

    /// The shared-rendezvous leg of `list_pages`: verified read under the
    /// courtesy lock; `None` ⇒ serve-time verification miss (plan F-B).
    fn serve_list_pages(
        &self,
        gen_id: i64,
        src: &str,
    ) -> Result<Option<RawPageRows>> {
        self.serve_shared_with("list_pages", |conn| {
            Self::list_pages_raw(conn, gen_id, src).map(Some)
        })
    }

    /// The rebuild leg: same query against a fresh handle's store, no
    /// rendezvous (this process just published it).
    fn serve_list_pages_raw(&self, gen_id: i64, src: &str) -> Result<RawPageRows> {
        self.store
            .read_txn(|conn| Self::list_pages_raw(conn, gen_id, src))
            .map_err(|e| miette::miette!("list_pages: {e}"))
    }

    fn list_pages_raw(
        conn: &rusqlite::Connection,
        gen_id: i64,
        src: &str,
    ) -> rusqlite::Result<RawPageRows> {
        let mut stmt = conn.prepare(
            "SELECT p.path_rel, b.title, b.summary, b.aliases_text, b.tags_text
             FROM blobs b
             JOIN gen_paths p ON p.oid = b.oid AND p.gen_id = ?1
             WHERE p.source = ?2 AND b.title <> '' AND b.summary <> ''
             ORDER BY b.title COLLATE NOCASE, p.path_rel",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![gen_id, src], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Enumerate wiki pages (non-empty title and summary) for this index's
    /// source, ordered by title then path. Optionally filtered to a single
    /// tag (case-insensitive, exact-token), then offset/limit applied.
    pub fn list_pages(
        &self,
        tag: Option<&str>,
        offset: u64,
        limit: Option<u64>,
    ) -> Result<Vec<PageRow>> {
        let Some(gen_id) = self.served() else {
            return Ok(Vec::new());
        };
        let src = generations::source_sql(self.source.gen_source());
        let raw_rows = self.serve_list_pages(gen_id, src)?;
        let raw_rows = match raw_rows {
            Some(rows) => rows,
            None => {
                let Some(rebuilt) = self.rebuild_floor("list_pages")? else {
                    return Ok(Vec::new());
                };
                let gen_id = rebuilt.served().expect("rebuild_floor checked");
                rebuilt.serve_list_pages_raw(gen_id, src)?
            }
        };


        let tag_lc = tag.map(|t| t.to_lowercase());
        let split =
            |s: &str| -> Vec<String> { s.split_whitespace().map(|t| t.to_string()).collect() };

        let mut out: Vec<PageRow> = Vec::new();
        let mut skipped: u64 = 0;
        for (path_rel, title, summary, aliases_text, tags_text) in raw_rows {
            let tags = split(&tags_text);
            if let Some(ref tag_lc) = tag_lc
                && !tags.iter().any(|t| t.to_lowercase() == *tag_lc)
            {
                continue;
            }
            if skipped < offset {
                skipped += 1;
                continue;
            }
            out.push(PageRow {
                path_rel,
                title,
                summary,
                aliases: split(&aliases_text),
                tags,
            });
            if let Some(n) = limit
                && out.len() as u64 >= n
            {
                break;
            }
        }
        Ok(out)
    }

    /// Up to [`SUGGESTION_LIMIT`] BM25 suggestions for a missed lookup.
    pub fn suggest(&self, query: &str) -> Result<Vec<SearchResult>> {
        let Some(gen_id) = self.served() else {
            return Ok(Vec::new());
        };
        let served = self
            .serve_shared_with("suggest", |conn| {
                search::search_weighted(
                    conn,
                    gen_id,
                    self.source,
                    query,
                    SUGGESTION_LIMIT as usize,
                    0,
                )
                .map(Some)
            })?;
        match served {
            Some((rows, _)) => Ok(rows),
            None => {
                let Some(rebuilt) = self.rebuild_floor("suggest")? else {
                    return Ok(Vec::new());
                };
                let gen_id = rebuilt.served().expect("rebuild_floor checked");
                rebuilt
                    .store
                    .read_txn(|conn| {
                        search::search_weighted(
                            conn,
                            gen_id,
                            self.source,
                            query,
                            SUGGESTION_LIMIT as usize,
                            0,
                        )
                        .map(|(rows, _)| rows)
                    })
                    .map_err(|e| miette::miette!("suggest: {e}"))
            }
        }
    }

    /// Open the index for `source`, injecting a filesystem classification
    /// so tests can force `HostileFs::Yes` without needing a real overlayfs.
    ///
    /// Phase 3 wires this into `fs_class.rs` detection; until then, bodies
    /// stay `unimplemented!()`.
    #[allow(dead_code)]
    pub fn prepare_with_fs_class(
        repo_root: &Path,
        source: DocSource,
        fs_class: HostileFs,
    ) -> Result<Self> {
        Self::prepare_with_fs_class_inner(repo_root, source, Some(fs_class))
    }

    /// Dump the served generation's path rows joined to their blob titles,
    /// sorted by `path_rel`. Test-only diagnostic used by
    /// `tests/index_parity.rs`.
    #[allow(dead_code)]
    pub fn debug_dump_paths(&self) -> Result<Vec<(String, Source, String)>> {
        let Some(gen_id) = self.served() else {
            return Ok(Vec::new());
        };
        let conn = self.store.conn();
        let mut stmt = conn
            .prepare(
                "SELECT p.path_rel, p.source, b.title
                 FROM gen_paths p JOIN blobs b ON b.oid = p.oid
                 WHERE p.gen_id = ?1
                 ORDER BY p.path_rel ASC",
            )
            .map_err(|e| miette::miette!("debug_dump_paths prepare: {e}"))?;
        let rows = stmt
            .query_map([gen_id], |row| {
                let path: String = row.get(0)?;
                let source_lit: String = row.get(1)?;
                let title: String = row.get(2)?;
                let source = generations::source_from_sql(&source_lit)
                    .unwrap_or(Source::Worktree);
                Ok((path, source, title))
            })
            .map_err(|e| miette::miette!("debug_dump_paths query: {e}"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| miette::miette!("debug_dump_paths collect: {e}"))
    }

    /// Count global `blobs` rows and the served generation's `gen_paths`
    /// rows for a given OID. Test-only diagnostic used by
    /// `tests/index_promotion.rs` and `tests/index_rename_clobber.rs`.
    #[allow(dead_code)]
    pub fn debug_blob_path_counts(&self, oid: &str) -> Result<(usize, usize)> {
        let conn = self.store.conn();
        let blob_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM blobs WHERE oid = ?1", [oid], |r| {
                r.get(0)
            })
            .map_err(|e| miette::miette!("blob_count: {e}"))?;
        let path_count = match self.served() {
            Some(gen_id) => conn
                .query_row(
                    "SELECT COUNT(*) FROM gen_paths WHERE oid = ?1 AND gen_id = ?2",
                    rusqlite::params![oid, gen_id],
                    |r| r.get(0),
                )
                .map_err(|e| miette::miette!("path_count: {e}"))?,
            None => 0,
        };
        Ok((blob_count as usize, path_count as usize))
    }

    /// Return diagnostic counters accumulated during the last refresh.
    ///
    /// Exposed for `index_hostile_fs` (asserts `pass3_full_rescans > 0`) and
    /// `index_rename` (asserts `fts_retokenizations == 0`).  Phase 3 fills in
    /// real bookkeeping; until then the body is `unimplemented!()`.
    #[allow(dead_code)]
    pub fn stats(&self) -> IndexStats {
        self.last_stats
    }
}

/// Raw `list_pages` projection before tag/offset/limit post-processing.
type RawPageRows = Vec<(String, String, String, String, String)>;

/// One wiki page row enumerated by [`WikiIndex::list_pages`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageRow {
    pub path_rel: String,
    pub title: String,
    pub summary: String,
    pub aliases: Vec<String>,
    pub tags: Vec<String>,
}

/// Diagnostic counters from the most recent refresh pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IndexStats {
    /// Number of times Pass 3 performed a full rescan (hostile filesystems
    /// disable mtime-based carry-forward entirely).
    pub pass3_full_rescans: u64,
    /// Blobs newly tokenized during the most recent refresh — zero for a
    /// pure rename (content-addressing means an existing oid never re-parses).
    pub fts_retokenizations: u64,
    /// Number of directories Pass 3 descended into and walked. There is no
    /// clean-dir skip anymore; carry-forward is per file.
    pub pass3_dir_walks: u64,
}

#[cfg(test)]
mod list_pages_tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    fn git(args: &[&str], cwd: &Path) {
        let status = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .status()
            .expect("git spawns");
        assert!(status.success(), "git {args:?} failed");
    }

    fn create_file(root: &Path, rel: &str, body: &str) {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("mkdir parents");
        }
        fs::write(path, body).expect("write file");
    }

    fn page(title: &str, summary: &str, tags: &str) -> String {
        format!("---\ntitle: {title}\nsummary: {summary}\ntags: [{tags}]\n---\n\nbody\n")
    }

    fn commit_repo(root: &Path) {
        git(&["init", "-q"], root);
        git(&["config", "user.email", "t@t"], root);
        git(&["config", "user.name", "t"], root);
        git(&["add", "-A"], root);
        git(&["commit", "-q", "-m", "init"], root);
    }

    #[test]
    fn only_pages_with_title_and_summary() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        create_file(root, "good.md", &page("Good", "has both", ""));
        create_file(root, "no_summary.md", "---\ntitle: NoSummary\n---\nbody\n");
        create_file(root, "plain.md", "no frontmatter here\n");
        commit_repo(root);

        let index = WikiIndex::prepare_for_source(root, DocSource::Head).unwrap();
        let rows = index.list_pages(None, 0, None).unwrap();
        let titles: Vec<&str> = rows.iter().map(|r| r.title.as_str()).collect();
        assert_eq!(titles, vec!["Good"]);
    }

    #[test]
    fn source_filtering_excludes_uncommitted() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        create_file(root, "committed.md", &page("Committed", "s", ""));
        commit_repo(root);
        // Add an uncommitted page; HEAD must not see it.
        create_file(root, "fresh.md", &page("Fresh", "s", ""));

        let head = WikiIndex::prepare_for_source(root, DocSource::Head).unwrap();
        let head_titles: Vec<String> = head
            .list_pages(None, 0, None)
            .unwrap()
            .into_iter()
            .map(|r| r.title)
            .collect();
        assert_eq!(head_titles, vec!["Committed".to_string()]);

        let wt = WikiIndex::prepare_for_source(root, DocSource::WorkingTree).unwrap();
        let wt_titles: Vec<String> = wt
            .list_pages(None, 0, None)
            .unwrap()
            .into_iter()
            .map(|r| r.title)
            .collect();
        assert!(wt_titles.contains(&"Committed".to_string()));
        assert!(wt_titles.contains(&"Fresh".to_string()));
    }

    #[test]
    fn tag_filter_exact_token_case_insensitive() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        create_file(root, "foo.md", &page("FooPage", "s", "foo"));
        create_file(root, "cap.md", &page("CapPage", "s", "Foo"));
        commit_repo(root);

        let index = WikiIndex::prepare_for_source(root, DocSource::Head).unwrap();

        // Prefix "fo" must NOT match exact token "foo".
        let none = index.list_pages(Some("fo"), 0, None).unwrap();
        assert!(none.is_empty(), "prefix should not match: {none:?}");

        // Filter "foo" matches both "foo" and "Foo" (case-insensitive).
        let mut both: Vec<String> = index
            .list_pages(Some("foo"), 0, None)
            .unwrap()
            .into_iter()
            .map(|r| r.title)
            .collect();
        both.sort();
        assert_eq!(both, vec!["CapPage".to_string(), "FooPage".to_string()]);
    }

    /// Round-2 F1 witness: on a warm, healthy store, a genuine
    /// title/alias no-match is an ORDINARY answer — it must not fire the
    /// served-generation-lost warning or take the rebuild floor (the
    /// pre-fix behavior rebuilt on every interactive miss).
    #[test]
    fn warm_resolve_miss_does_not_trigger_rebuild_floor() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        create_file(root, "real.md", &page("Real Page", "a real summary", ""));
        commit_repo(root);

        let index = WikiIndex::prepare_for_source(root, DocSource::WorkingTree).unwrap();
        let warned_before = SERVED_LOST_WARNED.load(std::sync::atomic::Ordering::SeqCst);

        let miss = index.resolve_page("NoSuchPageAnywhere").unwrap();
        assert!(miss.is_none(), "genuine miss still resolves to None");
        assert_eq!(
            warned_before,
            SERVED_LOST_WARNED.load(std::sync::atomic::Ordering::SeqCst),
            "an ordinary resolve miss must never fire the rebuild warning"
        );
    }

    /// F2 witness (end to end): a handle whose served generation is
    /// evicted by GC churn while held must degrade its queries to empty —
    /// never surface `no such table: fts_N` as a command error.
    #[test]
    fn query_under_held_handle_survives_gc_churn() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        create_file(root, "keep.md", &page("Keep", "kept summary", ""));
        // Body carries a distinctive token the rebuild assertions query for.
        // `page()` writes body text "body"; search uses it below.
        commit_repo(root);

        let index = WikiIndex::prepare_for_source(root, DocSource::WorkingTree).unwrap();
        assert!(
            index.search_weighted("body", 10, 0).is_ok(),
            "sanity: serving works before the churn"
        );

        // Churn eleven same-hour generations past the pinned one.
        use crate::index::generations::{
            GenPathRow, PublishCandidate, StateFingerprint,
        };
        for seed in 2u8..=12 {
            let fingerprint = StateFingerprint {
                head_oid: format!("{:040x}", seed as u64 + 1),
                head_tree_oid: format!("{:040x}", seed as u64 + 2),
                index_checksum: [seed; 20],
                wikiignore_hash: [seed ^ 0xA5; 20],
                worktree_sig: [seed; 32],
            };
            let oid = BlobOid(format!("{:040x}", seed as u64));
            index
                .store
                .publish(PublishCandidate {
                    fingerprint,
                    publisher: Some("churn-test".into()),
                    paths: vec![GenPathRow {
                        source: Source::Worktree,
                        path_rel: "shared.md".into(),
                        oid: oid.clone(),
                        parent_dir: String::new(),
                        stat_mtime_ns: Some(seed as i64),
                    }],
                    new_blobs: vec![(
                        oid,
                        crate::index::ingest::WikiBlobFields {
                            title: format!("Churn {seed}"),
                            summary: "churn corpus".into(),
                            body: "\nbody\n".into(),
                            aliases_text: String::new(),
                            tags_text: String::new(),
                            keywords_text: String::new(),
                        },
                    )],
                })
                .unwrap();
        }
        index.store.maintain().unwrap();

        // The pinned generation may now be evicted. Per plan D5 + F-B this
        // is a REBUILD miss, not a silent empty: every query must answer
        // Ok with THIS worktree's correct corpus (keep.md), never Err and
        // never a false no-match.
        let (rows, total) = index.search_weighted("kept", 10, 0).unwrap();
        assert_eq!(
            total, 1,
            "serve-miss must rebuild and return correct results, got {rows:?}"
        );
        assert!(rows.iter().any(|r| r.title == "Keep"));

        let page = index
            .resolve_page("Keep")
            .unwrap()
            .expect("rebuilt resolve must find Keep");
        assert_eq!(page.title, "Keep");

        let pages = index.list_pages(None, 0, None).unwrap();
        assert!(
            pages.iter().any(|p| p.title == "Keep"),
            "rebuilt listing must contain Keep, got {pages:?}"
        );

        // And the escalation is loud exactly where it should be: the churn
        // genuinely fired the rebuild warning (the warm-miss twin asserts
        // the negative side).
        assert!(SERVED_LOST_WARNED.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn limit_and_offset_bound_results() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        create_file(root, "a.md", &page("Apple", "s", ""));
        create_file(root, "b.md", &page("Banana", "s", ""));
        create_file(root, "c.md", &page("Cherry", "s", ""));
        commit_repo(root);

        let index = WikiIndex::prepare_for_source(root, DocSource::Head).unwrap();

        let first_two: Vec<String> = index
            .list_pages(None, 0, Some(2))
            .unwrap()
            .into_iter()
            .map(|r| r.title)
            .collect();
        assert_eq!(first_two, vec!["Apple".to_string(), "Banana".to_string()]);

        let offset_one: Vec<String> = index
            .list_pages(None, 1, Some(1))
            .unwrap()
            .into_iter()
            .map(|r| r.title)
            .collect();
        assert_eq!(offset_one, vec!["Banana".to_string()]);
    }
}
