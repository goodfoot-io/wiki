use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use miette::{Context, IntoDiagnostic, Result, miette};
use regex::RegexBuilder;
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::runtime::{Builder as RuntimeBuilder, Runtime};
use turso::{Builder, Connection, Row, Rows, params, params_from_iter};

use crate::commands::{looks_like_path, normalize_repo_relative_path, resolve_link_path};
use crate::frontmatter::parse_frontmatter;
use crate::git::{
    changed_paths_between, git_acceleration_state, has_staged_changes, has_tracked_files,
    has_unstaged_changes, head_sha, repo_inventory, untracked_paths, working_tree_changed_paths,
};
use crate::parser::{LinkKind, parse_fragment_links, parse_wikilinks};
use crate::perf;

const SCHEMA_VERSION: &str = "3";
pub const SEARCH_LIMIT: i64 = 3;
const SUGGESTION_LIMIT: i64 = 3;
const SUGGESTION_MIN_SCORE: f64 = 0.5;
const DISCOVERY_STRATEGY_VERSION: &str = "1";
const HEAD_SHA_KEY: &str = "head_sha";
const WIKI_DIR_KEY: &str = "wiki_dir";
const DISCOVERY_STRATEGY_VERSION_KEY: &str = "discovery_strategy_version";

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

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PageListEntry {
    pub title: String,
    pub aliases: Vec<String>,
    pub tags: Vec<String>,
    pub summary: String,
    pub file: String,
}

#[derive(Debug, Clone)]
struct ExistingDocument {
    path_rel: String,
    content_hash: String,
    mtime_ns: i64,
    size_bytes: i64,
}

#[derive(Debug, Clone)]
struct PendingDocument {
    path_abs: String,
    path_rel: String,
    title: String,
    title_key: String,
    summary: String,
    content: String,
    body: String,
    aliases: Vec<String>,
    tags: Vec<String>,
    keywords: Vec<String>,
    aliases_text: String,
    tags_text: String,
    keywords_text: String,
    incoming_links: Vec<PendingIncomingLink>,
    mtime_ns: i64,
    size_bytes: i64,
    content_hash: String,
}

#[derive(Debug, Clone)]
struct PendingIncomingLink {
    target_kind: PendingIncomingLinkKind,
    target_key: String,
    target_text: String,
    display_text: Option<String>,
    source_line: usize,
}

#[derive(Debug, Clone, Copy)]
enum PendingIncomingLinkKind {
    Page,
    File,
}

