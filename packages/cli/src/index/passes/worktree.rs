//! Pass 3 — `walkdir` walk of the worktree, with `gix::worktree::Stack`
//! used as a prune predicate so ignored directories are skipped.
//!
//! On healthy filesystems we read each `.md` file unconditionally (Group B
//! correctness); the dir-mtime Merkle short-circuit lands in Group C. On
//! a hostile filesystem (`HostileFs::Yes`) the orchestrator bumps
//! `pass3_full_rescans`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

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
) -> Result<Vec<PassDelta>> {
    if matches!(hostile_fs, HostileFs::Yes) {
        *pass3_full_rescans += 1;
    }

    // Build an excludes stack so we can prune ignored directories cheaply.
    // Use the in-tree index (empty if the repo has no commits yet).
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

    let walker = WalkDir::new(repo_root).follow_links(false).into_iter();
    let walker = walker.filter_entry(|entry| {
        let path = entry.path();
        // Always allow the root.
        if path == repo_root {
            return true;
        }
        // Never descend into .git/.
        if entry.file_name() == ".git" {
            return false;
        }
        let Ok(rel) = path.strip_prefix(repo_root) else {
            return true;
        };
        let is_dir = entry.file_type().is_dir();
        if !is_dir {
            // Files are always kept — the exclude stack is a directory-prune
            // predicate only; gitignored .md files remain searchable.
            return true;
        }
        let platform = match stack.at_path(rel, Some(gix::index::entry::Mode::DIR)) {
            Ok(p) => p,
            Err(_) => return true,
        };
        !platform.is_excluded()
    });

    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let Ok(rel) = path.strip_prefix(repo_root) else {
            continue;
        };
        if !is_markdown(rel) {
            continue;
        }
        let rel_buf = rel.to_path_buf();
        seen_paths.insert(rel_buf.clone());

        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let oid = compute_blob_oid(&bytes);

        let rel_str = rel_buf.to_string_lossy().to_string();
        let prior: Option<String> = tx
            .query_row(
                "SELECT oid FROM paths WHERE path_rel = ?1 AND source = ?2",
                params![rel_str, source_id(Source::Worktree)],
                |r| r.get(0),
            )
            .ok();
        if prior.as_deref() != Some(oid.0.as_str()) {
            deltas.push(PassDelta {
                path: rel_buf,
                source: Source::Worktree,
                action: DeltaAction::Add { oid: BlobOid(oid.0) },
            });
        }
    }

    // Removals: Worktree paths recorded in the DB but no longer on disk.
    let mut stmt = tx.prepare("SELECT path_rel FROM paths WHERE source = ?1")?;
    let rows: Vec<String> = stmt
        .query_map(params![source_id(Source::Worktree)], |r| r.get::<_, String>(0))?
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

    Ok(deltas)
}

fn is_markdown(p: &std::path::Path) -> bool {
    p.extension()
        .and_then(|s| s.to_str())
        .map(|s| s.eq_ignore_ascii_case("md"))
        .unwrap_or(false)
}
