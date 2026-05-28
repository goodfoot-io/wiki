//! Pass 3 — `walkdir` walk of the worktree, with `gix::worktree::Stack`
//! used as a prune predicate so ignored directories are skipped.
//!
//! On healthy filesystems we use a dir-mtime Merkle short-circuit: a
//! directory whose mtime matches the recorded `dir_mtimes` row is treated
//! as clean and its `.md` files are not re-stat'd or re-hashed. On a
//! hostile filesystem (`HostileFs::Yes`) the short-circuit is disabled
//! and the orchestrator bumps `pass3_full_rescans`; every directory is
//! treated as dirty.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::Result;
use rusqlite::params;
use walkdir::WalkDir;

use crate::index::blob::compute_blob_oid;
use crate::index::{BlobOid, HostileFs, Source};

use super::{DeltaAction, PassDelta, source_id};

pub fn pass_worktree(
    repo: &gix::Repository,
    repo_root: &Path,
    tx: &rusqlite::Transaction,
    hostile_fs: HostileFs,
    pass3_full_rescans: &mut u64,
    pass3_dir_walks: &mut u64,
) -> Result<Vec<PassDelta>> {
    let is_hostile = matches!(hostile_fs, HostileFs::Yes);
    if is_hostile {
        *pass3_full_rescans += 1;
    }

    // Snapshot dir_mtimes for this pass. The Merkle decision per directory
    // is: stat(D).mtime_ns == dir_mtimes[D] -> clean, skip files in D.
    let prior_dir_mtimes: HashMap<PathBuf, i64> = {
        let mut stmt = tx.prepare("SELECT path, mtime_ns FROM dir_mtimes")?;
        let rows = stmt.query_map([], |row| {
            let p: String = row.get(0)?;
            let m: i64 = row.get(1)?;
            Ok((PathBuf::from(p), m))
        })?;
        rows.collect::<rusqlite::Result<HashMap<_, _>>>()?
    };

    // Pre-index existing Worktree paths by parent dir so we can keep
    // their `seen_paths` entries (for clean dirs) and reconcile vanished
    // paths (for dirty dirs) without re-querying per file.
    let mut paths_by_parent: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    {
        let mut stmt = tx.prepare("SELECT path_rel, parent_dir FROM paths WHERE source = ?1")?;
        let rows = stmt.query_map(params![source_id(Source::Worktree)], |row| {
            let p: String = row.get(0)?;
            let d: String = row.get(1)?;
            Ok((PathBuf::from(p), PathBuf::from(d)))
        })?;
        for r in rows {
            let (path, parent) = r?;
            paths_by_parent.entry(parent).or_default().push(path);
        }
    }

    // Build an excludes stack so we can prune ignored directories cheaply.
    let index = repo
        .index_or_empty()
        .map_err(|e| anyhow::anyhow!("open index for excludes stack: {e}"))?;
    let mut stack = repo
        .excludes(
            &index,
            None,
            gix::worktree::stack::state::ignore::Source::WorktreeThenIdMappingIfNotSkipped,
        )
        .map_err(|e| anyhow::anyhow!("build excludes stack: {e}"))?;

    let mut seen_paths: HashSet<PathBuf> = HashSet::new();
    let mut deltas: Vec<PassDelta> = Vec::new();
    // Directories whose mtime we need to record after the walk.
    let mut new_dir_mtimes: Vec<(PathBuf, i64)> = Vec::new();

    let walker = WalkDir::new(repo_root).follow_links(false).into_iter();
    let walker = walker.filter_entry(|entry| {
        let path = entry.path();
        if path == repo_root {
            return true;
        }
        if entry.file_name() == ".git" {
            return false;
        }
        let Ok(rel) = path.strip_prefix(repo_root) else {
            return true;
        };
        let is_dir = entry.file_type().is_dir();
        if !is_dir {
            return true;
        }
        let platform = match stack.at_path(rel, Some(gix::index::entry::Mode::DIR)) {
            Ok(p) => p,
            Err(_) => return true,
        };
        !platform.is_excluded()
    });

    // We can't easily use the `for entry in walker` form because we want
    // to skip an entire subtree's file entries when the dir is clean.
    // walkdir gives us `skip_current_dir` only on the IntoIter, and
    // filter_entry wraps it. We use a per-directory "clean" set and
    // simply ignore file entries inside those dirs (we still descend so
    // child dirs are evaluated independently — child mtimes are
    // independent of parent mtimes).
    let mut clean_dirs: HashSet<PathBuf> = HashSet::new();

    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        let rel = match path.strip_prefix(repo_root) {
            Ok(r) => r.to_path_buf(),
            Err(_) => continue,
        };

        if entry.file_type().is_dir() {
            // Always count and decide cleanliness for every directory we
            // descend into (including the repo root, rel == "").
            let dir_mtime_ns = dir_mtime_ns(path);
            let prior = prior_dir_mtimes.get(&rel).copied();
            let is_clean =
                !is_hostile && dir_mtime_ns.is_some() && prior.is_some() && prior == dir_mtime_ns;

            if is_clean {
                // Carry forward this directory's known Worktree paths so
                // they survive the removal-reconciliation pass below.
                if let Some(children) = paths_by_parent.get(&rel) {
                    for p in children {
                        seen_paths.insert(p.clone());
                    }
                }
                clean_dirs.insert(rel.clone());
            } else {
                *pass3_dir_walks += 1;
                if let Some(m) = dir_mtime_ns {
                    new_dir_mtimes.push((rel.clone(), m));
                }
            }
            continue;
        }

        if !entry.file_type().is_file() {
            continue;
        }
        if !is_markdown(&rel) {
            continue;
        }
        // Skip files inside a clean directory — their parent's mtime
        // signalled the rows on disk are unchanged since last refresh.
        let parent = rel.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        if clean_dirs.contains(&parent) {
            continue;
        }

        seen_paths.insert(rel.clone());

        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let oid = compute_blob_oid(&bytes);

        let rel_str = rel.to_string_lossy().to_string();
        let prior: Option<String> = tx
            .query_row(
                "SELECT oid FROM paths WHERE path_rel = ?1 AND source = ?2",
                params![rel_str, source_id(Source::Worktree)],
                |r| r.get(0),
            )
            .ok();
        if prior.as_deref() != Some(oid.0.as_str()) {
            deltas.push(PassDelta {
                path: rel,
                source: Source::Worktree,
                action: DeltaAction::Add {
                    oid: BlobOid(oid.0),
                },
            });
        }
    }

    // Removals: Worktree paths recorded in the DB but no longer on disk
    // (or in a vanished/dirty dir that didn't re-observe them).
    let mut stmt = tx.prepare("SELECT path_rel FROM paths WHERE source = ?1")?;
    let rows: Vec<String> = stmt
        .query_map(params![source_id(Source::Worktree)], |r| {
            r.get::<_, String>(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(stmt);
    for row in rows {
        let pb = PathBuf::from(&row);
        if !seen_paths.contains(&pb) {
            deltas.push(PassDelta {
                path: pb,
                source: Source::Worktree,
                action: DeltaAction::Remove,
            });
        }
    }

    // Record dirty-dir mtimes for next refresh.
    for (rel, m) in new_dir_mtimes {
        let p = rel.to_string_lossy().to_string();
        tx.execute(
            "INSERT INTO dir_mtimes (path, mtime_ns) VALUES (?1, ?2)
             ON CONFLICT(path) DO UPDATE SET mtime_ns = excluded.mtime_ns",
            params![p, m],
        )?;
    }

    Ok(deltas)
}

fn dir_mtime_ns(p: &Path) -> Option<i64> {
    let meta = std::fs::metadata(p).ok()?;
    let mtime = meta.modified().ok()?;
    let dur = mtime.duration_since(UNIX_EPOCH).ok()?;
    Some(dur.as_nanos() as i64)
}

fn is_markdown(p: &std::path::Path) -> bool {
    p.extension()
        .and_then(|s| s.to_str())
        .map(|s| s.eq_ignore_ascii_case("md"))
        .unwrap_or(false)
}