#[derive(Debug, Clone)]
struct SearchRow {
    title: String,
    path_abs: String,
    summary: String,
    source_raw: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DiscoveryState {
    head_sha: String,
    wiki_dir: String,
    strategy_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ChangeSetMode {
    Noop,
    Incremental,
    FullRescan,
}

#[derive(Debug, Clone)]
struct ChangeSet {
    mode: ChangeSetMode,
    changed_or_added: Vec<PathBuf>,
    deleted: HashSet<String>,
}

pub struct WikiIndex {
    runtime: Runtime,
    conn: Connection,
    repo_root: PathBuf,
}

impl WikiIndex {
    pub fn prepare(repo_root: &Path) -> Result<Self> {
        perf::scope_result("index.prepare", json!({}), || {
            let runtime = RuntimeBuilder::new_current_thread()
                .build()
                .into_diagnostic()
                .wrap_err("failed to create runtime for wiki index")?;

            let repo_root = repo_root.to_path_buf();
            let conn = runtime.block_on(open_and_prepare_connection(&repo_root))?;

            Ok(Self {
                runtime,
                conn,
                repo_root,
            })
        })
    }

    pub fn resolve_page(&self, input: &str) -> Result<Option<ResolvedPage>> {
        self.runtime
            .block_on(resolve_page_async(&self.conn, &self.repo_root, input))
    }

    pub fn search(&self, query: &str) -> Result<Vec<SearchResult>> {
        self.search_weighted(query, SEARCH_LIMIT, 0)
            .map(|(results, _)| results)
    }

    pub fn search_weighted(
        &self,
        query: &str,
        limit: i64,
        offset: usize,
    ) -> Result<(Vec<SearchResult>, usize)> {
        self.runtime
            .block_on(search_weighted_async(&self.conn, query, limit, offset))
    }

    pub fn suggest(&self, query: &str) -> Result<Vec<SearchResult>> {
        self.runtime.block_on(search_async(
            &self.conn,
            query,
            Some(SUGGESTION_LIMIT),
            SUGGESTION_MIN_SCORE,
        ))
    }

    pub fn list_pages(&self, tag: Option<&str>) -> Result<Vec<PageListEntry>> {
        self.runtime.block_on(list_pages_async(&self.conn, tag))
    }

    pub fn links(&self, input: &str) -> Result<Vec<SearchResult>> {
        self.runtime
            .block_on(links_async(&self.conn, &self.repo_root, input))
    }

    pub fn extract_pages(&self, titles: &[String]) -> Result<(Vec<ResolvedPage>, Vec<String>)> {
        self.runtime
            .block_on(extract_pages_async(&self.conn, titles))
    }

    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }

    pub fn refresh_paths(&mut self, changed_paths: &[PathBuf]) -> Result<()> {
        self.runtime.block_on(sync_core_index_for_paths(
            &mut self.conn,
            &self.repo_root,
            changed_paths,
        ))
    }

    pub fn fetch_all_keywords(&self) -> Result<Vec<(String, i64)>> {
        self.runtime.block_on(fetch_all_keywords_async(&self.conn))
    }

    pub fn fetch_pages_by_ids(&self, ids: &[i64]) -> Result<Vec<SearchResult>> {
        self.runtime
            .block_on(fetch_pages_by_ids_async(&self.conn, &self.repo_root, ids))
    }
}

fn wiki_dir(repo_root: &Path) -> Result<PathBuf> {
    let wiki_dir_name = std::env::var("WIKI_DIR").unwrap_or_else(|_| "wiki".to_string());
    let wiki_dir = repo_root.join(&wiki_dir_name);
    if !wiki_dir.exists() {
        return Err(miette!(
            "wiki directory does not exist: {}",
            wiki_dir.display()
        ));
    }
    Ok(wiki_dir)
}

async fn bootstrap_schema(conn: &Connection) -> Result<()> {
    // PRAGMA journal_mode returns a result row so it cannot go in execute_batch.
    conn.query("PRAGMA journal_mode=WAL", ())
        .await
        .into_diagnostic()
        .wrap_err("failed to set WAL journal mode")?;

    conn.execute_batch(
        "
        PRAGMA foreign_keys = ON;
        CREATE TABLE IF NOT EXISTS index_state (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        ",
    )
    .await
    .into_diagnostic()
    .wrap_err("failed to bootstrap index state table")?;

    let current_version = get_state(conn, "schema_version").await?;
    if current_version.as_deref() != Some(SCHEMA_VERSION) {
        recreate_schema(conn).await?;
    }

    Ok(())
}

/// Open the database, bootstrap the schema, verify integrity, and sync.
///
/// Returns true when `err` is a transient database-lock or busy error.
///
/// Used to distinguish concurrent-access failures (which must not trigger a
/// database delete-and-recreate) from genuine schema or corruption errors.
pub fn is_lock_error(err: &miette::Error) -> bool {
    let s = format!("{err:?}").to_lowercase();
    s.contains("locked") || s.contains("busy")
}

/// If bootstrap or integrity checks fail the database file is likely corrupt
/// or was created by an incompatible binary version. In that case the file is
/// deleted and the whole sequence is retried once from a clean state.
///
/// Lock errors are propagated without deleting the database — the file is
/// valid; it is just held by a concurrent writer.
async fn open_and_prepare_connection(repo_root: &Path) -> Result<Connection> {
    match try_open_and_prepare(repo_root).await {
        Ok(conn) => Ok(conn),
        Err(err) if is_lock_error(&err) => Err(err),
        Err(_) => {
            // Delete the stale or incompatible database and retry from scratch.
            let wiki_dir = wiki_dir(repo_root)?;
            let db_path = wiki_dir.join(".index.db");
            let _ = std::fs::remove_file(&db_path);
            try_open_and_prepare(repo_root).await
        }
    }
}

async fn try_open_and_prepare(repo_root: &Path) -> Result<Connection> {
    let mut conn = open_index_connection(repo_root).await?;
    perf::scope_async_result("index.bootstrap_schema", json!({}), bootstrap_schema(&conn)).await?;
    perf::scope_async_result("index.verify_integrity", json!({}), verify_integrity(&conn)).await?;
    perf::scope_async_result(
        "index.sync",
        json!({}),
        sync_core_index(&mut conn, repo_root),
    )
    .await?;
    Ok(conn)
}

async fn open_index_connection(repo_root: &Path) -> Result<Connection> {
    let wiki_dir = wiki_dir(repo_root)?;
    let db_path = wiki_dir.join(".index.db");
    let db = perf::scope_async_result(
        "index.open_database",
        json!({
            "db_path": db_path.display().to_string(),
        }),
        async {
            let db_path_str = db_path.to_string_lossy();
            Builder::new_local(&db_path_str)
                .experimental_index_method(true)
                .build()
                .await
                .into_diagnostic()
                .wrap_err_with(|| format!("failed to open index database at {}", db_path.display()))
        },
    )
    .await?;
    let conn = db
        .connect()
        .into_diagnostic()
        .wrap_err("failed to connect to index database")?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .into_diagnostic()
        .wrap_err("failed to configure index database busy timeout")?;
    Ok(conn)
}

async fn recreate_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        DROP INDEX IF EXISTS idx_documents_fts;
        DROP TABLE IF EXISTS keywords;
        DROP TABLE IF EXISTS incoming_links;
        DROP TABLE IF EXISTS tags;
        DROP TABLE IF EXISTS lookup_keys;
        DROP TABLE IF EXISTS documents;
        DELETE FROM index_state;

        CREATE TABLE IF NOT EXISTS documents (
            id INTEGER PRIMARY KEY,
            path_abs TEXT NOT NULL,
            path_rel TEXT NOT NULL UNIQUE,
            title TEXT NOT NULL,
            title_key TEXT NOT NULL,
            summary TEXT NOT NULL,
            body TEXT NOT NULL,
            source_raw TEXT NOT NULL,
            aliases_text TEXT NOT NULL,
            tags_text TEXT NOT NULL,
            keywords_text TEXT NOT NULL,
            mtime_ns INTEGER NOT NULL,
            size_bytes INTEGER NOT NULL,
            content_hash TEXT NOT NULL,
            indexed_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS lookup_keys (
            document_id INTEGER NOT NULL,
            key TEXT NOT NULL UNIQUE,
            raw_text TEXT NOT NULL,
            kind TEXT NOT NULL,
            FOREIGN KEY (document_id) REFERENCES documents(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS tags (
            document_id INTEGER NOT NULL,
            tag TEXT NOT NULL,
            tag_key TEXT NOT NULL,
            FOREIGN KEY (document_id) REFERENCES documents(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS incoming_links (
            document_id INTEGER NOT NULL,
            target_kind TEXT NOT NULL,
            target_key TEXT NOT NULL,
            target_text TEXT NOT NULL,
            display_text TEXT,
            source_line INTEGER NOT NULL,
            FOREIGN KEY (document_id) REFERENCES documents(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS keywords (
            document_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
            keyword     TEXT    NOT NULL,
            keyword_key TEXT    NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_keywords_document_id ON keywords(document_id);
        CREATE INDEX IF NOT EXISTS idx_keywords_keyword_key ON keywords(keyword_key);

        CREATE INDEX IF NOT EXISTS idx_lookup_keys_document_id ON lookup_keys(document_id);
        CREATE INDEX IF NOT EXISTS idx_tags_document_id ON tags(document_id);
        CREATE INDEX IF NOT EXISTS idx_tags_tag_key ON tags(tag_key);
        CREATE INDEX IF NOT EXISTS idx_incoming_links_target ON incoming_links(target_kind, target_key);
        CREATE INDEX IF NOT EXISTS idx_documents_fts ON documents USING fts (title, aliases_text, tags_text, keywords_text, summary, body)
        WITH (weights = 'title=5.0,aliases_text=4.0,tags_text=3.0,keywords_text=3.0,summary=2.0,body=1.0');
        ",
    )
    .await
    .into_diagnostic()
    .wrap_err("failed to create wiki index schema")?;

    set_state(conn, "schema_version", SCHEMA_VERSION).await?;
    Ok(())
}

async fn verify_integrity(conn: &Connection) -> Result<()> {
    let mut rows = conn
        .query("PRAGMA integrity_check", ())
        .await
        .into_diagnostic()
        .wrap_err("failed to run integrity check for wiki index")?;
    let mut errors = Vec::new();
    while let Some(row) = next_row(&mut rows).await? {
        let result = row
            .get::<String>(0usize)
            .into_diagnostic()
            .wrap_err("failed to decode integrity check result")?;
        if result == "ok" {
            return Ok(());
        }
        // Turso beta: FTS internal directory index reports false-positive integrity
        // errors across sessions. Skip known turso-internal FTS state entries.
        if result.contains("__turso_internal_fts") {
            continue;
        }
        errors.push(result);
    }
    if errors.is_empty() {
        return Ok(());
    }
    Err(miette!(
        "wiki index integrity check failed: {}",
        errors.join("; ")
    ))
}

async fn sync_core_index(conn: &mut Connection, repo_root: &Path) -> Result<()> {
    sync_core_index_inner(conn, repo_root, None).await
}

async fn sync_core_index_for_paths(
    conn: &mut Connection,
    repo_root: &Path,
    changed_paths: &[PathBuf],
) -> Result<()> {
    sync_core_index_inner(conn, repo_root, Some(changed_paths)).await
}

async fn sync_core_index_inner(
    conn: &mut Connection,
    repo_root: &Path,
    changed_paths: Option<&[PathBuf]>,
) -> Result<()> {
    let existing = perf::scope_async_result(
        "index.load_existing_documents",
        json!({}),
        load_existing_documents(conn),
    )
    .await?;
    let existing_by_path: HashMap<String, ExistingDocument> = existing
        .into_iter()
        .map(|doc| (doc.path_rel.clone(), doc))
        .collect();
    let change_set = perf::scope_async_result(
        "index.compute_change_set",
        json!({}),
        compute_change_set(conn, repo_root, &existing_by_path, changed_paths),
    )
    .await?;

    let mut changed_or_new = HashSet::new();
    let mut pending_documents = Vec::new();
    let now_ns = unix_time_now_ns()?;

    for path in &change_set.changed_or_added {
        let path_rel = relative_path(repo_root, path)?;

        let metadata = fs::metadata(path)
            .into_diagnostic()
            .wrap_err_with(|| format!("failed to stat {}", path.display()))?;
        let mtime_ns = metadata_modified_ns(&metadata)
            .wrap_err_with(|| format!("failed to read mtime for {}", path.display()))?;
        let size_bytes = i64::try_from(metadata.len()).into_diagnostic()?;
        let existing_doc = existing_by_path.get(&path_rel);

        let must_read = match existing_doc {
            Some(doc) => doc.mtime_ns != mtime_ns || doc.size_bytes != size_bytes,
            None => true,
        };

        if !must_read {
            continue;
        }

        let content = fs::read_to_string(path)
            .into_diagnostic()
            .wrap_err_with(|| format!("failed to read {}", path.display()))?;
        let content_hash = sha256_hex(&content);

        if let Some(doc) = existing_doc
            && doc.content_hash == content_hash
        {
            changed_or_new.insert(path_rel.clone());
            pending_documents.push(PendingDocument {
                path_abs: canonical_display_path(path),
                path_rel,
                title: String::new(),
                title_key: String::new(),
                summary: String::new(),
                content,
                body: String::new(),
                aliases: Vec::new(),
                tags: Vec::new(),
                keywords: Vec::new(),
                aliases_text: String::new(),
                tags_text: String::new(),
                keywords_text: String::new(),
                incoming_links: Vec::new(),
                mtime_ns,
                size_bytes,
                content_hash,
            });
            continue;
        }

        let frontmatter = parse_frontmatter(&content, path)
            .map_err(|error| miette!("frontmatter error in `{}`: {error}", path.display()))?
            .ok_or_else(|| {
                miette!(
                    "No frontmatter in `{}` — add a `---` block with `title` and `summary`.",
                    path.display()
                )
            })?;
        let body = markdown_body(&content);
        let mut incoming_links = parse_wikilinks(&content)
            .into_iter()
            .map(|link| PendingIncomingLink {
                target_kind: PendingIncomingLinkKind::Page,
                target_text: link.title.clone(),
                target_key: link.title.to_lowercase(),
                display_text: link.display,
                source_line: link.source_line,
            })
            .collect::<Vec<_>>();
        incoming_links.extend(
            parse_fragment_links(&content)
                .into_iter()
                .filter(|link| link.kind != LinkKind::External)
                .map(|link| {
                    let resolved_path = resolve_link_path(&link.path, path, repo_root)
                        .to_string_lossy()
                        .into_owned();
                    PendingIncomingLink {
                        target_kind: PendingIncomingLinkKind::File,
                        target_text: resolved_path.clone(),
                        target_key: resolved_path,
                        display_text: None,
                        source_line: link.source_line,
                    }
                }),
        );

        changed_or_new.insert(path_rel.clone());
        pending_documents.push(PendingDocument {
            path_abs: canonical_display_path(path),
            path_rel,
            title_key: frontmatter.title.to_lowercase(),
            aliases_text: frontmatter.aliases.join(" "),
            tags_text: frontmatter.tags.join(" "),
            keywords_text: frontmatter.keywords.join(" "),
            title: frontmatter.title,
            summary: frontmatter.summary,
            aliases: frontmatter.aliases,
            tags: frontmatter.tags,
            keywords: frontmatter.keywords,
            body,
            content,
            incoming_links,
            mtime_ns,
            size_bytes,
            content_hash,
        });
    }

    let stale_paths = change_set.deleted;
    let has_changes = !pending_documents.is_empty() || !stale_paths.is_empty();

    perf::scope_async_result(
        "index.validate_lookup_collisions",
        json!({
            "pending_documents": pending_documents.len(),
            "changed_or_new": changed_or_new.len(),
            "stale_paths": stale_paths.len(),
        }),
        validate_lookup_collisions(conn, &pending_documents, &changed_or_new, &stale_paths),
    )
    .await?;

    perf::log_event(
        "index.sync_plan",
        0.0,
        "ok",
        json!({
            "mode": format!("{:?}", change_set.mode),
            "pending_documents": pending_documents.len(),
            "changed_or_new": changed_or_new.len(),
            "stale_paths": stale_paths.len(),
        }),
    );

    let tx = perf::scope_async_result("index.begin_transaction", json!({}), async {
        conn.transaction()
            .await
            .into_diagnostic()
            .wrap_err("failed to start wiki index transaction")
    })
    .await?;

    for stale_path in &stale_paths {
        tx.execute(
            "DELETE FROM documents WHERE path_rel = ?1",
            params![stale_path.clone()],
        )
        .await
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to remove stale index entry for {stale_path}"))?;
    }

    for pending in pending_documents {
        if pending.title.is_empty() {
            tx.execute(
                "
                UPDATE documents
                SET path_abs = ?1, source_raw = ?2, mtime_ns = ?3, size_bytes = ?4, content_hash = ?5, indexed_at = ?6
                WHERE path_rel = ?7
                ",
                params![
                    pending.path_abs,
                    pending.content,
                    pending.mtime_ns,
                    pending.size_bytes,
                    pending.content_hash,
                    now_ns,
                    pending.path_rel,
                ],
            )
            .await
            .into_diagnostic()
            .wrap_err("failed to refresh unchanged wiki index metadata")?;
            continue;
        }

        tx.execute(
            "DELETE FROM documents WHERE path_rel = ?1",
            params![pending.path_rel.clone()],
        )
        .await
        .into_diagnostic()
        .wrap_err_with(|| {
            format!(
                "failed to clear existing index row for {}",
                pending.path_rel
            )
        })?;

        let title = pending.title.clone();
        let title_key = pending.title_key.clone();
        let summary = pending.summary.clone();
        let body = pending.body;
        let aliases_text = pending.aliases_text;
        let tags_text = pending.tags_text;
        let keywords_text = pending.keywords_text;

        tx.execute(
            "
            INSERT INTO documents (
                path_abs, path_rel, title, title_key, summary, body, source_raw, aliases_text, tags_text, keywords_text,
                mtime_ns, size_bytes, content_hash, indexed_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
            ",
            params![
                pending.path_abs,
                pending.path_rel,
                title.clone(),
                title_key.clone(),
                summary.clone(),
                body.clone(),
                pending.content,
                aliases_text.clone(),
                tags_text.clone(),
                keywords_text.clone(),
                pending.mtime_ns,
                pending.size_bytes,
                pending.content_hash,
                now_ns,
            ],
        )
        .await
        .into_diagnostic()
        .wrap_err("failed to insert wiki document into index")?;

        let document_id = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO lookup_keys (document_id, key, raw_text, kind) VALUES (?1, ?2, ?3, 'title')",
            params![document_id, title_key, title],
        )
        .await
        .into_diagnostic()
        .wrap_err("failed to insert title lookup key")?;

        for alias in pending.aliases {
            let alias_key = alias.to_lowercase();
            tx.execute(
                "INSERT INTO lookup_keys (document_id, key, raw_text, kind) VALUES (?1, ?2, ?3, 'alias')",
                params![document_id, alias_key.clone(), alias.clone()],
            )
            .await
            .into_diagnostic()
            .wrap_err("failed to insert alias lookup key")?;
        }

        for tag in pending.tags {
            tx.execute(
                "INSERT INTO tags (document_id, tag, tag_key) VALUES (?1, ?2, ?3)",
                params![document_id, tag.clone(), tag.to_lowercase()],
            )
            .await
            .into_diagnostic()
            .wrap_err("failed to insert tag row")?;
        }

        for keyword in pending.keywords {
            tx.execute(
                "INSERT INTO keywords (document_id, keyword, keyword_key) VALUES (?1, ?2, ?3)",
                params![document_id, keyword.clone(), keyword.to_lowercase()],
            )
            .await
            .into_diagnostic()
            .wrap_err("failed to insert keyword row")?;
        }

        for incoming_link in pending.incoming_links {
            tx.execute(
                "
                INSERT INTO incoming_links (document_id, target_kind, target_key, target_text, display_text, source_line)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                ",
                params![
                    document_id,
                    match incoming_link.target_kind {
                        PendingIncomingLinkKind::Page => "page",
                        PendingIncomingLinkKind::File => "file",
                    },
                    incoming_link.target_key,
                    incoming_link.target_text,
                    incoming_link.display_text,
                    i64::try_from(incoming_link.source_line).into_diagnostic()?,
                ],
            )
            .await
            .into_diagnostic()
            .wrap_err("failed to insert incoming link row")?;
        }
    }

    perf::scope_async_result("index.commit_transaction", json!({}), async {
        tx.commit()
            .await
            .into_diagnostic()
            .wrap_err("failed to commit wiki index transaction")
    })
    .await?;

    if has_changes {
        conn.execute("OPTIMIZE INDEX idx_documents_fts", ())
            .await
            .into_diagnostic()
            .wrap_err("failed to optimize FTS index")?;
    }
    write_discovery_state(conn, &current_discovery_state(repo_root)?).await?;

    Ok(())
}

fn discover_index_files(repo_root: &Path) -> Result<Vec<PathBuf>> {
    match crate::commands::discover_files(&[], repo_root) {
        Ok(files) => Ok(files),
        Err(e) => {
            if e.to_string().contains("no wiki pages found") {
                Ok(Vec::new())
            } else {
                Err(e)
            }
        }
    }
}

async fn compute_change_set(
    conn: &Connection,
    repo_root: &Path,
    existing_by_path: &HashMap<String, ExistingDocument>,
    changed_paths: Option<&[PathBuf]>,
) -> Result<ChangeSet> {
    if let Some(changed_paths) = changed_paths {
        return hinted_change_set(repo_root, existing_by_path, changed_paths);
    }

    let current_state = current_discovery_state(repo_root)?;
    let Some(previous_state) = read_discovery_state(conn).await? else {
        return full_rescan_change_set(repo_root, existing_by_path);
    };

    let tracked_files_present = has_tracked_files(repo_root)?;
    let acceleration_state = git_acceleration_state(repo_root).unwrap_or_default();
    perf::log_event(
        "index.git_probe_state",
        0.0,
        "ok",
        json!({
            "has_tracked_files": tracked_files_present,
            "untracked_cache": acceleration_state.untracked_cache,
            "split_index": acceleration_state.split_index,
        }),
    );

    if previous_state != current_state {
        return full_rescan_change_set(repo_root, existing_by_path);
    }

    let mut candidate_paths = HashSet::new();

    if previous_state.head_sha.is_empty() != current_state.head_sha.is_empty() {
        return full_rescan_change_set(repo_root, existing_by_path);
    }

    if !previous_state.head_sha.is_empty() && previous_state.head_sha != current_state.head_sha {
        for path in
            changed_paths_between(repo_root, &previous_state.head_sha, &current_state.head_sha)?
        {
            if matches_default_discovery_path(&path, &current_state.wiki_dir) {
                candidate_paths.insert(path);
            }
        }
    }

    if current_state.head_sha.is_empty() || !tracked_files_present {
        let current_inventory = repo_inventory(repo_root)?
            .into_iter()
            .filter(|path| matches_default_discovery_path(path, &current_state.wiki_dir))
            .collect::<HashSet<_>>();

        for path in &current_inventory {
            candidate_paths.insert(path.clone());
        }

        for path in existing_by_path.keys() {
            if matches_default_discovery_path(path, &current_state.wiki_dir)
                && !current_inventory.contains(path)
            {
                candidate_paths.insert(path.clone());
            }
        }
    }

    let has_dirty_tracked_tree = if current_state.head_sha.is_empty() || !tracked_files_present {
        false
    } else {
        has_staged_changes(repo_root)? || has_unstaged_changes(repo_root)?
    };

    if has_dirty_tracked_tree {
        for path in working_tree_changed_paths(repo_root)? {
            if matches_default_discovery_path(&path, &current_state.wiki_dir) {
                candidate_paths.insert(path);
            }
        }
    } else {
        for path in untracked_paths(repo_root)? {
            if matches_default_discovery_path(&path, &current_state.wiki_dir) {
                candidate_paths.insert(path);
            }
        }
    }

    if candidate_paths.is_empty() {
        return Ok(ChangeSet {
            mode: ChangeSetMode::Noop,
            changed_or_added: Vec::new(),
            deleted: HashSet::new(),
        });
    }

    let mut changed_or_added = Vec::new();
    let mut deleted = HashSet::new();

    for path_rel in candidate_paths {
        let path = repo_root.join(&path_rel);
        if path.is_file() {
            changed_or_added.push(path);
        } else if existing_by_path.contains_key(&path_rel) {
            deleted.insert(path_rel);
        }
    }

    Ok(ChangeSet {
        mode: ChangeSetMode::Incremental,
        changed_or_added,
        deleted,
    })
}

fn hinted_change_set(
    repo_root: &Path,
    existing_by_path: &HashMap<String, ExistingDocument>,
    changed_paths: &[PathBuf],
) -> Result<ChangeSet> {
    let mut candidate_paths = HashSet::new();
    for path in changed_paths {
        if let Ok(path_rel) = relative_path(repo_root, path) {
            candidate_paths.insert(path_rel);
        }
    }

    let mut changed_or_added = Vec::new();
    let mut deleted = HashSet::new();
    for path_rel in candidate_paths {
        let path = repo_root.join(&path_rel);
        if path.is_file() {
            changed_or_added.push(path);
        } else if existing_by_path.contains_key(&path_rel) {
            deleted.insert(path_rel);
        }
    }

    Ok(ChangeSet {
        mode: ChangeSetMode::Incremental,
        changed_or_added,
        deleted,
    })
}

fn full_rescan_change_set(
    repo_root: &Path,
    existing_by_path: &HashMap<String, ExistingDocument>,
) -> Result<ChangeSet> {
    let files = perf::scope_result("index.discover_files", json!({}), || {
        discover_index_files(repo_root)
    })?;
    let mut seen_paths = HashSet::new();
    for path in &files {
        seen_paths.insert(relative_path(repo_root, path)?);
    }
    let deleted = existing_by_path
        .keys()
        .filter(|path_rel| !seen_paths.contains(*path_rel))
        .cloned()
        .collect::<HashSet<_>>();
    Ok(ChangeSet {
        mode: ChangeSetMode::FullRescan,
        changed_or_added: files,
        deleted,
    })
}

fn current_discovery_state(repo_root: &Path) -> Result<DiscoveryState> {
    Ok(DiscoveryState {
        head_sha: head_sha(repo_root).unwrap_or_default(),
        wiki_dir: std::env::var("WIKI_DIR").unwrap_or_else(|_| "wiki".to_string()),
        strategy_version: DISCOVERY_STRATEGY_VERSION.to_string(),
    })
}

async fn read_discovery_state(conn: &Connection) -> Result<Option<DiscoveryState>> {
    let Some(head_sha) = get_state(conn, HEAD_SHA_KEY).await? else {
        return Ok(None);
    };
    let Some(wiki_dir) = get_state(conn, WIKI_DIR_KEY).await? else {
        return Ok(None);
    };
    let Some(strategy_version) = get_state(conn, DISCOVERY_STRATEGY_VERSION_KEY).await? else {
        return Ok(None);
    };

    Ok(Some(DiscoveryState {
        head_sha,
        wiki_dir,
        strategy_version,
    }))
}

async fn write_discovery_state(conn: &Connection, state: &DiscoveryState) -> Result<()> {
    set_state(conn, HEAD_SHA_KEY, &state.head_sha).await?;
    set_state(conn, WIKI_DIR_KEY, &state.wiki_dir).await?;
    set_state(
        conn,
        DISCOVERY_STRATEGY_VERSION_KEY,
        &state.strategy_version,
    )
    .await?;
    Ok(())
}

fn matches_default_discovery_path(path_rel: &str, wiki_dir_name: &str) -> bool {
    if !path_rel.ends_with(".md") {
        return false;
    }

    if path_rel.ends_with(".wiki.md") {
        return true;
    }

    let path = Path::new(path_rel);
    let wiki_dir = Path::new(wiki_dir_name);
    path.starts_with(wiki_dir)
}

async fn validate_lookup_collisions(
    conn: &Connection,
    pending_documents: &[PendingDocument],
    changed_or_new: &HashSet<String>,
    stale_paths: &HashSet<String>,
) -> Result<()> {
    let mut existing = HashMap::<String, String>::new();
    let mut rows = conn
        .query(
            "
            SELECT lk.key, d.path_rel
            FROM lookup_keys lk
            JOIN documents d ON d.id = lk.document_id
            ",
            (),
        )
        .await
        .into_diagnostic()
        .wrap_err("failed to read existing lookup keys from wiki index")?;

    while let Some(row) = next_row(&mut rows).await? {
        let key = row_string(&row, 0)?;
        let path_rel = row_string(&row, 1)?;
        if changed_or_new.contains(&path_rel) || stale_paths.contains(&path_rel) {
            continue;
        }
        existing.insert(key, path_rel);
    }

    for pending in pending_documents {
        if pending.title.is_empty() {
            continue;
        }

        let mut keys = Vec::new();
        keys.push((pending.title_key.clone(), pending.title.clone()));
        keys.extend(
            pending
                .aliases
                .iter()
                .map(|alias| (alias.to_lowercase(), alias.clone())),
        );

        for (key, raw_text) in keys {
            if let Some(existing_path) = existing.get(&key)
                && existing_path != &pending.path_rel
            {
                return Err(miette!(
                    "title or alias collision for `{raw_text}` between `{}` and `{existing_path}`",
                    pending.path_rel
                ));
            }

            match existing.get(&key) {
                Some(existing_path) if existing_path == &pending.path_rel => {}
                _ => {
                    existing.insert(key, pending.path_rel.clone());
                }
            }
        }
    }

    Ok(())
}

async fn load_existing_documents(conn: &Connection) -> Result<Vec<ExistingDocument>> {
    let mut rows = conn
        .query(
            "SELECT path_rel, content_hash, mtime_ns, size_bytes FROM documents",
            (),
        )
        .await
        .into_diagnostic()
        .wrap_err("failed to read existing wiki index documents")?;
    let mut documents = Vec::new();
    while let Some(row) = next_row(&mut rows).await? {
        documents.push(ExistingDocument {
            path_rel: row_string(&row, 0)?,
            content_hash: row_string(&row, 1)?,
            mtime_ns: row_i64(&row, 2)?,
            size_bytes: row_i64(&row, 3)?,
        });
    }
    Ok(documents)
}

async fn resolve_page_async(
    conn: &Connection,
    repo_root: &Path,
    input: &str,
) -> Result<Option<ResolvedPage>> {
    perf::scope_async_result(
        "index.resolve_page",
        json!({
            "input": input,
            "input_kind": if looks_like_path(input) { "path" } else { "lookup" },
        }),
        async {
            if looks_like_path(input) {
                for candidate in path_candidates(repo_root, input)? {
                    if let Some(page) = fetch_page_by_path(conn, &candidate).await? {
                        return Ok(Some(page));
                    }
                }
                return Ok(None);
            }

            fetch_page_by_lookup(conn, input).await
        },
    )
    .await
}

async fn fetch_page_by_lookup(conn: &Connection, input: &str) -> Result<Option<ResolvedPage>> {
    let mut rows = conn
        .query(
            "
            SELECT d.id, d.title, d.path_abs, d.summary, d.source_raw, lk.raw_text, lk.kind
            FROM lookup_keys lk
            JOIN documents d ON d.id = lk.document_id
            WHERE lk.key = ?1
            LIMIT 1
            ",
            params![input.to_lowercase()],
        )
        .await
        .into_diagnostic()
        .wrap_err("failed to resolve wiki page from lookup index")?;
    let Some(row) = next_row(&mut rows).await? else {
        return Ok(None);
    };
    let kind = row_string(&row, 6)?;
    let alias = if kind == "alias" {
        Some(row_string(&row, 5)?)
    } else {
        None
    };
    Ok(Some(ResolvedPage {
        document_id: row_i64(&row, 0)?,
        title: row_string(&row, 1)?,
        file: row_string(&row, 2)?,
        summary: row_string(&row, 3)?,
        content: row_string(&row, 4)?,
        alias,
    }))
}

async fn fetch_page_by_path(
    conn: &Connection,
    candidate: &PathCandidate,
) -> Result<Option<ResolvedPage>> {
    let mut queries = Vec::new();
    queries.push(("SELECT d.id, d.title, d.path_abs, d.summary, d.source_raw FROM documents d WHERE d.path_abs = ?1 LIMIT 1", candidate.path_abs.clone()));
    if let Some(path_rel) = &candidate.path_rel {
        queries.push(("SELECT d.id, d.title, d.path_abs, d.summary, d.source_raw FROM documents d WHERE d.path_rel = ?1 LIMIT 1", path_rel.clone()));
    }

    for (sql, value) in queries {
        let mut rows = conn
            .query(sql, params![value])
            .await
            .into_diagnostic()
            .wrap_err("failed to resolve wiki page by path")?;
        if let Some(row) = next_row(&mut rows).await? {
            return Ok(Some(ResolvedPage {
                document_id: row_i64(&row, 0)?,
                title: row_string(&row, 1)?,
                file: row_string(&row, 2)?,
                summary: row_string(&row, 3)?,
                content: row_string(&row, 4)?,
                alias: None,
            }));
        }
    }

    Ok(None)
}

async fn search_weighted_async(
    conn: &Connection,
    query: &str,
    limit: i64,
    offset: usize,
) -> Result<(Vec<SearchResult>, usize)> {
    perf::scope_async_result(
        "index.search_weighted",
        json!({
            "query": query,
            "limit": limit,
            "offset": offset,
        }),
        async {
            let limit = limit.max(0) as usize;
            if limit == 0 {
                return Ok((Vec::new(), 0));
            }

            let tokens = search_tokens(query);
            let mut all_results = Vec::new();
            let mut seen_titles = HashMap::new();

            for result in search_exact_matches_async(conn, query, &tokens).await? {
                push_weighted_result(&mut all_results, &mut seen_titles, result);
            }

            for result in search_path_matches_async(conn, query, &tokens).await? {
                push_weighted_result(&mut all_results, &mut seen_titles, result);
            }

            for result in search_async(conn, query, None, 0.0).await? {
                push_weighted_result(&mut all_results, &mut seen_titles, result);
            }

            let total = all_results.len();
            let results: Vec<SearchResult> =
                all_results.into_iter().skip(offset).take(limit).collect();

            perf::log_event(
                "index.search_weighted_result",
                0.0,
                "ok",
                json!({
                    "query": query,
                    "limit": limit,
                    "offset": offset,
                    "total": total,
                    "result_count": results.len(),
                }),
            );

            Ok((results, total))
        },
    )
    .await
}

async fn search_exact_matches_async(
    conn: &Connection,
    query: &str,
    tokens: &[String],
) -> Result<Vec<SearchResult>> {
    let normalized = query.trim().to_lowercase();
    if normalized.is_empty() {
        return Ok(Vec::new());
    }

    let mut rows = conn
        .query(
            "
            SELECT d.title, d.path_abs, d.summary, d.source_raw, lk.kind
            FROM lookup_keys lk
            JOIN documents d ON d.id = lk.document_id
            WHERE lk.key = ?1
            ORDER BY CASE lk.kind WHEN 'title' THEN 0 ELSE 1 END, d.title ASC
            ",
            params![normalized],
        )
        .await
        .into_diagnostic()
        .wrap_err("failed to execute exact wiki search query")?;

    let mut results = Vec::new();
    while let Some(row) = next_row(&mut rows).await? {
        results.push(SearchResult {
            title: row_string(&row, 0)?,
            file: row_string(&row, 1)?,
            summary: row_string(&row, 2)?,
            alias: None,
            snippets: matched_snippets(&row_string(&row, 3)?, tokens),
        });
    }
    Ok(results)
}

async fn search_path_matches_async(
    conn: &Connection,
    query: &str,
    tokens: &[String],
) -> Result<Vec<SearchResult>> {
    if tokens.is_empty() {
        return Ok(Vec::new());
    }

    let predicate = tokens
        .iter()
        .enumerate()
        .map(|(index, _)| {
            let placeholder = index + 1;
            format!("LOWER(d.path_rel || ' ' || d.path_abs) LIKE ?{placeholder} ESCAPE '\\'")
        })
        .collect::<Vec<_>>()
        .join(" AND ");
    let sql = format!(
        "
        SELECT d.title, d.path_abs, d.summary, d.source_raw
        FROM documents d
        WHERE {predicate}
        ORDER BY d.path_rel ASC, d.title ASC
        "
    );
    let patterns = tokens
        .iter()
        .map(|token| format!("%{}%", escape_like_pattern(token)))
        .collect::<Vec<_>>();

    let mut rows = conn
        .query(&sql, params_from_iter(patterns.iter().map(String::as_str)))
        .await
        .into_diagnostic()
        .wrap_err("failed to execute path wiki search query")?;

    let mut results = Vec::new();
    while let Some(row) = next_row(&mut rows).await? {
        results.push(SearchResult {
            title: row_string(&row, 0)?,
            file: row_string(&row, 1)?,
            summary: row_string(&row, 2)?,
            alias: None,
            snippets: matched_snippets(&row_string(&row, 3)?, tokens),
        });
    }

    perf::log_event(
        "index.search_path_result",
        0.0,
        "ok",
        json!({
            "query": query,
            "token_count": tokens.len(),
            "result_count": results.len(),
        }),
    );

    Ok(results)
}

async fn search_async(
    conn: &Connection,
    query: &str,
    limit: Option<i64>,
    min_score: f64,
) -> Result<Vec<SearchResult>> {
    let limit_clause = limit.map_or_else(String::new, |limit| format!(" LIMIT {limit}"));
    perf::scope_async_result(
        "index.search",
        json!({
            "query": query,
            "limit": limit,
            "min_score": min_score,
        }),
        async {
            let fts_query = build_fts_query(query);
            if fts_query.is_empty() {
                return Ok(Vec::new());
            }

            let sql = format!(
                "
                SELECT
                    d.title,
                    d.path_abs,
                    d.summary,
                    d.source_raw,
                    fts_score(d.title, d.aliases_text, d.tags_text, d.keywords_text, d.summary, d.body, ?1) AS score
                FROM documents d
                WHERE fts_match(d.title, d.aliases_text, d.tags_text, d.keywords_text, d.summary, d.body, ?1)
                ORDER BY score DESC, d.title ASC
                {limit_clause}
                "
            );

            let mut rows = conn
                .query(&sql, params![fts_query])
                .await
                .into_diagnostic()
                .wrap_err("failed to execute wiki search query")?;

            let tokens = search_tokens(query);
            let mut results = Vec::new();
            while let Some(row) = next_row(&mut rows).await? {
                let row = SearchRow {
                    title: row_string(&row, 0)?,
                    path_abs: row_string(&row, 1)?,
                    summary: row_string(&row, 2)?,
                    source_raw: row_string(&row, 3)?,
                };
                if token_match_score(&row.title, &row.summary, &row.source_raw, &tokens) < min_score
                {
                    continue;
                }
                results.push(SearchResult {
                    title: row.title,
                    file: row.path_abs,
                    summary: row.summary,
                    alias: None,
                    snippets: matched_snippets(&row.source_raw, &tokens),
                });
            }

            perf::log_event(
                "index.search_result",
                0.0,
                "ok",
                json!({
                    "query": query,
                    "token_count": tokens.len(),
                    "result_count": results.len(),
                }),
            );

            Ok(results)
        },
    )
    .await
}

fn push_weighted_result(
    results: &mut Vec<SearchResult>,
    seen_titles: &mut HashMap<String, usize>,
    result: SearchResult,
) {
    if let Some(&index) = seen_titles.get(&result.title) {
        if results[index].snippets.is_empty() && !result.snippets.is_empty() {
            results[index].snippets = result.snippets;
        }
        return;
    }

    let index = results.len();
    seen_titles.insert(result.title.clone(), index);
    results.push(result);
}

fn escape_like_pattern(token: &str) -> String {
    token
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

async fn list_pages_async(conn: &Connection, tag: Option<&str>) -> Result<Vec<PageListEntry>> {
    perf::scope_async_result(
        "index.list_pages",
        json!({
            "tag": tag,
        }),
        async {
            let mut rows = if let Some(tag) = tag {
                conn.query(
                    "
                    SELECT DISTINCT d.id, d.title, d.summary, d.path_abs
                    FROM documents d
                    JOIN tags t ON t.document_id = d.id
                    WHERE t.tag_key = ?1
                    ORDER BY d.title ASC
                    ",
                    params![tag.to_lowercase()],
                )
                .await
            } else {
                conn.query(
                    "SELECT d.id, d.title, d.summary, d.path_abs FROM documents d ORDER BY d.title ASC",
                    (),
                )
                .await
            }
            .into_diagnostic()
            .wrap_err("failed to list wiki pages from index")?;

            let mut pages = Vec::new();
            while let Some(row) = next_row(&mut rows).await? {
                let document_id = row_i64(&row, 0)?;
                pages.push(PageListEntry {
                    title: row_string(&row, 1)?,
                    summary: row_string(&row, 2)?,
                    file: row_string(&row, 3)?,
                    aliases: load_aliases(conn, document_id).await?,
                    tags: load_tags(conn, document_id).await?,
                });
            }

            perf::log_event(
                "index.list_pages_result",
                0.0,
                "ok",
                json!({
                    "tag": tag,
                    "count": pages.len(),
                }),
            );

            Ok(pages)
        },
    )
    .await
}

async fn links_async(
    conn: &Connection,
    repo_root: &Path,
    input: &str,
) -> Result<Vec<SearchResult>> {
    perf::scope_async_result(
        "index.links",
        json!({
            "input": input,
        }),
        async {
            let page_target_keys =
                if let Some(page) = resolve_page_async(conn, repo_root, input).await? {
                    load_lookup_keys(conn, page.document_id).await?
                } else {
                    Vec::new()
                };
            let file_target = looks_like_path(input)
                .then(|| normalize_repo_relative_path(input, repo_root))
                .filter(|path| !path.is_empty());

            if page_target_keys.is_empty() && file_target.is_none() {
                return Ok(Vec::new());
            }

            let mut predicates = Vec::new();
            let mut args = Vec::new();

            if !page_target_keys.is_empty() {
                let placeholders = page_target_keys
                    .iter()
                    .enumerate()
                    .map(|(index, _)| format!("?{}", index + 1))
                    .collect::<Vec<_>>()
                    .join(", ");
                predicates.push(format!(
                    "(il.target_kind = 'page' AND il.target_key IN ({placeholders}))"
                ));
                args.extend(page_target_keys.iter().cloned());
            }

            if let Some(path) = &file_target {
                predicates.push(format!(
                    "(il.target_kind = 'file' AND il.target_key = ?{})",
                    args.len() + 1
                ));
                args.push(path.clone());
            }

            let sql = format!(
                "
                SELECT d.title, d.path_abs, d.summary, d.source_raw, il.source_line
                FROM incoming_links il
                JOIN documents d ON d.id = il.document_id
                WHERE {}
                ORDER BY d.title ASC, il.source_line ASC
                ",
                predicates.join(" OR ")
            );

            let mut rows = conn
                .query(&sql, params_from_iter(args.iter().map(String::as_str)))
                .await
                .into_diagnostic()
                .wrap_err("failed to load incoming links from wiki index")?;

            let mut results = Vec::<SearchResult>::new();
            let mut seen_titles = HashMap::<String, usize>::new();
            while let Some(row) = next_row(&mut rows).await? {
                let title = row_string(&row, 0)?;
                let file = row_string(&row, 1)?;
                let summary = row_string(&row, 2)?;
                let source_raw = row_string(&row, 3)?;
                let source_line = usize::try_from(row_i64(&row, 4)?).into_diagnostic()?;
                let snippet = line_snippet(&source_raw, source_line);

                if let Some(&index) = seen_titles.get(&title) {
                    if let Some(snippet) = snippet
                        && !results[index]
                            .snippets
                            .iter()
                            .any(|existing| existing == &snippet)
                        && results[index].snippets.len() < 3
                    {
                        results[index].snippets.push(snippet);
                    }
                    continue;
                }

                let index = results.len();
                seen_titles.insert(title.clone(), index);
                results.push(SearchResult {
                    title,
                    file,
                    summary,
                    alias: None,
                    snippets: snippet.into_iter().collect(),
                });
            }

            perf::log_event(
                "index.links_result",
                0.0,
                "ok",
                json!({
                    "input": input,
                    "count": results.len(),
                    "page_target_count": page_target_keys.len(),
                    "has_file_target": file_target.is_some(),
                }),
            );

            Ok(results)
        },
    )
    .await
}

async fn extract_pages_async(
    conn: &Connection,
    titles: &[String],
) -> Result<(Vec<ResolvedPage>, Vec<String>)> {
    perf::scope_async_result(
        "index.extract_pages",
        json!({
            "title_count": titles.len(),
        }),
        async {
            let mut resolved = Vec::new();
            let mut unresolved = Vec::new();

            for title in titles {
                if let Some(page) = fetch_page_by_lookup(conn, title).await? {
                    resolved.push(page);
                } else {
                    unresolved.push(title.clone());
                }
            }

            perf::log_event(
                "index.extract_pages_result",
                0.0,
                "ok",
                json!({
                    "resolved_count": resolved.len(),
                    "unresolved_count": unresolved.len(),
                }),
            );

            Ok((resolved, unresolved))
        },
    )
    .await
}

async fn load_aliases(conn: &Connection, document_id: i64) -> Result<Vec<String>> {
    let mut rows = conn
        .query(
            "SELECT raw_text FROM lookup_keys WHERE document_id = ?1 AND kind = 'alias' ORDER BY key ASC",
            params![document_id],
        )
        .await
        .into_diagnostic()
        .wrap_err("failed to load wiki aliases from index")?;
    let mut aliases = Vec::new();
    while let Some(row) = next_row(&mut rows).await? {
        aliases.push(row_string(&row, 0)?);
    }
    Ok(aliases)
}

async fn load_lookup_keys(conn: &Connection, document_id: i64) -> Result<Vec<String>> {
    let mut rows = conn
        .query(
            "SELECT key FROM lookup_keys WHERE document_id = ?1 ORDER BY kind ASC, key ASC",
            params![document_id],
        )
        .await
        .into_diagnostic()
        .wrap_err("failed to load lookup keys from wiki index")?;

    let mut keys = Vec::new();
    while let Some(row) = next_row(&mut rows).await? {
        keys.push(row_string(&row, 0)?);
    }
    Ok(keys)
}

async fn load_tags(conn: &Connection, document_id: i64) -> Result<Vec<String>> {
    let mut rows = conn
        .query(
            "SELECT tag FROM tags WHERE document_id = ?1 ORDER BY tag_key ASC, tag ASC",
            params![document_id],
        )
        .await
        .into_diagnostic()
        .wrap_err("failed to load wiki tags from index")?;
    let mut tags = Vec::new();
    while let Some(row) = next_row(&mut rows).await? {
        tags.push(row_string(&row, 0)?);
    }
    Ok(tags)
}

async fn fetch_all_keywords_async(conn: &Connection) -> Result<Vec<(String, i64)>> {
    let mut rows = conn
        .query("SELECT keyword, document_id FROM keywords", ())
        .await
        .into_diagnostic()
        .wrap_err("failed to fetch keywords")?;
    let mut result = Vec::new();
    while let Some(row) = next_row(&mut rows).await? {
        let keyword: String = row.get(0usize).into_diagnostic()?;
        let doc_id: i64 = row.get(1usize).into_diagnostic()?;
        result.push((keyword, doc_id));
    }
    Ok(result)
}

async fn fetch_pages_by_ids_async(
    conn: &Connection,
    repo_root: &Path,
    ids: &[i64],
) -> Result<Vec<SearchResult>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut results = Vec::new();
    for &id in ids {
        let mut rows = conn
            .query(
                "SELECT title, path_abs, summary FROM documents WHERE id = ?1",
                params![id],
            )
            .await
            .into_diagnostic()
            .wrap_err("failed to fetch page by id")?;
        if let Some(row) = next_row(&mut rows).await? {
            let title: String = row.get(0usize).into_diagnostic()?;
            let path_abs: String = row.get(1usize).into_diagnostic()?;
            let summary: String = row.get(2usize).into_diagnostic()?;
            let file = normalize_repo_relative_path(&path_abs, repo_root);
            results.push(SearchResult {
                title,
                file,
                summary,
                alias: None,
                snippets: Vec::new(),
            });
        }
    }
    Ok(results)
}

fn build_fts_query(query: &str) -> String {
    let tokens = search_tokens(query);
    tokens.join(" ")
}

fn search_tokens(query: &str) -> Vec<String> {
    let tokens = query
        .split(|character: char| !character.is_alphanumeric())
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(|token| token.to_lowercase())
        .collect::<Vec<_>>();

    if tokens.is_empty() {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            Vec::new()
        } else {
            vec![trimmed.to_lowercase()]
        }
    } else {
        tokens
    }
}

fn matched_snippets(source: &str, tokens: &[String]) -> Vec<Snippet> {
    if tokens.is_empty() {
        return Vec::new();
    }

    let highlight_re = RegexBuilder::new(
        &tokens
            .iter()
            .map(|token| regex::escape(token))
            .collect::<Vec<_>>()
            .join("|"),
    )
    .case_insensitive(true)
    .build()
    .expect("search token regex should compile");

    source
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }
            // Skip lines with only one alphanumeric word — e.g. `- runtime` —
            // as they carry no useful context beyond the match itself.
            let word_count = trimmed
                .split(|c: char| !c.is_alphanumeric())
                .filter(|w| !w.is_empty())
                .count();
            if word_count <= 1 {
                return None;
            }
            let normalized = trimmed.to_lowercase();
            tokens
                .iter()
                .any(|token| normalized.contains(token))
                .then(|| Snippet {
                    line: index + 1,
                    text: highlight_re
                        .replace_all(trimmed, |caps: &regex::Captures<'_>| {
                            format!("**{}**", &caps[0])
                        })
                        .into_owned(),
                })
        })
        .take(3)
        .collect()
}

fn line_snippet(source: &str, line_number: usize) -> Option<Snippet> {
    source
        .lines()
        .nth(line_number.saturating_sub(1))
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| Snippet {
            line: line_number,
            text: line.to_string(),
        })
}

fn token_match_score(title: &str, summary: &str, source: &str, tokens: &[String]) -> f64 {
    if tokens.is_empty() {
        return 0.0;
    }

    let corpus = format!("{title}\n{summary}\n{source}").to_lowercase();
    let matched = tokens
        .iter()
        .filter(|token| corpus.contains(token.as_str()))
        .count();
    matched as f64 / tokens.len() as f64
}

fn markdown_body(content: &str) -> String {
    let trimmed = content.trim_start_matches('\n');
    if !trimmed.starts_with("---\n") {
        return content.to_string();
    }
    let remainder = &trimmed[4..];
    if let Some(close) = remainder.find("\n---\n") {
        return remainder[close + 5..].to_string();
    }
    if let Some(close) = remainder.find("\n---\r\n") {
        return remainder[close + 6..].to_string();
    }
    content.to_string()
}

fn sha256_hex(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn canonical_display_path(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string()
}

fn relative_path(repo_root: &Path, path: &Path) -> Result<String> {
    path.strip_prefix(repo_root)
        .into_diagnostic()
        .wrap_err_with(|| {
            format!(
                "failed to compute path relative to repo root: {}",
                path.display()
            )
        })
        .map(|relative| relative.to_string_lossy().to_string())
}

fn metadata_modified_ns(metadata: &fs::Metadata) -> Result<i64> {
    let modified = metadata
        .modified()
        .into_diagnostic()
        .wrap_err("failed to get file modified time")?;
    system_time_to_ns(modified)
}

fn system_time_to_ns(time: SystemTime) -> Result<i64> {
    let duration = time
        .duration_since(UNIX_EPOCH)
        .into_diagnostic()
        .wrap_err("system time is before UNIX_EPOCH")?;
    i64::try_from(duration.as_nanos()).into_diagnostic()
}

fn unix_time_now_ns() -> Result<i64> {
    system_time_to_ns(SystemTime::now())
}

#[derive(Debug, Clone)]
struct PathCandidate {
    path_abs: String,
    path_rel: Option<String>,
}

fn path_candidates(repo_root: &Path, input: &str) -> Result<Vec<PathCandidate>> {
    let path = Path::new(input);
    let candidates = if path.is_absolute() {
        vec![path.to_path_buf()]
    } else {
        let mut candidates = Vec::new();
        if let Ok(current_dir) = std::env::current_dir() {
            candidates.push(current_dir.join(path));
        }
        candidates.push(repo_root.join(path));
        candidates
    };

    let mut resolved = Vec::new();
    for candidate in candidates {
        let path_abs = canonical_display_path(&candidate);
        let path_rel = candidate
            .strip_prefix(repo_root)
            .ok()
            .map(|relative| relative.to_string_lossy().to_string());
        resolved.push(PathCandidate { path_abs, path_rel });
    }

    Ok(resolved)
}

async fn get_state(conn: &Connection, key: &str) -> Result<Option<String>> {
    let mut rows = conn
        .query(
            "SELECT value FROM index_state WHERE key = ?1 LIMIT 1",
            params![key],
        )
        .await
        .into_diagnostic()
        .wrap_err("failed to query wiki index state")?;
    let Some(row) = next_row(&mut rows).await? else {
        return Ok(None);
    };
    Ok(Some(row_string(&row, 0)?))
}

async fn set_state(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "
        INSERT INTO index_state (key, value)
        VALUES (?1, ?2)
        ON CONFLICT(key) DO UPDATE SET value = excluded.value
        ",
        params![key, value],
    )
    .await
    .into_diagnostic()
    .wrap_err("failed to write wiki index state")?;
    Ok(())
}

async fn next_row(rows: &mut Rows) -> Result<Option<Row>> {
    rows.next()
        .await
        .into_diagnostic()
        .wrap_err("failed to advance database cursor")
}

fn row_string(row: &Row, index: usize) -> Result<String> {
    row.get::<String>(index)
        .into_diagnostic()
        .wrap_err("failed to decode string column")
}

fn row_i64(row: &Row, index: usize) -> Result<i64> {
    row.get::<i64>(index)
        .into_diagnostic()
        .wrap_err("failed to decode integer column")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    struct TestRepo {
        dir: TempDir,
    }

    impl TestRepo {
        fn new() -> Self {
            let dir = TempDir::new().expect("tempdir");
            let repo = Self { dir };
            repo.git(&["init"]);
            repo.git(&["checkout", "-b", "main"]);
            repo
        }

        fn path(&self) -> &Path {
            self.dir.path()
        }

        fn create_file(&self, path: &str, content: &str) {
            let full = self.dir.path().join(path);
            if let Some(parent) = full.parent() {
                fs::create_dir_all(parent).expect("create_dir_all");
            }
            fs::write(full, content).expect("write file");
        }

        fn rename(&self, from: &str, to: &str) {
            fs::rename(self.dir.path().join(from), self.dir.path().join(to)).expect("rename");
        }

        fn remove(&self, path: &str) {
            fs::remove_file(self.dir.path().join(path)).expect("remove");
        }

        fn git(&self, args: &[&str]) {
            let output = Command::new("git")
                .current_dir(self.dir.path())
                .args(args)
                .env("GIT_AUTHOR_NAME", "Test Author")
                .env("GIT_AUTHOR_EMAIL", "test@example.com")
                .env("GIT_COMMITTER_NAME", "Test Committer")
                .env("GIT_COMMITTER_EMAIL", "test@example.com")
                .output()
                .expect("spawn git");
            assert!(
                output.status.success(),
                "git {:?} failed:\n{}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    #[test]
    fn creates_index_and_resolves_pages() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file(
            "wiki/example.md",
            "---\ntitle: Example\naliases:\n  - Sample\ntags:\n  - docs\nsummary: Example summary.\n---\nBody with [[Other]].\n",
        );

        let index = WikiIndex::prepare(repo.path()).expect("prepare");
        assert!(repo.path().join("wiki/.index.db").exists());

        let page = index
            .resolve_page("sample")
            .expect("resolve")
            .expect("page");
        assert_eq!(page.title, "Example");
        assert_eq!(page.alias.as_deref(), Some("Sample"));
    }

    #[test]
    fn sync_removes_deleted_files_and_handles_renames() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file(
            "wiki/old.md",
            "---\ntitle: Old\nsummary: Old summary.\n---\nBody.\n",
        );

        let index = WikiIndex::prepare(repo.path()).expect("prepare");
        assert!(index.resolve_page("Old").expect("resolve").is_some());

        repo.rename("wiki/old.md", "wiki/new.md");
        repo.create_file(
            "wiki/new.md",
            "---\ntitle: New\nsummary: New summary.\n---\nBody.\n",
        );

        let index = WikiIndex::prepare(repo.path()).expect("prepare");
        assert!(index.resolve_page("Old").expect("resolve").is_none());
        assert!(index.resolve_page("New").expect("resolve").is_some());

        repo.remove("wiki/new.md");
        let index = WikiIndex::prepare(repo.path()).expect("prepare");
        assert!(index.resolve_page("New").expect("resolve").is_none());
    }

    #[test]
    fn search_weighted_prioritizes_exact_title_matches() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file(
            "wiki/title-match.md",
            "---\ntitle: Rust Indexing\nsummary: Title match.\n---\nBody.\n",
        );
        repo.create_file(
            "wiki/body-match.md",
            "---\ntitle: Secondary\nsummary: Body match.\n---\nRust indexing appears in the body.\n",
        );

        let index = WikiIndex::prepare(repo.path()).expect("prepare");
        let (results, _) = index
            .search_weighted("rust indexing", SEARCH_LIMIT, 0)
            .expect("search");

        assert_eq!(
            results.first().map(|result| result.title.as_str()),
            Some("Rust Indexing")
        );
        assert_eq!(
            results.get(1).map(|result| result.title.as_str()),
            Some("Secondary")
        );
        assert!(!results[1].snippets.is_empty());
    }

    #[test]
    fn search_weighted_prioritizes_path_matches() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file(
            "wiki/rust-indexing-guide.md",
            "---\ntitle: Unrelated\nsummary: Path match.\n---\nNothing here.\n",
        );
        repo.create_file(
            "wiki/body-match.md",
            "---\ntitle: Body Match\nsummary: Body match.\n---\nRust indexing guide appears in the body.\n",
        );

        let index = WikiIndex::prepare(repo.path()).expect("prepare");
        let (results, _) = index
            .search_weighted("rust indexing guide", SEARCH_LIMIT, 0)
            .expect("search");

        assert_eq!(
            results.first().map(|result| result.title.as_str()),
            Some("Unrelated")
        );
        assert_eq!(
            results.get(1).map(|result| result.title.as_str()),
            Some("Body Match")
        );
    }

    #[test]
    fn search_weighted_truncates_to_limit() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file(
            "wiki/a.md",
            "---\ntitle: Alpha\nsummary: Alpha summary.\n---\nneedle one.\n",
        );
        repo.create_file(
            "wiki/b.md",
            "---\ntitle: Beta\nsummary: Beta summary.\n---\nneedle two.\n",
        );
        repo.create_file(
            "wiki/c.md",
            "---\ntitle: Gamma\nsummary: Gamma summary.\n---\nneedle three.\n",
        );

        let index = WikiIndex::prepare(repo.path()).expect("prepare");
        let (results, total) = index.search_weighted("needle", 2, 0).expect("search");

        assert_eq!(results.len(), 2);
        assert_eq!(total, 3);
    }

