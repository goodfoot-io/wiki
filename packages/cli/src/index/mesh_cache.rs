//! Memoization cache for mesh anchor freshness and move resolution.
//!
//! The `mesh_anchor` table stores the last-verified content hash and resolved
//! location for each `(slug, path, start_line, end_line)` tuple, tagged with
//! the `worktree_generation` at which the entry was verified.
//!
//! A cache hit is valid only when `verified_generation` equals the current
//! `worktree_generation` **and** the anchor's file is not in the working-tree
//! change set. The caller is responsible for the change-set guard; this module
//! exposes only the generation-aware lookup/upsert interface.
//!
//! Correctness is not dependent on the cache — a cold or missing cache
//! produces identical [`crate::commands::check_fix::MeshMovePlan`] results,
//! only without the memoization speedup.

use std::path::Path;

use rusqlite::{Connection, OpenFlags, params};

use crate::index::find_dot_git;
use crate::index::schema;

/// A cached anchor entry returned on a generation-matching hit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedAnchor {
    /// The stored content hash (may carry a `sha256:` prefix).
    pub content_hash: String,
    /// Repo-relative path where the content was last located.
    pub loc_path: String,
    /// Start line of the located range (0 for whole-file).
    pub loc_start: u32,
    /// End line of the located range (0 for whole-file).
    pub loc_end: u32,
}

/// Resolved location to write into the cache alongside a content hash.
pub struct CacheLocation<'a> {
    pub path: &'a str,
    pub start: u32,
    pub end: u32,
}

/// Short-lived handle to the `mesh_anchor` cache table.
///
/// Open one per `plan_mesh_follows` call; drop it when done. Does **not**
/// participate in the refresh-lock lifecycle.
pub struct MeshCache {
    conn: Connection,
    worktree_generation: i64,
}

impl MeshCache {
    /// Open (or create) the wiki-index database for `repo_root` and read the
    /// current `worktree_generation` from the `state` row.
    ///
    /// Returns `None` when `.git` cannot be located, the DB cannot be opened,
    /// or the `state` row is absent — in all these cases the caller falls back
    /// to full recomputation.
    pub fn open(repo_root: &Path) -> Option<Self> {
        let dot_git = find_dot_git(repo_root)?;
        let db_path = dot_git.join("wiki-index.sqlite");

        // Open read-write (creates the file if needed so we can upsert later).
        let conn = Connection::open_with_flags(
            &db_path,
            OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_READ_WRITE,
        )
        .ok()?;

        // Bootstrap the schema (idempotent). On version mismatch the DB
        // rebuilds — the new `mesh_anchor` table will be present after that.
        if schema::bootstrap(&conn).is_err() {
            return None;
        }

        let worktree_generation: i64 = conn
            .query_row(
                "SELECT worktree_generation FROM state WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .ok()?;

        Some(MeshCache { conn, worktree_generation })
    }

    /// Look up a cached anchor for `(slug, path, start_line, end_line)`.
    ///
    /// Returns `Some(CachedAnchor)` only when an entry exists **and** its
    /// `verified_generation` equals the current `worktree_generation`. Any
    /// other condition (miss, generation mismatch, DB error) returns `None`.
    pub fn lookup(
        &self,
        slug: &str,
        path: &str,
        start_line: u32,
        end_line: u32,
    ) -> Option<CachedAnchor> {
        self.conn
            .query_row(
                "SELECT content_hash, loc_path, loc_start, loc_end, verified_generation
                 FROM mesh_anchor
                 WHERE slug = ?1 AND path = ?2 AND start_line = ?3 AND end_line = ?4",
                params![slug, path, start_line, end_line],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, u32>(2)?,
                        row.get::<_, u32>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                },
            )
            .ok()
            .and_then(|(content_hash, loc_path, loc_start, loc_end, verified_gen)| {
                if verified_gen == self.worktree_generation {
                    Some(CachedAnchor { content_hash, loc_path, loc_start, loc_end })
                } else {
                    None
                }
            })
    }

