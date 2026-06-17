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
use rusqlite::{Connection, OpenFlags};
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

/// True for [`schema::bootstrap`] errors that mean the cache DB should be
/// deleted and rebuilt: a schema-version mismatch (`InvalidQuery`, the
/// sentinel `bootstrap` returns for it) or a corrupt file (`NotADatabase`).
fn rebuildable_bootstrap_error(e: &rusqlite::Error) -> bool {
    matches!(e, rusqlite::Error::InvalidQuery)
        || matches!(
            e,
            rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error {
                    code: rusqlite::ffi::ErrorCode::NotADatabase,
                    ..
                },
                _,
            )
        )
}

/// The `.wiki/` directory anchored at the repo root, where the index cache DB
/// and its sidecar files live alongside the mesh store.
pub(crate) fn wiki_dir(repo_root: &Path) -> PathBuf {
    repo_root.join(".wiki")
}

/// Path of the index cache database under `.wiki/`.
pub(crate) fn index_db_path(repo_root: &Path) -> PathBuf {
    wiki_dir(repo_root).join("wiki-index.sqlite")
}

/// Stat-only check that the working tree is unchanged since the last verified
/// index state. Resolves `.git`, opens the index DB read-only, and runs
/// [`freshness::fast_gate`].
///
/// Returns `true` only when the gate confirms an unchanged tree. Any error,
/// missing `.git`, or stale state returns `false` (fail-open toward doing the
/// work — callers must re-hash when this is `false`).
///
/// No longer consulted by `plan_mesh_follows` (the anchor-staleness pass must
/// re-hash even on a stat-clean tree, since committed source edits to non-`.md`
/// files are invisible to this markdown-mtime gate). Retained as a public stat
/// helper and exercised by the worktree-freshness integration tests.
#[allow(dead_code)]
pub fn tree_unchanged(repo_root: &Path) -> bool {
    let Some(dot_git) = find_dot_git(repo_root) else {
        return false;
    };
    let db_path = index_db_path(repo_root);
    // Open read-only; if the DB does not exist yet there is no verified state.
    let conn = match Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY) {
        Ok(c) => c,
        Err(_) => return false,
    };
    matches!(freshness::fast_gate(&dot_git, &conn), Ok(Some(_)))
}

/// Hard cap on the number of results `wiki "<query>"` will print.
pub const SEARCH_LIMIT: i64 = 3;

#[allow(dead_code)]
const SUGGESTION_LIMIT: i64 = 3;

/// Selects which git snapshot `WikiIndex` reads from.
///
/// The variants are preserved verbatim from the pre-rewrite surface so
/// `commands/{search,summary,check,check_fix,mesh/scaffold,list,mod}.rs`
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
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
/// The pre-rewrite version held a Tokio current-thread runtime plus a
/// Turso connection. The new version owns a rusqlite connection and the
/// repo root used to resolve relative paths during result rendering.
pub struct WikiIndex {
    repo_root: PathBuf,
    dot_git: PathBuf,
    source: DocSource,
    #[allow(dead_code)]
    repo: Option<gix::Repository>,
    conn: Connection,
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

        // The cache DB and its sidecars live under `.wiki/` alongside the mesh
        // store. Ensure the directory exists before opening for write.
        let wiki = wiki_dir(repo_root);
        std::fs::create_dir_all(&wiki)
            .map_err(|e| miette::miette!("failed to create {}: {e}", wiki.display()))?;
        let db_path = index_db_path(repo_root);
        let open = || {
            Connection::open_with_flags(
                &db_path,
                OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_READ_WRITE,
            )
            .map_err(|e| miette::miette!("failed to open wiki index at {}: {e}", db_path.display()))
        };
        let mut conn = open()?;

