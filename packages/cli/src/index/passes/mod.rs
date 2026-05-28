//! Pass orchestrator: `Tree -> Index -> Worktree` merged into a single
//! `seen_paths` map.
//!
//! All three passes emit [`PassDelta`] values; the orchestrator merges
//! them with strict later-source-wins ordering, applies refcount-driven
//! blob bookkeeping, and re-tokenizes only on actual content changes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use rusqlite::{Connection, params};

use crate::index::ingest::{WikiBlobFields, parse_blob};
use crate::index::{BlobOid, HostileFs, Source};

pub mod index_file;
pub mod tree;
pub mod worktree;

/// Numeric encoding of [`Source`] used in the `paths.source` column.
pub(crate) fn source_id(s: Source) -> i64 {
    match s {
        Source::Tree => 0,
        Source::Index => 1,
        Source::Worktree => 2,
    }
}

/// One change observed by a single pass.
#[derive(Debug, Clone)]
pub struct PassDelta {
    /// Repo-root-relative path.
    pub path: PathBuf,
    pub source: Source,
    pub action: DeltaAction,
}

#[derive(Debug, Clone)]
pub enum DeltaAction {
    /// Path is present with the given blob OID.
    Add { oid: BlobOid },
    /// Path is no longer present in this source.
    Remove,
    /// Pass 1 rewrite — a rename inside the tree. `from` is the previous
    /// path; the new path is the delta's `path`.
    Rename { from: PathBuf, oid: BlobOid },
}

/// Counters returned from the orchestrator.
#[derive(Debug, Clone, Copy, Default)]
pub struct RefreshOutcome {
    #[allow(dead_code)]
    pub deltas_applied: usize,
    pub fts_retokenizations: u64,
    pub pass3_full_rescans: u64,
    /// Number of directories Pass 3 had to descend into and stat/hash files
    /// for (i.e. directories whose mtime differed from the recorded
    /// `dir_mtimes` entry, or all directories on a hostile filesystem).
    pub pass3_dir_walks: u64,
    /// `true` when the orchestrator lost the state-row CAS on commit and
    /// the transaction was rolled back. The caller must serve the prior
    /// snapshot and must not bump any freshness gates.
    pub cas_lost: bool,
}

/// Resolve a blob's bytes — first from disk (worktree), then via gix
/// (committed/staged blobs that may not exist as files).
fn read_blob_bytes(
    repo: &gix::Repository,
    repo_root: &Path,
    path_rel: &Path,
    oid: &BlobOid,
) -> Result<Vec<u8>> {
    let abs = repo_root.join(path_rel);
    if let Ok(bytes) = std::fs::read(&abs) {
        return Ok(bytes);
    }
    let id = gix::ObjectId::from_hex(oid.0.as_bytes())
        .map_err(|e| anyhow::anyhow!("invalid blob oid `{}`: {e}", oid.0))?;
    let blob = repo
        .find_blob(id)
        .map_err(|e| anyhow::anyhow!("blob {} not in odb: {e}", oid.0))?;
    Ok(blob.data.to_vec())
}