    /// Insert or replace a cache entry for `(slug, path, start_line, end_line)`.
    ///
    /// Errors are silently ignored — the cache is a pure optimisation; a failed
    /// write has no correctness impact.
    pub fn upsert(
        &self,
        slug: &str,
        path: &str,
        start_line: u32,
        end_line: u32,
        content_hash: &str,
        loc: CacheLocation<'_>,
    ) {
        let _ = self.conn.execute(
            "INSERT OR REPLACE INTO mesh_anchor
             (slug, path, start_line, end_line, content_hash, loc_path, loc_start, loc_end,
              verified_generation)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                slug,
                path,
                start_line,
                end_line,
                content_hash,
                loc.path,
                loc.start,
                loc.end,
                self.worktree_generation,
            ],
        );
    }

    /// The `worktree_generation` this cache was opened against.
    #[cfg(test)]
    pub fn worktree_generation(&self) -> i64 {
        self.worktree_generation
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    fn make_git_repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(root)
            .status()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "t@t"])
            .current_dir(root)
            .status()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "t"])
            .current_dir(root)
            .status()
            .unwrap();
        fs::write(root.join("README.md"), "hello").unwrap();
        Command::new("git")
            .args(["add", "-A"])
            .current_dir(root)
            .status()
            .unwrap();
        Command::new("git")
            .args(["commit", "-q", "-m", "init"])
            .current_dir(root)
            .status()
            .unwrap();
        tmp
    }

    #[test]
    fn mesh_anchor_table_bootstrap_and_idempotency() {
        let tmp = make_git_repo();
        let root = tmp.path();

        // First open creates the DB and bootstraps the schema.
        let cache = MeshCache::open(root).expect("cache should open");
        assert_eq!(cache.worktree_generation(), 0);

        // Second open is idempotent.
        let cache2 = MeshCache::open(root).expect("second open should succeed");
        assert_eq!(cache2.worktree_generation(), 0);
    }

    #[test]
    fn cache_round_trip_hit() {
        let tmp = make_git_repo();
        let cache = MeshCache::open(tmp.path()).unwrap();

        // Miss before upsert.
        assert!(cache.lookup("my-mesh", "src/foo.rs", 10, 20).is_none());

        cache.upsert(
            "my-mesh", "src/foo.rs", 10, 20,
            "sha256:abc123",
            CacheLocation { path: "src/foo.rs", start: 10, end: 20 },
        );

        // Hit after upsert.
        let entry = cache.lookup("my-mesh", "src/foo.rs", 10, 20).unwrap();
        assert_eq!(entry.content_hash, "sha256:abc123");
        assert_eq!(entry.loc_path, "src/foo.rs");
        assert_eq!(entry.loc_start, 10);
        assert_eq!(entry.loc_end, 20);
    }

    #[test]
    fn generation_mismatch_is_a_miss() {
        let tmp = make_git_repo();

        // Open and write at generation 0.
        let cache = MeshCache::open(tmp.path()).unwrap();
        cache.upsert("slug", "a.rs", 1, 5, "sha256:xyz", CacheLocation { path: "a.rs", start: 1, end: 5 });

        // Manually bump worktree_generation so the next open sees gen 1.
        {
            let dot_git = find_dot_git(tmp.path()).unwrap();
            let conn = Connection::open(dot_git.join("wiki-index.sqlite")).unwrap();
            conn.execute(
                "UPDATE state SET worktree_generation = 1 WHERE id = 1",
                [],
            )
            .unwrap();
        }

        // Reopen; the entry written at gen 0 must be a miss at gen 1.
        let cache2 = MeshCache::open(tmp.path()).unwrap();
        assert_eq!(cache2.worktree_generation(), 1);
        assert!(cache2.lookup("slug", "a.rs", 1, 5).is_none());
    }
}