    #[test]
    fn search_reflects_content_changes_after_resync() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file(
            "wiki/example.md",
            "---\ntitle: Example\nsummary: Example summary.\n---\nRust indexing appears here.\n",
        );

        let index = WikiIndex::prepare(repo.path()).expect("prepare");
        let initial = index.search("rust").expect("search");
        assert_eq!(initial.len(), 1);

        repo.create_file(
            "wiki/example.md",
            "---\ntitle: Example\nsummary: Example summary.\n---\nGraph traversal appears here.\n",
        );

        let index = WikiIndex::prepare(repo.path()).expect("prepare");
        assert!(index.search("rust").expect("search").is_empty());
        let updated = index.search("graph").expect("search");
        assert_eq!(updated.len(), 1);
        assert_eq!(updated[0].title, "Example");
    }

    #[test]
    fn prepare_updates_untracked_files_without_head() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file(
            "wiki/example.md",
            "---\ntitle: Example\nsummary: Example summary.\n---\nBody.\n",
        );

        let index = WikiIndex::prepare(repo.path()).expect("prepare");
        assert!(index.resolve_page("Example").expect("resolve").is_some());

        repo.create_file(
            "wiki/example.md",
            "---\ntitle: Renamed\nsummary: Example summary.\n---\nBody.\n",
        );

        let index = WikiIndex::prepare(repo.path()).expect("prepare");
        assert!(index.resolve_page("Example").expect("resolve").is_none());
        assert!(index.resolve_page("Renamed").expect("resolve").is_some());
    }

    #[test]
    fn prepare_invalidates_discovery_state_when_wiki_dir_changes() {
        let repo = TestRepo::new();
        let wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file(
            "wiki/example.md",
            "---\ntitle: Example\nsummary: Example summary.\n---\nBody.\n",
        );

        let index = WikiIndex::prepare(repo.path()).expect("prepare");
        assert!(index.resolve_page("Example").expect("resolve").is_some());

        repo.create_file(
            "docs/other.md",
            "---\ntitle: Other\nsummary: Other summary.\n---\nBody.\n",
        );
        repo.remove("wiki/example.md");
        drop(wiki_dir);
        let _wiki_dir = crate::test_support::set_wiki_dir("docs");

        let index = WikiIndex::prepare(repo.path()).expect("prepare");
        assert!(index.resolve_page("Example").expect("resolve").is_none());
        assert!(index.resolve_page("Other").expect("resolve").is_some());
    }
}