/// Drive Pass 1, Pass 2, Pass 3 and apply their deltas inside a single
/// `BEGIN IMMEDIATE` transaction.
pub fn refresh(
    repo: &gix::Repository,
    repo_root: &Path,
    dot_git: &Path,
    conn: &mut Connection,
    hostile_fs: HostileFs,
) -> Result<RefreshOutcome> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;

    // Read prior state (head_tree_oid + index_checksum + generation).
    let (prior_head_tree, prior_index_checksum, prior_generation): (String, Vec<u8>, i64) =
        tx.query_row(
            "SELECT head_tree_oid, index_checksum, generation FROM state WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(3 - 1)?)),
        )?;
    let prior_head_tree_oid = if prior_head_tree.is_empty() {
        None
    } else {
        Some(
            gix::ObjectId::from_hex(prior_head_tree.as_bytes())
                .map_err(|e| anyhow::anyhow!("decode prior head_tree_oid: {e}"))?,
        )
    };

    // Pass 1: Tree (committed snapshot).
    let tree_deltas = tree::pass_tree(repo, prior_head_tree_oid)?;

    // Pass 2: Index file.
    let prior_index_checksum_arr: [u8; 20] = if prior_index_checksum.len() == 20 {
        let mut a = [0u8; 20];
        a.copy_from_slice(&prior_index_checksum);
        a
    } else {
        [0u8; 20]
    };
    let index_deltas = index_file::pass_index(dot_git, &prior_index_checksum_arr, &tx)?;

    // Pass 3: Worktree.
    let mut pass3_full_rescans: u64 = 0;
    let mut pass3_dir_walks: u64 = 0;
    let worktree_deltas = worktree::pass_worktree(
        repo,
        repo_root,
        &tx,
        hostile_fs,
        &mut pass3_full_rescans,
        &mut pass3_dir_walks,
    )?;

    // Merge deltas in strict order Tree -> Index -> Worktree.
    // For each (source, path) we want the final OID assignment to obey
    // later-source-wins.
    let all_deltas: Vec<PassDelta> = tree_deltas
        .into_iter()
        .chain(index_deltas)
        .chain(worktree_deltas)
        .collect();

    let deltas_applied = all_deltas.len();
    let mut fts_retokenizations: u64 = 0;

    // Apply each delta.
    for delta in &all_deltas {
        match &delta.action {
            DeltaAction::Add { oid } => {
                apply_add(
                    repo,
                    repo_root,
                    &tx,
                    &delta.path,
                    delta.source,
                    oid,
                    &mut fts_retokenizations,
                )?;
            }
            DeltaAction::Remove => {
                apply_remove(&tx, &delta.path, delta.source)?;
            }
            DeltaAction::Rename { from, oid } => {
                // A rename keeps refcount > 0 — the path swap is row-level.
                apply_rename(&tx, from, &delta.path, delta.source, oid)?;
            }
        }
    }

    // Update state row. Compute new head_tree_oid from HEAD (or empty).
    let new_head_tree = repo
        .head_tree_id()
        .ok()
        .map(|id| id.to_hex().to_string())
        .unwrap_or_default();
    // Normalize unborn HEAD to the 40-zero sentinel so it matches what
    // `freshness::read_head_oid` returns on the same condition. Otherwise the
    // fast triple gate misses on every invocation against an unborn repo.
    let new_head_oid = repo
        .head_id()
        .ok()
        .map(|id| id.to_hex().to_string())
        .unwrap_or_else(|| "0".repeat(40));
    let new_index_checksum: Vec<u8> = match gix::index::File::at(
        dot_git.join("index"),
        gix::hash::Kind::Sha1,
        false,
        Default::default(),
    ) {
        Ok(file) => file
            .checksum()
            .map(|c| c.as_bytes().to_vec())
            .unwrap_or_else(|| vec![0u8; 20]),
        Err(_) => vec![0u8; 20],
    };

    // Compare-and-swap on the prior generation. If another writer
    // committed between our state read and this UPDATE, zero rows match
    // and we must roll back — the loser does not bump any freshness
    // signal and the caller continues to serve the prior snapshot.
    let updated = tx.execute(
        "UPDATE state SET head_oid = ?1, head_tree_oid = ?2, index_checksum = ?3,
                          generation = generation + 1
         WHERE id = 1 AND generation = ?4",
        params![new_head_oid, new_head_tree, new_index_checksum, prior_generation],
    )?;

    if updated == 0 {
        // CAS lost — roll back every delta we applied in this tx.
        drop(tx);
        return Ok(RefreshOutcome {
            deltas_applied: 0,
            fts_retokenizations: 0,
            pass3_full_rescans: 0,
            pass3_dir_walks: 0,
            cas_lost: true,
        });
    }

    tx.commit()?;

    Ok(RefreshOutcome {
        deltas_applied,
        fts_retokenizations,
        pass3_full_rescans,
        pass3_dir_walks,
        cas_lost: false,
    })
}