        if let Err(e) = schema::bootstrap(&conn) {
            // A schema-version mismatch (InvalidQuery) means the cache was
            // written by a different CLI version; NotADatabase means the
            // file is corrupt. Either way the DB is purely derived data, so
            // discard and rebuild it rather than failing the command. Two
            // racing processes may both take this path — POSIX unlink keeps
            // each working on its own inode, so the worst case is one
            // redundant rebuild, never a corrupt result.
            if rebuildable_bootstrap_error(&e) {
                drop(conn);
                // Sidecars first, DB file last: a crash mid-deletion must
                // not leave an old WAL behind for a fresh DB to replay.
                for suffix in ["-shm", "-wal", ""] {
                    let mut p = db_path.clone().into_os_string();
                    p.push(suffix);
                    let _ = std::fs::remove_file(PathBuf::from(p));
                }
                conn = open()?;
                schema::bootstrap(&conn)
                    .map_err(|e| miette::miette!("schema bootstrap failed after rebuild: {e}"))?;
            } else {
                return Err(miette::miette!("schema bootstrap failed: {e}"));
            }
        }

        let fs_class = forced_fs_class.unwrap_or_else(|| fs_class::detect(&dot_git));

        let mut index = WikiIndex {
            repo_root: repo_root.to_path_buf(),
            dot_git: dot_git.clone(),
            source,
            repo: None,
            conn,
            last_stats: IndexStats::default(),
        };

        // Fast triple gate — stat-only path with no gix open.
        // A forced HostileFs::Yes skips the gate so the test can observe a
        // Pass 3 full rescan.
        if forced_fs_class != Some(HostileFs::Yes) {
            // Wrap the gate in a perf span: its worktree leg walks and stats the
            // whole repo on every invocation, the single largest unmeasured
            // per-command cost. The span surfaces that cost in wiki.log so the
            // benchmark can attribute it to the prepare term.
            let gate = crate::perf::scope_result(
                "index.fast_gate",
                serde_json::json!({}),
                || freshness::fast_gate(&index.dot_git, &index.conn),
            );
            match gate {
                Ok(Some(_)) => return Ok(index),
                Ok(None) => {}
                Err(_) => {}
            }
        }

        // Try to acquire the refresh lock; on contention serve the existing
        // snapshot without blocking.
        let lock = lock::try_acquire(&wiki)
            .map_err(|e| miette::miette!("refresh lock acquire failed: {e}"))?;
        if lock.is_none() {
            return Ok(index);
        }

