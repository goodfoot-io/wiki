//! Pass 2 — `gix::index::File::at` entry iteration. Pread-mode reading
//! (NOT mmap) avoids SIGBUS on a concurrent index rewrite.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::Result;
use rusqlite::params;

use crate::index::{BlobOid, Source};

use super::{DeltaAction, PassDelta, source_id};

pub fn pass_index(
    dot_git: &Path,
    last_index_checksum: &[u8; 20],
    tx: &rusqlite::Transaction,
) -> Result<Vec<PassDelta>> {
    let index_path = dot_git.join("index");
    if !index_path.exists() {
        // No index file -> remove every Source::Index row.
        return remove_all_index_rows(tx);
    }

    let file = match gix::index::File::at(
        &index_path,
        gix::hash::Kind::Sha1,
        false, // false = pread (not mmap)
        Default::default(),
    ) {
        Ok(f) => f,
        Err(_) => return Ok(Vec::new()),
    };

    // Cheap shortcut: if the on-disk checksum matches the prior snapshot,
    // nothing changed since last refresh.
    if let Some(checksum) = file.checksum()
        && checksum.as_bytes() == last_index_checksum
    {
        return Ok(Vec::new());
    }

    // Batch-load all Source::Index path→oid mappings before the entry loop.
    // This replaces the per-file point query that was inside the loop with a
    // single query whose result set also feeds the removal reconciliation below.
    let mut prior_oids: HashMap<PathBuf, String> = HashMap::new();
    {
        let mut stmt = tx.prepare("SELECT path_rel, oid FROM paths WHERE source = ?1")?;
        let rows = stmt.query_map(params![source_id(Source::Index)], |row| {
            let p: String = row.get(0)?;
            let o: String = row.get(1)?;
            Ok((PathBuf::from(p), o))
        })?;
        for r in rows {
            let (path, oid) = r?;
            prior_oids.insert(path, oid);
        }
    }

    let mut on_disk: HashSet<PathBuf> = HashSet::new();
    let mut deltas = Vec::new();

    for entry in file.entries() {
        let p = entry.path(&file);
        let path = PathBuf::from(p.to_string());
        if !is_markdown(&path) {
            continue;
        }
        on_disk.insert(path.clone());

        let on_disk_oid = entry.id.to_hex().to_string();
        let prior = prior_oids.get(&path).map(|s| s.as_str());
        if prior != Some(on_disk_oid.as_str()) {
            deltas.push(PassDelta {
                path,
                source: Source::Index,
                action: DeltaAction::Add {
                    oid: BlobOid(on_disk_oid),
                    blob_bytes: None,
                    stat_mtime_ns: None,
                },
            });
        }
    }

    // Removals: DB paths not present in the current index file.
    for path in prior_oids.keys() {
        if !on_disk.contains(path) {
            deltas.push(PassDelta {
                path: path.clone(),
                source: Source::Index,
                action: DeltaAction::Remove,
            });
        }
    }

    Ok(deltas)
}

fn remove_all_index_rows(tx: &rusqlite::Transaction) -> Result<Vec<PassDelta>> {
    let mut stmt = tx.prepare("SELECT path_rel FROM paths WHERE source = ?1")?;
    let rows: Vec<String> = stmt
        .query_map(params![source_id(Source::Index)], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows
        .into_iter()
        .map(|r| PassDelta {
            path: PathBuf::from(r),
            source: Source::Index,
            action: DeltaAction::Remove,
        })
        .collect())
}

fn is_markdown(p: &std::path::Path) -> bool {
    p.extension()
        .and_then(|s| s.to_str())
        .map(|s| s.eq_ignore_ascii_case("md"))
        .unwrap_or(false)
}