fn apply_add(
    repo: &gix::Repository,
    repo_root: &Path,
    tx: &rusqlite::Transaction,
    path_rel: &Path,
    source: Source,
    oid: &BlobOid,
    fts_retokenizations: &mut u64,
) -> Result<()> {
    // Look up any existing path row at (path_rel, source).
    let path_str = path_rel.to_string_lossy().to_string();
    let existing_oid: Option<String> = tx
        .query_row(
            "SELECT oid FROM paths WHERE path_rel = ?1 AND source = ?2",
            params![path_str, source_id(source)],
            |r| r.get(0),
        )
        .ok();

    // Ensure the blob exists in the `blobs` table. If not, parse the
    // bytes; non-wiki blobs cause us to skip the path row entirely.
    if !blob_exists(tx, oid)? {
        let bytes = read_blob_bytes(repo, repo_root, path_rel, oid)?;
        let Some(fields) = parse_blob(&bytes) else {
            // Not a wiki page; skip recording this path entirely. If a
            // stale row existed at this (path, source), drop it.
            if let Some(prev) = existing_oid.as_ref() {
                tx.execute(
                    "DELETE FROM paths WHERE path_rel = ?1 AND source = ?2",
                    params![path_str, source_id(source)],
                )?;
                decrement_blob(tx, &BlobOid(prev.clone()))?;
            }
            return Ok(());
        };
        insert_blob(tx, oid, &fields)?;
        *fts_retokenizations += 1;
    } else {
        // Blob already known to be a wiki page; still need to verify the
        // bytes parse — but that's a one-shot per OID, so trust the row.
    }

    match existing_oid {
        Some(prev) if prev == oid.0 => {
            // No-op: same path, same OID.
        }
        Some(prev) => {
            // Path's OID changed within the same source.
            tx.execute(
                "UPDATE paths SET oid = ?1 WHERE path_rel = ?2 AND source = ?3",
                params![oid.0, path_str, source_id(source)],
            )?;
            increment_blob(tx, oid)?;
            decrement_blob(tx, &BlobOid(prev))?;
        }
        None => {
            let parent = path_rel
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            tx.execute(
                "INSERT INTO paths (path_rel, source, oid, parent_dir)
                 VALUES (?1, ?2, ?3, ?4)",
                params![path_str, source_id(source), oid.0, parent],
            )?;
            increment_blob(tx, oid)?;
        }
    }

    Ok(())
}

fn apply_remove(
    tx: &rusqlite::Transaction,
    path_rel: &Path,
    source: Source,
) -> Result<()> {
    let path_str = path_rel.to_string_lossy().to_string();
    let prev: Option<String> = tx
        .query_row(
            "SELECT oid FROM paths WHERE path_rel = ?1 AND source = ?2",
            params![path_str, source_id(source)],
            |r| r.get(0),
        )
        .ok();
    if let Some(prev) = prev {
        tx.execute(
            "DELETE FROM paths WHERE path_rel = ?1 AND source = ?2",
            params![path_str, source_id(source)],
        )?;
        decrement_blob(tx, &BlobOid(prev))?;
    }
    Ok(())
}

fn apply_rename(
    tx: &rusqlite::Transaction,
    from: &Path,
    to: &Path,
    source: Source,
    oid: &BlobOid,
) -> Result<()> {
    let from_str = from.to_string_lossy().to_string();
    let to_str = to.to_string_lossy().to_string();
    let parent = to
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    // Delete the old row.
    tx.execute(
        "DELETE FROM paths WHERE path_rel = ?1 AND source = ?2",
        params![from_str, source_id(source)],
    )?;
    // Upsert the new row at the same OID — refcount unchanged because
    // we did not bump on either side.
    tx.execute(
        "INSERT OR REPLACE INTO paths (path_rel, source, oid, parent_dir)
         VALUES (?1, ?2, ?3, ?4)",
        params![to_str, source_id(source), oid.0, parent],
    )?;
    Ok(())
}

fn blob_exists(tx: &rusqlite::Transaction, oid: &BlobOid) -> Result<bool> {
    let n: i64 = tx.query_row(
        "SELECT COUNT(*) FROM blobs WHERE oid = ?1",
        params![oid.0],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

fn insert_blob(
    tx: &rusqlite::Transaction,
    oid: &BlobOid,
    fields: &WikiBlobFields,
) -> Result<()> {
    tx.execute(
        "INSERT INTO blobs (oid, refcount, title, summary, body, aliases_text, tags_text, keywords_text)
         VALUES (?1, 0, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            oid.0,
            fields.title,
            fields.summary,
            fields.body,
            fields.aliases_text,
            fields.tags_text,
            fields.keywords_text,
        ],
    )?;
    Ok(())
}

fn increment_blob(tx: &rusqlite::Transaction, oid: &BlobOid) -> Result<()> {
    tx.execute(
        "UPDATE blobs SET refcount = refcount + 1 WHERE oid = ?1",
        params![oid.0],
    )?;
    Ok(())
}

fn decrement_blob(tx: &rusqlite::Transaction, oid: &BlobOid) -> Result<()> {
    tx.execute(
        "UPDATE blobs SET refcount = refcount - 1 WHERE oid = ?1",
        params![oid.0],
    )?;
    tx.execute(
        "DELETE FROM blobs WHERE oid = ?1 AND refcount <= 0",
        params![oid.0],
    )?;
    Ok(())
}

/// In-memory cache for `paths`-table reads (currently unused; reserved
/// for Group C's incremental pass cache).
#[allow(dead_code)]
pub(crate) type SeenPaths = HashMap<PathBuf, (Source, BlobOid)>;