        let repo = crate::perf::scope_result("index.gix_open", serde_json::json!({}), || {
            gix::open(&index.repo_root).map_err(Box::new)
        })
        .map_err(|e| miette::miette!("gix::open({}) failed: {e}", index.repo_root.display()))?;
        let outcome = crate::perf::scope_result("index.refresh", serde_json::json!({}), || {
            passes::refresh(
                &repo,
                &index.repo_root,
                &index.dot_git,
                &mut index.conn,
                fs_class,
            )
        })
        .map_err(|e| miette::miette!("refresh failed: {e}"))?;
        if !outcome.cas_lost {
            index.last_stats = IndexStats {
                pass3_full_rescans: outcome.pass3_full_rescans,
                fts_retokenizations: outcome.fts_retokenizations,
                pass3_dir_walks: outcome.pass3_dir_walks,
            };
        }
        index.repo = Some(repo);
        // `lock` releases on drop here.
        Ok(index)
    }

    /// Resolve a single page by title or alias (case-insensitive), or by a
    /// repo-relative path / `.md` file reference.
    pub fn resolve_page(&self, input: &str) -> Result<Option<ResolvedPage>> {
        search::resolve_page(&self.conn, &self.repo_root, self.source, input)
            .map_err(|e| miette::miette!("resolve_page: {e}"))
    }

    /// BM25-weighted search, paginated, returning `(rows, total)`.
    pub fn search_weighted(
        &self,
        query: &str,
        limit: i64,
        offset: usize,
    ) -> Result<(Vec<SearchResult>, usize)> {
        let limit_usize = if limit < 0 { 0 } else { limit as usize };
        let (mut rows, total) =
            search::search_weighted(&self.conn, self.source, query, limit_usize, offset)
                .map_err(|e| miette::miette!("search_weighted: {e}"))?;
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

    /// Enumerate wiki pages (non-empty title and summary) for this index's
    /// source, ordered by title then path. Optionally filtered to a single
    /// tag (case-insensitive, exact-token), then offset/limit applied.
    pub fn list_pages(
        &self,
        tag: Option<&str>,
        offset: u64,
        limit: Option<u64>,
    ) -> Result<Vec<PageRow>> {
        let src = search::source_filter_id(self.source);
        let mut stmt = self
            .conn
            .prepare(
                "SELECT p.path_rel, b.title, b.summary, b.aliases_text, b.tags_text
                 FROM blobs b
                 JOIN paths p ON p.oid = b.oid
                 WHERE p.source = ?1 AND b.title <> '' AND b.summary <> ''
                 ORDER BY b.title COLLATE NOCASE, p.path_rel",
            )
            .map_err(|e| miette::miette!("list_pages prepare: {e}"))?;
        let rows = stmt
            .query_map(rusqlite::params![src], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                ))
            })
            .map_err(|e| miette::miette!("list_pages query: {e}"))?;

        let tag_lc = tag.map(|t| t.to_lowercase());
        let split =
            |s: &str| -> Vec<String> { s.split_whitespace().map(|t| t.to_string()).collect() };

        let mut out: Vec<PageRow> = Vec::new();
        let mut skipped: u64 = 0;
        for row in rows {
            let (path_rel, title, summary, aliases_text, tags_text) =
                row.map_err(|e| miette::miette!("list_pages row: {e}"))?;
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
        let (rows, _total) =
            search::search_weighted(&self.conn, self.source, query, SUGGESTION_LIMIT as usize, 0)
                .map_err(|e| miette::miette!("suggest: {e}"))?;
        Ok(rows)
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

    /// Dump every `paths` row joined to its blob title, sorted by `path_rel`.
    /// Test-only diagnostic used by `tests/index_parity.rs`.
    #[allow(dead_code)]
    pub fn debug_dump_paths(&self) -> Result<Vec<(String, Source, String)>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT p.path_rel, p.source, b.title
                 FROM paths p JOIN blobs b ON b.oid = p.oid
                 ORDER BY p.path_rel ASC",
            )
            .map_err(|e| miette::miette!("debug_dump_paths prepare: {e}"))?;
        let rows = stmt
            .query_map([], |row| {
                let path: String = row.get(0)?;
                let source: i64 = row.get(1)?;
                let title: String = row.get(2)?;
                let s = match source {
                    0 => Source::Tree,
                    1 => Source::Index,
                    2 => Source::Worktree,
                    _ => Source::Worktree,
                };
                Ok((path, s, title))
            })
            .map_err(|e| miette::miette!("debug_dump_paths query: {e}"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| miette::miette!("debug_dump_paths collect: {e}"))
    }

    /// Count `blobs` and `paths` rows for a given OID.
    /// Test-only diagnostic used by `tests/index_promotion.rs`.
    #[allow(dead_code)]
    pub fn debug_blob_path_counts(&self, oid: &str) -> Result<(usize, usize)> {
        let blob_count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM blobs WHERE oid = ?1", [oid], |r| {
                r.get(0)
            })
            .map_err(|e| miette::miette!("blob_count: {e}"))?;
        let path_count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM paths WHERE oid = ?1", [oid], |r| {
                r.get(0)
            })
            .map_err(|e| miette::miette!("path_count: {e}"))?;
        Ok((blob_count as usize, path_count as usize))
    }

    /// Count `dir_mtimes` rows.
    /// Test-only diagnostic used by `tests/index_hostile_dir_mtimes.rs` and
    /// `tests/index_dir_mtimes_prune.rs`.
    #[allow(dead_code)]
    pub fn debug_dir_mtimes_count(&self) -> Result<usize> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM dir_mtimes", [], |r| r.get(0))
            .map_err(|e| miette::miette!("dir_mtimes count: {e}"))?;
        Ok(count as usize)
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
///
/// All counts are zero when the index was opened read-only without refreshing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IndexStats {
    /// Number of times Pass 3 performed a full directory rescan
    /// (rather than the dir-mtime Merkle short-circuit).
    pub pass3_full_rescans: u64,
    /// Number of FTS rows that were deleted then re-inserted (re-tokenized)
    /// during the most recent refresh.  A rename must not bump this counter.
    pub fts_retokenizations: u64,
    /// Number of directories Pass 3 actually descended into and stat/hashed
    /// markdown files for. Directories whose mtime matched the recorded
    /// `dir_mtimes` entry are skipped and do not contribute.
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
