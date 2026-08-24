//! Pass 2 — `gix::index::File::at` entry iteration. Pread-mode reading
//! (NOT mmap) avoids SIGBUS on a concurrent index rewrite.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::index::{BlobOid, Source};
use crate::wikiignore::WikiIgnore;

use super::{DeltaAction, PassDelta};

pub fn pass_index(
    dot_git: &Path,
    last_index_checksum: &[u8; 20],
    prior_oids_all: &HashMap<PathBuf, String>,
    wiki_ignore: &WikiIgnore,
    wikiignore_changed: bool,
) -> Result<Vec<PassDelta>> {
    let index_path = dot_git.join("index");
    if !index_path.exists() {
        // No index file -> remove every Source::Index row.
        return remove_all_index_rows(prior_oids_all);
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

    // Cheap shortcut: if the on-disk checksum matches the prior snapshot AND
    // the wikiignore is unchanged, nothing changed since last refresh.
    // When the wikiignore changed we must bypass this gate: the git index
    // checksum may be unchanged even though a file that was previously indexed
    // is now wikiignored (or was previously ignored and must be re-added).
    if !wikiignore_changed
        && file
            .checksum()
            .map(|c| c.as_bytes() == last_index_checksum)
            .unwrap_or(false)
    {
        return Ok(Vec::new());
    }

    // Base-generation Index mappings feed both the entry loop and the
    // removal reconciliation below.
    let prior_oids = prior_oids_all;

    let mut on_disk: HashSet<PathBuf> = HashSet::new();
    let mut deltas = Vec::new();

    for entry in file.entries() {
        let p = entry.path(&file);
        let path = PathBuf::from(p.to_string());
        if !is_markdown(&path) {
            continue;
        }
        // Wikiignored paths are never indexed; treat as absent so any
        // previously-indexed row is purged by the removal sweep below.
        if wiki_ignore.is_ignored(&path) {
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

fn remove_all_index_rows(
    prior_oids: &HashMap<PathBuf, String>,
) -> Result<Vec<PassDelta>> {
    Ok(prior_oids
        .keys()
        .map(|path| PassDelta {
            path: path.clone(),
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
