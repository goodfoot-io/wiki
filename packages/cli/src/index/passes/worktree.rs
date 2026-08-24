//! Pass 3 — `walkdir` walk of the worktree, with `gix::worktree::Stack`
//! used as a prune predicate so ignored directories are skipped.
//!
//! Carry-forward is path-granular (plan D5/D6): a walked markdown file
//! whose `(path_rel, stat_mtime_ns)` pair matches its base-generation row
//! is carried forward without re-reading or re-hashing; everything else is
//! re-ingested from this walk. There is no dir-mtime Merkle short-circuit
//! and no `dir_mtimes` table anymore. On a hostile filesystem
//! (`HostileFs::Yes`) the mtime evidence is never consulted — every file
//! re-ingests and the orchestrator bumps `pass3_full_rescans`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::Result;
use walkdir::WalkDir;

use crate::index::blob::compute_blob_oid;
use crate::index::{BlobOid, HostileFs, Source};
use crate::wikiignore::WikiIgnore;

use super::{DeltaAction, PassDelta};
use crate::index::generations::GenPathRow;

pub fn pass_worktree(
    repo: &gix::Repository,
    repo_root: &Path,
    base_rows: &[GenPathRow],
    hostile_fs: HostileFs,
    pass3_full_rescans: &mut u64,
    pass3_dir_walks: &mut u64,
    wiki_ignore: &WikiIgnore,
) -> Result<Vec<PassDelta>> {
    let is_hostile = matches!(hostile_fs, HostileFs::Yes);
    if is_hostile {
        *pass3_full_rescans += 1;
    }

    // Base-generation Worktree state: per-file oid and walk mtime. The
    // `(path_rel, stat_mtime_ns)` pair IS the carry-forward key.
    let prior_oids: HashMap<PathBuf, String> = base_rows
        .iter()
        .map(|row| (PathBuf::from(&row.path_rel), row.oid.0.clone()))
        .collect();
    let prior_mtimes: HashMap<PathBuf, Option<i64>> = base_rows
        .iter()
        .map(|row| (PathBuf::from(&row.path_rel), row.stat_mtime_ns))
        .collect();

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
            // Every descended directory counts — there is no clean-dir
            // short-circuit anymore; carry-forward happens per file below.
            *pass3_dir_walks += 1;
            continue;
        }

        if !entry.file_type().is_file() {
            continue;
        }
        if !is_markdown(&rel) {
            continue;
        }
        // Wikiignore gate at file level: never marked seen, so the removal
        // sweep purges any previously-indexed row.
        if wiki_ignore.is_ignored(&rel) {
            continue;
        }
        seen_paths.insert(rel.clone());

        let cur_mtime = mtime_ns(path);
        let prior_oid = prior_oids.get(&rel);
        let prior_mtime = prior_mtimes.get(&rel).copied().flatten();

        // Path-granular carry-forward: same path, stat-identical mtime, on
        // a filesystem we trust for mtime evidence ⇒ keep the base row
        // verbatim (the orchestrator seeded it already).
        if !is_hostile && cur_mtime.is_some() && prior_mtime == cur_mtime && prior_oid.is_some() {
            continue;
        }

        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let hashed = compute_blob_oid(&bytes);

        // Re-ingested from this walk — either the content changed (new oid
        // ⇒ parse + queue as a global upsert) or only the mtime moved on
        // identical content (already-known oid ⇒ the member row refreshes
        // its mtime without re-tokenizing). Both are plain Adds; the
        // candidate builder distinguishes them by oid knowledge.
        deltas.push(PassDelta {
            path: rel,
            source: Source::Worktree,
            action: DeltaAction::Add {
                oid: BlobOid(hashed.0),
                blob_bytes: Some(bytes),
                stat_mtime_ns: cur_mtime,
            },
        });
    }

    // Removals: base rows no longer observed in this walk.
    for row in base_rows {
        let pb = PathBuf::from(&row.path_rel);
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

fn mtime_ns(p: &Path) -> Option<i64> {
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
