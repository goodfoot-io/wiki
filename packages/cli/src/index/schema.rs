//! SQLite schema for the wiki search index.
//!
//! The schema is the single source of truth per `CARD.md` § Storage. The
//! FTS5 virtual table is external-content over `blobs`; triggers keep
//! `fts` synchronized so an FTS row dies the moment the corresponding
//! `blobs.refcount` hits zero.

pub const SCHEMA_VERSION: i64 = 1;

pub const SCHEMA_V1: &str = r#"
CREATE TABLE state (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  head_oid TEXT NOT NULL,
  head_tree_oid TEXT NOT NULL,
  index_checksum BLOB NOT NULL,
  worktree_generation INTEGER NOT NULL,
  schema_version INTEGER NOT NULL,
  generation INTEGER NOT NULL
) STRICT;

CREATE TABLE blobs (
  oid TEXT PRIMARY KEY,
  refcount INTEGER NOT NULL,
  title TEXT NOT NULL,
  summary TEXT NOT NULL,
  body TEXT NOT NULL,
  aliases_text TEXT NOT NULL,
  tags_text TEXT NOT NULL,
  keywords_text TEXT NOT NULL
) STRICT;

CREATE TABLE paths (
  path_rel TEXT NOT NULL,
  source INTEGER NOT NULL,
  oid TEXT NOT NULL REFERENCES blobs(oid),
  stat_mtime_ns INTEGER,
  stat_size INTEGER,
  stat_ino INTEGER,
  stat_ctime_ns INTEGER,
  parent_dir TEXT NOT NULL,
  PRIMARY KEY (path_rel, source)
) WITHOUT ROWID;

CREATE TABLE dir_mtimes (
  path TEXT PRIMARY KEY,
  mtime_ns INTEGER NOT NULL
) WITHOUT ROWID;

-- Search JOINs go FTS → blobs → paths on b.oid = p.oid filtered by source.
-- Without this index the JOIN scans `paths` per FTS hit (O(N²) on a 5k-doc
-- corpus, ~3s warm path). The composite (oid, source) lets the planner
-- satisfy the join + source filter from the index alone.
CREATE INDEX idx_paths_oid_source ON paths(oid, source);

CREATE INDEX idx_paths_parent_source ON paths(parent_dir, source);

CREATE VIRTUAL TABLE fts USING fts5(
  title, aliases_text, tags_text, keywords_text, summary, body,
  content='blobs',
  content_rowid='rowid',
  tokenize='unicode61 remove_diacritics 2',
  prefix='2 3 4'
);

"#;

/// Apply the schema and initial state row to `conn`, or validate an existing
/// schema. Idempotent: safe to call on an already-bootstrapped database.
///
/// # Errors
///
/// Returns `Err(rusqlite::Error::InvalidQuery)` when the existing
/// `schema_version` differs from [`SCHEMA_VERSION`] (greenfield — no
/// migrations).
pub fn bootstrap(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    // Pragmas must be executed outside a transaction.
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         PRAGMA busy_timeout=5000;
         PRAGMA mmap_size=268435456;",
    )?;

    // Detect whether the schema has been applied.
    let state_exists: bool = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='state'",
        [],
        |row| row.get::<_, i64>(0),
    )? > 0;

    if !state_exists {
        // First-time bootstrap: apply schema + triggers in one transaction.
        conn.execute_batch(&format!(
            "BEGIN;
             {schema}
             {triggers}
             INSERT INTO state (id, head_oid, head_tree_oid, index_checksum,
                                worktree_generation, schema_version, generation)
             VALUES (1, '', '', zeroblob(20), 0, {version}, 0);
             COMMIT;",
            schema = SCHEMA_V1,
            triggers = FTS_TRIGGERS,
            version = SCHEMA_VERSION,
        ))?;
    } else {
        // Validate that the stored schema version matches ours.
        let stored: i64 =
            conn.query_row("SELECT schema_version FROM state WHERE id = 1", [], |row| {
                row.get(0)
            })?;
        if stored != SCHEMA_VERSION {
            return Err(rusqlite::Error::InvalidQuery);
        }
    }

    Ok(())
}

/// External-content sync triggers. Kept separate from `SCHEMA_V1` so the
/// bootstrap path can rerun them defensively after schema upgrades.
pub const FTS_TRIGGERS: &str = r#"
CREATE TRIGGER blobs_ai AFTER INSERT ON blobs BEGIN
  INSERT INTO fts(rowid, title, aliases_text, tags_text, keywords_text, summary, body)
  VALUES (new.rowid, new.title, new.aliases_text, new.tags_text, new.keywords_text, new.summary, new.body);
END;

CREATE TRIGGER blobs_ad AFTER DELETE ON blobs BEGIN
  INSERT INTO fts(fts, rowid, title, aliases_text, tags_text, keywords_text, summary, body)
  VALUES ('delete', old.rowid, old.title, old.aliases_text, old.tags_text, old.keywords_text, old.summary, old.body);
END;

CREATE TRIGGER blobs_au AFTER UPDATE ON blobs BEGIN
  INSERT INTO fts(fts, rowid, title, aliases_text, tags_text, keywords_text, summary, body)
  VALUES ('delete', old.rowid, old.title, old.aliases_text, old.tags_text, old.keywords_text, old.summary, old.body);
  INSERT INTO fts(rowid, title, aliases_text, tags_text, keywords_text, summary, body)
  VALUES (new.rowid, new.title, new.aliases_text, new.tags_text, new.keywords_text, new.summary, new.body);
END;
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_creates_state_row_and_triggers() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        bootstrap(&conn).unwrap();

        // State row exists with correct schema version.
        let (count, version): (i64, i64) = conn
            .query_row(
                "SELECT COUNT(*), schema_version FROM state WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(version, SCHEMA_VERSION);
        assert_eq!(version, 1);

        // FTS triggers exist.
        let trigger_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='trigger' AND tbl_name='blobs'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(trigger_count, 3, "expected blobs_ai, blobs_ad, blobs_au");
    }

    #[test]
    fn bootstrap_is_idempotent() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        bootstrap(&conn).unwrap();
        // Second call must succeed without error.
        bootstrap(&conn).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM state", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }
}
