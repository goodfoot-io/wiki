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

CREATE VIRTUAL TABLE fts USING fts5(
  title, aliases_text, tags_text, keywords_text, summary, body,
  content='blobs',
  content_rowid='rowid',
  tokenize='unicode61 remove_diacritics 2',
  prefix='2 3 4'
);
"#;

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
