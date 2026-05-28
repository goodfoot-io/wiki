//! Pass 2 — `gix::index::File::at` entry iteration. Pread-mode reading
//! (NOT mmap) avoids SIGBUS on a concurrent index rewrite.

use std::collections::HashSet;
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

    let mut on_disk: HashSet<PathBuf> = HashSet::new();
    let mut deltas = Vec::new();

    for entry in file.entries() {
        let p = entry.path(&file);
        let path_str = p.to_string();
        let path = PathBuf::from(&path_str);
        if !is_markdown(&path) {
            continue;
        }
        on_disk.insert(path.clone());

        let on_disk_oid = entry.id.to_hex().to_string();
        let prior: Option<String> = tx
            .query_row(
                "SELECT oid FROM paths WHERE path_rel = ?1 AND source = ?2",
                params![path_str, source_id(Source::Index)],
                |r| r.get(0),
            )
            .ok();
        if prior.as_deref() != Some(on_disk_oid.as_str()) {
            deltas.push(PassDelta {
                path,
                source: Source::Index,
                action: DeltaAction::Add {
                    oid: BlobOid(on_disk_oid),
                },
            });
        }
    }

    // Removals: paths that exist in the DB as Source::Index but not in the
    // current index file.
    let mut stmt = tx.prepare("SELECT path_rel FROM paths WHERE source = ?1")?;
    let rows: Vec<String> = stmt
        .query_map(params![source_id(Source::Index)], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for row in rows {
        if !on_disk.contains(&PathBuf::from(&row)) {
            deltas.push(PassDelta {
                path: PathBuf::from(row),
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
