//! Acceptance checks for the anchor cache (card main-4 plan decisions 1–5, 8).
//!
//! Written in tdd-bootstrap Phase 2: every check is `#[ignore]`d against the
//! P1 contract surface (stubs) and unskipped one at a time as the store is
//! implemented — the same bootstrap order body_fts_integration.rs documents.
//! A skipped check that does not compile is still broken, so this file must
//! stay clean under lint and typecheck, and a `cargo test` run must show
//! every check here as ignored, never failing or erroring.
//!
//! Groups: (a) key-derivation vectors and injectivity, (b) probe
//! classification, quarantine, open ordering, and store roundtrips, (c)
//! linked-worktree sharing of `git::common_dir`.
//!
//! All fixtures live in temp dirs — never in the workspace tree.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

use wiki::cache::key::{fingerprint_key, sha256_hex, walk_key};
use wiki::cache::schema::{
    ANCHOR_WALK_DDL, APPLICATION_ID, BUSY_TIMEOUT_MS, DB_FILE_NAME, FINGERPRINT_DDL, META_DDL,
    ProbeOutcome, SCHEMA_VERSION, SuspectKind, Tier, db_path, init_lock_path, open_connection,
    probe, quarantine,
};
use wiki::cache::{AnchorCache, CacheStore, WalkRow};

/// A 40-hex commit SHA for fixtures.
const SHA: &str = "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6a7b8c9d0";
/// A 16-hex rk64 fingerprint for fixtures.
const FP: &str = "0123456789abcdef";
/// A tier-A walk input: raw `git log --follow --name-status --format=%H -- <page>`
/// output, exact and untrimmed (commit SHAs, blank separator lines, name-status
/// rows, trailing blank line).
const LOG_ONE: &str = "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6a7b8c9d0\n\nM\tpages/foo.md\n\nf0e1d2c3b4a5968778695a4b3c2d1e0f9a8b7c6d5\n\nR100\tpages/old.md\tpages/foo.md\n\n";
/// A second, shorter walk input with a different commit sequence.
const LOG_TWO: &str = "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6a7b8c9d0\n\nM\tpages/foo.md\n\n";

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

/// Run `git` in `dir`, panicking on failure. Test fixtures shell out; the
/// production-side ban does not apply (same idiom as tests/common/mod.rs).
fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(dir)
        .args(args)
        .status()
        .expect("git command");
    assert!(status.success(), "git {:?} failed in {:?}", args, dir);
}

/// A fresh temp dir standing in for a repository's common git dir.
fn temp_common_dir() -> TempDir {
    TempDir::new().expect("tempdir")
}

/// The database path under `common_dir`, with the `wiki/` directory created
/// private (0700 — the mode the production open path enforces).
fn db_path_for(common_dir: &TempDir) -> PathBuf {
    let path = db_path(common_dir.path());
    let dir = path.parent().expect("db path has a parent");
    fs::create_dir_all(dir).expect("create wiki dir");
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).expect("privatize wiki dir");
    path
}

/// Craft a healthy store database at `db_path` — the binding DDL consts
/// executed against a connection, seeded with the binding identity values.
/// This is exactly the state the real open path creates on a fresh file.
fn craft_valid_db(db_path: &Path) {
    let conn = open_connection(db_path).expect("open connection for crafting");
    for ddl in [META_DDL, FINGERPRINT_DDL, ANCHOR_WALK_DDL] {
        conn.execute_batch(ddl).expect("create table");
    }
    conn.execute_batch(&format!(
        "INSERT OR REPLACE INTO meta (id, application_id, schema_version, anchor_epoch, index_epoch) \
         VALUES (1, '{APPLICATION_ID}', {SCHEMA_VERSION}, 0, 0);"
    ))
    .expect("seed meta");
}

/// Open a second connection to `db_path` and run one parameterless statement
/// (crafted-DB mutation for serve-verification checks). The connection drops
/// when this returns, so the store's next lookup sees the committed change.
fn exec_on_db(db_path: &Path, sql: &str) {
    let conn = open_connection(db_path).expect("open connection for mutation");
    conn.execute_batch(sql).expect("execute mutation");
}

/// The database file and its quarantine artifacts in `cache_dir` — excludes
/// the `-wal`/`-shm` companions and the init lock.
fn db_artifacts(cache_dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(cache_dir)
        .expect("read cache dir")
        .map(|entry| entry.expect("dir entry").path())
        .filter(|path| {
            let name = path.file_name().expect("file name").to_string_lossy();
            name.starts_with(DB_FILE_NAME)
                && name != DB_FILE_NAME
                && name != format!("{DB_FILE_NAME}-wal")
                && name != format!("{DB_FILE_NAME}-shm")
        })
        .collect()
}

/// Open the store for a fixture common dir, asserting the init lock is free.
fn open_store(common_dir: &Path) -> CacheStore {
    CacheStore::open(common_dir)
        .expect("open store")
        .expect("init lock is not held in a test fixture")
}

#[test]
fn probe_rejects_malformed_fingerprint_binding_schema_and_open_rebuilds() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = wiki::cache::schema::db_path(dir.path());
    drop(open_store(dir.path()));
    let conn = rusqlite::Connection::open(&db).expect("open db");
    conn.execute_batch(
        "DROP TABLE fingerprint; CREATE TABLE fingerprint (key_digest TEXT PRIMARY KEY) STRICT;",
    )
    .expect("malform fingerprint");
    drop(conn);
    assert!(matches!(
        wiki::cache::schema::probe(&db).unwrap(),
        wiki::cache::schema::ProbeOutcome::Skew(Tier::Anchor)
    ));
    drop(open_store(dir.path()));
    assert_eq!(
        wiki::cache::schema::probe(&db).unwrap(),
        wiki::cache::schema::ProbeOutcome::Valid
    );
}

#[test]
fn probe_rejects_malformed_walk_binding_schema_and_open_rebuilds() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = wiki::cache::schema::db_path(dir.path());
    drop(open_store(dir.path()));
    let conn = rusqlite::Connection::open(&db).expect("open db");
    conn.execute_batch(
        "DROP TABLE anchor_walk; CREATE TABLE anchor_walk (key_digest TEXT PRIMARY KEY) STRICT;",
    )
    .expect("malform walk");
    drop(conn);
    assert!(matches!(
        wiki::cache::schema::probe(&db).unwrap(),
        wiki::cache::schema::ProbeOutcome::Skew(Tier::Anchor)
    ));
    drop(open_store(dir.path()));
    assert_eq!(
        wiki::cache::schema::probe(&db).unwrap(),
        wiki::cache::schema::ProbeOutcome::Valid
    );
}

#[test]
fn page_writes_remain_queued_until_one_explicit_flush() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = open_store(dir.path());
    store.begin_page();
    for line in 1..=8 {
        let key = fingerprint_key("page.md", SHA, "target.md", line, line);
        store
            .upsert_fingerprint(&key, "page.md", SHA, "target.md", line, line, FP)
            .unwrap();
    }
    let db = wiki::cache::schema::db_path(dir.path());
    let observer = rusqlite::Connection::open(&db).expect("observer");
    let before: i64 = observer
        .query_row("SELECT count(*) FROM fingerprint", [], |r| r.get(0))
        .unwrap();
    assert_eq!(before, 0, "page writes must not autocommit per row");
    store.flush_page().unwrap();
    let after: i64 = observer
        .query_row("SELECT count(*) FROM fingerprint", [], |r| r.get(0))
        .unwrap();
    assert_eq!(after, 8, "one page flush publishes the whole batch");
}

#[test]
fn clear_releases_its_connection_before_the_destructive_window() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = open_store(dir.path());
    assert!(store.connection_is_open());
    store.clear().expect("clear");
    assert!(
        !store.connection_is_open(),
        "clear must release SQLite before the destructive window"
    );
}

/// Restores the process cwd on drop. `git::common_dir()` discovers from the
/// current directory, and the suite runs single-threaded (--test-threads=1).
struct CwdGuard(PathBuf);

impl CwdGuard {
    fn new() -> Self {
        CwdGuard(std::env::current_dir().expect("current dir"))
    }
}

impl Drop for CwdGuard {
    fn drop(&mut self) {
        std::env::set_current_dir(&self.0).expect("restore cwd");
    }
}

/// A temp repository with one linked worktree (both kept alive for the test).
struct LinkedWorktreeRepo {
    main: TempDir,
    linked: TempDir,
}

fn linked_worktree_repo() -> LinkedWorktreeRepo {
    let main = TempDir::new().expect("tempdir");
    git(main.path(), &["init", "-b", "main"]);
    git(main.path(), &["config", "user.email", "test@example.com"]);
    git(main.path(), &["config", "user.name", "Test"]);
    fs::write(
        main.path().join("README.md"),
        "anchor cache worktree fixture\n",
    )
    .expect("write README");
    git(main.path(), &["add", "."]);
    git(main.path(), &["commit", "-m", "init"]);
    let linked = TempDir::new().expect("tempdir");
    // `-b` pins the branch name: without it git derives one from the target
    // path's basename, and tempfile dirs are `.tmpXXXX` — an invalid branch
    // name (leading dot), so the worktree would never be created.
    git(
        main.path(),
        &[
            "worktree",
            "add",
            "-b",
            "wt-linked",
            linked.path().to_str().expect("utf8 temp path"),
        ],
    );
    LinkedWorktreeRepo { main, linked }
}

// ---------------------------------------------------------------------------
// (a) Key-derivation vectors
// ---------------------------------------------------------------------------

/// sha256_hex: the two standard NIST vectors.
#[test]
fn sha256_hex_known_answers() {
    assert_eq!(
        sha256_hex(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

/// fingerprint_key pins the byte-exact encoding: each field is its u64
/// little-endian byte length followed by the bytes; `start`/`end` contribute
/// decimal ASCII (still length-tagged). The three range shapes are covered:
/// empty range (0, 0), `start == 0`, whole-file (1, u32::MAX) — plus a
/// hostile path carrying `#`, `L`, a tab, and a newline.
#[test]
fn fingerprint_key_known_answers_pin_the_encoding() {
    assert_eq!(
        fingerprint_key("", "", "", 0, 0),
        "038a8aeb39c05511a94517abc6edddefb6a62abb6aa12e45cdb1dce1394e1fa6"
    );
    assert_eq!(
        fingerprint_key("a", "b", "c", 12, 34),
        "54f363dd9da809d1f68910d3fb9f1fe7eda010440c22e1817a127006b8b291a4"
    );
    assert_eq!(
        fingerprint_key("docs/guide.md", SHA, "docs/other.md", 0, 0),
        "3befbad3e1c4404fccdc8142f9c725e3ef156b719ec1b0090e414d9940b5c682"
    );
    assert_eq!(
        fingerprint_key("docs/guide.md", SHA, "docs/other.md", 0, 42),
        "4141e63ce883e3c991ec7657565ae3f65c2cf7a5e837089173410e6f6c2d3357"
    );
    assert_eq!(
        fingerprint_key("docs/guide.md", SHA, "docs/other.md", 1, u32::MAX),
        "57b45e4fd95ffe19b29aedd7db92c10fb0dd34b266ca12723254631b702c0935"
    );
    assert_eq!(
        fingerprint_key("p#1\nq\tz", "L", "tab\there\nend", 2, 9),
        "dd064c8a0a38692e0fd84f94be4a7096a8b720217671085e9f9ac1943c1afac2"
    );
}

/// fingerprint_key is injective over its range bounds: `(12, 34)` must never
/// collide with `(1, 234)` — the ambiguity that motivated length tagging —
/// and an empty range differs from a one-line range.
#[test]
fn fingerprint_key_distinguishes_range_boundaries() {
    assert_ne!(
        fingerprint_key("", "", "", 12, 34),
        fingerprint_key("", "", "", 1, 234)
    );
    assert_ne!(
        fingerprint_key("a", SHA, "b", 5, 5),
        fingerprint_key("a", SHA, "b", 5, 6)
    );
}

/// fingerprint_key is injective over field boundaries: paths may contain
/// anything (`#`, tabs, newlines, `L`), so a page `a` + target `b#c` must
/// never collide with page `a#b` + target `c`, and the classic
/// concatenation pair `(a, bc)` vs `(ab, c)` must stay apart.
#[test]
fn fingerprint_key_distinguishes_field_boundaries() {
    assert_ne!(
        fingerprint_key("a", SHA, "b#c", 1, 2),
        fingerprint_key("a#b", SHA, "c", 1, 2)
    );
    assert_ne!(
        fingerprint_key("a", "bc", "", 1, 2),
        fingerprint_key("ab", "c", "", 1, 2)
    );
    assert_ne!(
        fingerprint_key("p#1\nq\tz", "L", "tab\there\nend", 2, 9),
        fingerprint_key("p#1", "L", "q\tz\ntab\there\nend", 2, 9)
    );
}

/// walk_key pins the encoding over newline-containing log output, including a
/// realistic `git log --follow --name-status --format=%H` sample passed
/// through untrimmed.
#[test]
fn walk_key_known_answers_pin_the_encoding() {
    assert_eq!(
        walk_key("a", "b"),
        "cf6ab613e3942391f88ed698557e1680f160bd10e88c6b668c50360c10930e2b"
    );
    assert_eq!(
        walk_key("a", "x\ny"),
        "d7c5c8aa58d1656433e976c01122d09a68fbc8a9fadda34bcf952abfddc0b9a6"
    );
    assert_eq!(
        walk_key("pages/foo.md", LOG_ONE),
        "5a72424f89531635646f12b962dd388c002f75a5b49bf4c40952bab6b80e3113"
    );
}

/// walk_key is injective over its two fields: a newline inside a field vs a
/// newline between fields, an empty page vs an empty log, and swapped fields.
#[test]
fn walk_key_distinguishes_field_boundaries() {
    assert_ne!(walk_key("a", "x\ny"), walk_key("a\nx", "y"));
    assert_ne!(walk_key("a", ""), walk_key("", "a"));
    assert_ne!(walk_key("a", "b"), walk_key("b", "a"));
}

/// Every digest the cache keys on is 64 lowercase hex characters.
#[test]
fn key_digests_are_64_lowercase_hex() {
    for digest in [
        fingerprint_key("", "", "", 0, 0),
        fingerprint_key("p\n#", "L", "t", 1, 2),
        walk_key("", ""),
        sha256_hex(b""),
    ] {
        assert_eq!(digest.len(), 64);
        assert!(
            digest
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
            "digest must be 64 lowercase hex: {digest}"
        );
    }
}

// ---------------------------------------------------------------------------
// (b) Probe classification
// ---------------------------------------------------------------------------

/// A missing file is [`ProbeOutcome::Missing`] — a fresh create, not a
/// quarantine — and the probe itself must never create the file.
#[test]
fn probe_missing_file_is_missing() {
    let dir = temp_common_dir();
    let db = db_path(dir.path());
    assert_eq!(probe(&db).expect("probe"), ProbeOutcome::Missing);
    assert!(!db.exists(), "a probe must never create the file");
}

/// A database crafted with the binding DDL consts and meta seed probes Valid.
#[test]
fn probe_classifies_a_healthy_db_as_valid() {
    let dir = temp_common_dir();
    let db = db_path_for(&dir);
    craft_valid_db(&db);
    assert_eq!(probe(&db).expect("probe"), ProbeOutcome::Valid);
}

/// A file that is not a database at all probes NotADatabase.
#[test]
fn probe_classifies_garbage_as_not_a_database() {
    let dir = temp_common_dir();
    let db = db_path_for(&dir);
    fs::write(&db, b"this is not a sqlite database file - plain garbage").expect("write garbage");
    assert_eq!(
        probe(&db).expect("probe"),
        ProbeOutcome::Suspect(SuspectKind::NotADatabase)
    );
}

/// A truncated database (header intact, pages cut short) probes Corrupt.
#[test]
fn probe_classifies_a_truncated_db_as_corrupt() {
    let dir = temp_common_dir();
    let db = db_path_for(&dir);
    craft_valid_db(&db);
    // Fold the WAL into the main file so the truncation lands in real pages.
    exec_on_db(&db, "PRAGMA wal_checkpoint(TRUNCATE);");
    let len = fs::metadata(&db).expect("metadata").len();
    assert!(
        len > 512,
        "crafted db must be larger than the 100-byte header"
    );
    let file = fs::File::options()
        .write(true)
        .open(&db)
        .expect("open for truncate");
    file.set_len(len / 2).expect("truncate");
    assert_eq!(
        probe(&db).expect("probe"),
        ProbeOutcome::Suspect(SuspectKind::Corrupt)
    );
}

/// A database whose `application_id` belongs to a different tool probes
/// MetaMismatch — a foreign file must never be adopted.
#[test]
fn probe_classifies_a_foreign_application_id_as_meta_mismatch() {
    let dir = temp_common_dir();
    let db = db_path_for(&dir);
    craft_valid_db(&db);
    exec_on_db(&db, "UPDATE meta SET application_id = 'foreign-tool';");
    assert_eq!(
        probe(&db).expect("probe"),
        ProbeOutcome::Suspect(SuspectKind::MetaMismatch)
    );
}

/// A database with no meta row probes MetaMismatch.
#[test]
fn probe_classifies_a_missing_meta_row_as_meta_mismatch() {
    let dir = temp_common_dir();
    let db = db_path_for(&dir);
    craft_valid_db(&db);
    exec_on_db(&db, "DELETE FROM meta;");
    assert_eq!(
        probe(&db).expect("probe"),
        ProbeOutcome::Suspect(SuspectKind::MetaMismatch)
    );
}

// ---------------------------------------------------------------------------
// (b) Quarantine
// ---------------------------------------------------------------------------

/// Quarantine renames the suspect file aside exactly once with its content
/// preserved, deletes the `-wal`/`-shm` companions, and leaves a fresh,
/// fully usable cache at the original path.
#[test]
fn quarantine_renames_aside_deletes_sidecars_and_creates_fresh() {
    let dir = temp_common_dir();
    let db = db_path_for(&dir);
    let cache = db.parent().expect("wiki dir");
    let garbage = b"not a database - quarantine target".to_vec();
    fs::write(&db, &garbage).expect("write suspect db");
    fs::write(cache.join(format!("{DB_FILE_NAME}-wal")), b"old-wal").expect("write wal sidecar");
    fs::write(cache.join(format!("{DB_FILE_NAME}-shm")), b"old-shm").expect("write shm sidecar");

    quarantine(&db).expect("quarantine");

    // The suspect file is renamed aside exactly once, content preserved.
    let aside = db_artifacts(cache);
    assert_eq!(aside.len(), 1, "exactly one quarantine rename-aside");
    assert_eq!(fs::read(&aside[0]).expect("read aside"), garbage);

    // The original -wal/-shm companions are gone — nothing in the cache dir
    // still carries their content.
    for entry in fs::read_dir(cache).expect("read cache dir") {
        let path = entry.expect("entry").path();
        if path.is_file() {
            assert_ne!(
                fs::read(&path).expect("read entry"),
                b"old-wal".to_vec(),
                "-wal companion must be deleted, not kept"
            );
            assert_ne!(
                fs::read(&path).expect("read entry"),
                b"old-shm".to_vec(),
                "-shm companion must be deleted, not kept"
            );
        }
    }

    // A fresh, fully seeded cache sits at the original path.
    assert_eq!(probe(&db).expect("fresh file probes"), ProbeOutcome::Valid);
}

/// The TOCTOU re-probe: between the open path's probe and the quarantine's
/// lock acquisition another process may have recreated the file — a valid
/// file must never be renamed aside (plan decision 4).
#[test]
fn quarantine_leaves_a_valid_db_untouched() {
    let dir = temp_common_dir();
    let db = db_path_for(&dir);
    craft_valid_db(&db);
    quarantine(&db).expect("quarantine");
    assert_eq!(probe(&db).expect("probe"), ProbeOutcome::Valid);
    assert!(
        db_artifacts(db.parent().expect("wiki dir")).is_empty(),
        "no rename-aside for a valid file"
    );
}

// ---------------------------------------------------------------------------
// (b) Open ordering
// ---------------------------------------------------------------------------

/// The binding open order's observable contract: after open, the connection
/// runs in WAL mode with the 1000 ms busy timeout armed (busy_timeout set
/// *before* the WAL pragma — git-span's ordering invariant), and the freshly
/// created file is a fully seeded cache.
#[test]
fn open_connection_applies_busy_timeout_and_wal() {
    let dir = temp_common_dir();
    let db = db_path_for(&dir);
    let conn = open_connection(&db).expect("open");
    let journal_mode: String = conn
        .query_row("PRAGMA journal_mode;", [], |row| row.get(0))
        .expect("journal mode");
    assert_eq!(journal_mode, "wal");
    let busy_timeout: i64 = conn
        .query_row("PRAGMA busy_timeout;", [], |row| row.get(0))
        .expect("busy timeout");
    assert_eq!(busy_timeout, BUSY_TIMEOUT_MS as i64);
    assert_eq!(probe(&db).expect("probe"), ProbeOutcome::Valid);
}

// ---------------------------------------------------------------------------
// (b) Store roundtrips — fingerprint tier
// ---------------------------------------------------------------------------

/// Upsert then lookup serves the stored fingerprint.
#[test]
fn store_fingerprint_upsert_then_lookup_serves() {
    let dir = temp_common_dir();
    let store = open_store(dir.path());
    let key = fingerprint_key("pages/guide.md", SHA, "pages/other.md", 10, 20);
    store
        .upsert_fingerprint(&key, "pages/guide.md", SHA, "pages/other.md", 10, 20, FP)
        .expect("upsert");
    let served = store
        .lookup_fingerprint(&key, "pages/guide.md", SHA, "pages/other.md", 10, 20)
        .expect("lookup");
    assert_eq!(served.as_deref(), Some(FP));
}

/// A different queried tuple is a miss — even the same key queried with a
/// different range, because the stored tuple is compared field-by-field on
/// serve (plan decision 5).
#[test]
fn store_fingerprint_misses_on_a_different_queried_tuple() {
    let dir = temp_common_dir();
    let store = open_store(dir.path());
    let key = fingerprint_key("pages/guide.md", SHA, "pages/other.md", 10, 20);
    store
        .upsert_fingerprint(&key, "pages/guide.md", SHA, "pages/other.md", 10, 20, FP)
        .expect("upsert");
    assert_eq!(
        store
            .lookup_fingerprint(&key, "pages/guide.md", SHA, "pages/other.md", 10, 21)
            .expect("lookup"),
        None,
        "same key, different queried range: serve verification must miss"
    );
    let other_key = fingerprint_key("pages/guide.md", SHA, "pages/other.md", 10, 21);
    assert_eq!(
        store
            .lookup_fingerprint(&other_key, "pages/guide.md", SHA, "pages/other.md", 10, 21)
            .expect("lookup"),
        None
    );
}

/// Overwriting the same key is last-write-wins.
#[test]
fn store_fingerprint_overwrite_is_last_write_wins() {
    let dir = temp_common_dir();
    let store = open_store(dir.path());
    let key = fingerprint_key("pages/guide.md", SHA, "pages/other.md", 10, 20);
    store
        .upsert_fingerprint(
            &key,
            "pages/guide.md",
            SHA,
            "pages/other.md",
            10,
            20,
            "0000000000000000",
        )
        .expect("upsert first");
    store
        .upsert_fingerprint(&key, "pages/guide.md", SHA, "pages/other.md", 10, 20, FP)
        .expect("upsert second");
    let served = store
        .lookup_fingerprint(&key, "pages/guide.md", SHA, "pages/other.md", 10, 20)
        .expect("lookup");
    assert_eq!(served.as_deref(), Some(FP), "the later write wins");
}

/// A tampered `fp` column is a miss — the row_digest no longer verifies.
#[test]
fn store_fingerprint_tampered_fp_is_a_miss() {
    let dir = temp_common_dir();
    let store = open_store(dir.path());
    let key = fingerprint_key("pages/guide.md", SHA, "pages/other.md", 10, 20);
    store
        .upsert_fingerprint(&key, "pages/guide.md", SHA, "pages/other.md", 10, 20, FP)
        .expect("upsert");
    exec_on_db(
        &db_path(dir.path()),
        &format!("UPDATE fingerprint SET fp = 'deadbeefdeadbeef' WHERE key_digest = '{key}';"),
    );
    assert_eq!(
        store
            .lookup_fingerprint(&key, "pages/guide.md", SHA, "pages/other.md", 10, 20)
            .expect("lookup"),
        None,
        "a tampered fp must never be served"
    );
}

/// A tampered stored tuple is a miss both ways: queried with the original
/// tuple (stored tuple mismatch) and queried with the tampered tuple
/// (row_digest covers the tuple too).
#[test]
fn store_fingerprint_tampered_tuple_is_a_miss() {
    let dir = temp_common_dir();
    let store = open_store(dir.path());
    let key = fingerprint_key("pages/guide.md", SHA, "pages/other.md", 10, 20);
    store
        .upsert_fingerprint(&key, "pages/guide.md", SHA, "pages/other.md", 10, 20, FP)
        .expect("upsert");
    exec_on_db(
        &db_path(dir.path()),
        &format!("UPDATE fingerprint SET range_start = 999 WHERE key_digest = '{key}';"),
    );
    assert_eq!(
        store
            .lookup_fingerprint(&key, "pages/guide.md", SHA, "pages/other.md", 10, 20)
            .expect("lookup"),
        None,
        "stored tuple mismatch is a miss"
    );
    assert_eq!(
        store
            .lookup_fingerprint(&key, "pages/guide.md", SHA, "pages/other.md", 999, 20)
            .expect("lookup"),
        None,
        "row_digest covers the tuple — a tampered tuple never serves its own lie"
    );
}

/// A tampered row_digest is a miss.
#[test]
fn store_fingerprint_tampered_row_digest_is_a_miss() {
    let dir = temp_common_dir();
    let store = open_store(dir.path());
    let key = fingerprint_key("pages/guide.md", SHA, "pages/other.md", 10, 20);
    store
        .upsert_fingerprint(&key, "pages/guide.md", SHA, "pages/other.md", 10, 20, FP)
        .expect("upsert");
    exec_on_db(
        &db_path(dir.path()),
        &format!("UPDATE fingerprint SET row_digest = randomblob(32) WHERE key_digest = '{key}';"),
    );
    assert_eq!(
        store
            .lookup_fingerprint(&key, "pages/guide.md", SHA, "pages/other.md", 10, 20)
            .expect("lookup"),
        None,
        "a tampered row_digest must never be served"
    );
}

// ---------------------------------------------------------------------------
// (b) Store roundtrips — anchor-walk tier
// ---------------------------------------------------------------------------

/// Upsert then lookup serves the stored walk epoch.
#[test]
fn store_walk_upsert_then_lookup_serves() {
    let dir = temp_common_dir();
    let store = open_store(dir.path());
    let key = walk_key("pages/foo.md", LOG_ONE);
    let log_sha = sha256_hex(LOG_ONE.as_bytes());
    store
        .upsert_walk(
            &key,
            "pages/foo.md",
            &log_sha,
            SHA,
            "pages/foo.md",
            Some("3"),
        )
        .expect("upsert");
    let served = store
        .lookup_walk(&key, "pages/foo.md", LOG_ONE)
        .expect("lookup")
        .expect("served");
    assert_eq!(
        served,
        WalkRow {
            anchor_sha: SHA.to_string(),
            path_at_commit: "pages/foo.md".to_string(),
            value: Some("3".to_string()),
        }
    );
}

/// A field-less anchor (`None`) is a legitimate cached state, distinct from
/// an empty value.
#[test]
fn store_walk_none_value_is_distinct_from_empty() {
    let dir = temp_common_dir();
    let store = open_store(dir.path());
    let key_none = walk_key("pages/foo.md", LOG_ONE);
    let key_empty = walk_key("pages/foo.md", LOG_TWO);
    store
        .upsert_walk(
            &key_none,
            "pages/foo.md",
            &sha256_hex(LOG_ONE.as_bytes()),
            SHA,
            "pages/foo.md",
            None,
        )
        .expect("upsert None");
    store
        .upsert_walk(
            &key_empty,
            "pages/foo.md",
            &sha256_hex(LOG_TWO.as_bytes()),
            SHA,
            "pages/foo.md",
            Some(""),
        )
        .expect("upsert empty");
    assert_eq!(
        store
            .lookup_walk(&key_none, "pages/foo.md", LOG_ONE)
            .expect("lookup")
            .expect("served"),
        WalkRow {
            anchor_sha: SHA.to_string(),
            path_at_commit: "pages/foo.md".to_string(),
            value: None,
        }
    );
    assert_eq!(
        store
            .lookup_walk(&key_empty, "pages/foo.md", LOG_TWO)
            .expect("lookup")
            .expect("served"),
        WalkRow {
            anchor_sha: SHA.to_string(),
            path_at_commit: "pages/foo.md".to_string(),
            value: Some(String::new()),
        }
    );
}

/// A different queried tuple is a miss: a different log output (different
/// key), the same key with a different log_output argument (the stored
/// `log_output_sha` must match sha256 of the queried log), or a different
/// page path.
#[test]
fn store_walk_misses_on_a_different_queried_tuple() {
    let dir = temp_common_dir();
    let store = open_store(dir.path());
    let key = walk_key("pages/foo.md", LOG_ONE);
    store
        .upsert_walk(
            &key,
            "pages/foo.md",
            &sha256_hex(LOG_ONE.as_bytes()),
            SHA,
            "pages/foo.md",
            Some("3"),
        )
        .expect("upsert");
    assert_eq!(
        store
            .lookup_walk(&key, "pages/foo.md", LOG_TWO)
            .expect("lookup"),
        None,
        "same key, different log output: serve verification must miss"
    );
    assert_eq!(
        store
            .lookup_walk(&key, "pages/other.md", LOG_ONE)
            .expect("lookup"),
        None,
        "different page path is a miss"
    );
    let other_key = walk_key("pages/foo.md", LOG_TWO);
    assert_eq!(
        store
            .lookup_walk(&other_key, "pages/foo.md", LOG_TWO)
            .expect("lookup"),
        None,
        "different log output is a different key"
    );
}

/// A tampered `log_output_sha` is a miss.
#[test]
fn store_walk_tampered_log_output_sha_is_a_miss() {
    let dir = temp_common_dir();
    let store = open_store(dir.path());
    let key = walk_key("pages/foo.md", LOG_ONE);
    store
        .upsert_walk(
            &key,
            "pages/foo.md",
            &sha256_hex(LOG_ONE.as_bytes()),
            SHA,
            "pages/foo.md",
            Some("3"),
        )
        .expect("upsert");
    exec_on_db(
        &db_path(dir.path()),
        &format!(
            "UPDATE anchor_walk SET log_output_sha = 'deadbeefdeadbeef' WHERE key_digest = '{key}';"
        ),
    );
    assert_eq!(
        store
            .lookup_walk(&key, "pages/foo.md", LOG_ONE)
            .expect("lookup"),
        None,
        "a tampered log_output_sha must never be served"
    );
}

/// A tampered row_digest is a miss.
#[test]
fn store_walk_tampered_row_digest_is_a_miss() {
    let dir = temp_common_dir();
    let store = open_store(dir.path());
    let key = walk_key("pages/foo.md", LOG_ONE);
    store
        .upsert_walk(
            &key,
            "pages/foo.md",
            &sha256_hex(LOG_ONE.as_bytes()),
            SHA,
            "pages/foo.md",
            Some("3"),
        )
        .expect("upsert");
    exec_on_db(
        &db_path(dir.path()),
        &format!("UPDATE anchor_walk SET row_digest = randomblob(32) WHERE key_digest = '{key}';"),
    );
    assert_eq!(
        store
            .lookup_walk(&key, "pages/foo.md", LOG_ONE)
            .expect("lookup"),
        None,
        "a tampered row_digest must never be served"
    );
}

// ---------------------------------------------------------------------------
// (b) Clear
// ---------------------------------------------------------------------------

/// clear() is tier-scoped (plan D10): both tiers' static tables plus every
/// dynamic `fts_%` child die inside one transaction under the exclusive
/// rendezvous lock and the init lock, stale quarantine asides go, and
/// everything else survives — the directory itself (it holds journals,
/// rendezvous state, and the log), both lock files, the meta singleton,
/// and the cross-tier `store_events` ledger. The cleared store probes Valid
/// immediately, and a second clear is a best-effort success.
#[test]
fn clear_empties_both_tiers_and_preserves_the_directory() {
    let dir = temp_common_dir();
    let store = open_store(dir.path());
    // Anchor-tier data.
    let key = fingerprint_key("pages/guide.md", SHA, "pages/other.md", 10, 20);
    store
        .upsert_fingerprint(&key, "pages/guide.md", SHA, "pages/other.md", 10, 20, FP)
        .expect("upsert");
    // Index-tier data: one static row and one dynamic `fts_<gen_id>` child,
    // shaped as generations.rs produces them.
    let db = db_path(dir.path());
    exec_on_db(
        &db,
        "INSERT INTO generations (gen_id, digest, head_oid, head_tree_oid, index_checksum,
             wikiignore_hash, worktree_sig, publisher, created_at, access_bucket, blob_count)
         VALUES (1, x'0000000000000000000000000000000000000000000000000000000000000000',
             'a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6a7b8c9d0', '',
             x'0000000000000000000000000000000000000000',
             x'0000000000000000000000000000000000000000',
             x'0000000000000000000000000000000000000000000000000000000000000000',
             NULL, 0, 0, 0);",
    );
    exec_on_db(&db, "CREATE VIRTUAL TABLE fts_1 USING fts5(body);");
    // A diagnostic event recorded before the clear must survive it — the
    // ledger is cross-tier infrastructure, never dropped by tier-scoped work.
    {
        let observer = rusqlite::Connection::open(&db).expect("observer");
        wiki::cache::diagnostics::record(&observer, "quarantine_performed");
    }
    // A quarantine rename-aside (as schema::quarantine produces it) goes.
    let cache = db.parent().expect("db path has a parent").to_path_buf();
    fs::write(
        cache.join(format!("{DB_FILE_NAME}.1234567890.quarantine")),
        b"aside",
    )
    .expect("write aside");
    let lock = init_lock_path(dir.path());
    assert!(lock.exists(), "the open path leaves the init lock behind");

    store.clear().expect("clear");
    assert!(cache.exists(), "the directory itself is preserved");
    assert!(lock.exists(), "the init lock file is preserved");

    // Tier tables exist again (empty shapes) with zero rows; the dynamic
    // fts child is gone entirely; meta and the ledger survive.
    assert_eq!(probe(&db).expect("probe"), ProbeOutcome::Valid);
    let conn = rusqlite::Connection::open(&db).expect("reopen cleared store");
    for table in [
        "fingerprint",
        "anchor_walk",
        "generations",
        "gen_paths",
        "blobs",
    ] {
        let rows: i64 = conn
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap_or_else(|e| panic!("cleared tier table {table} must serve: {e}"));
        assert_eq!(rows, 0, "{table} must be empty after a clear");
    }
    let fts_children: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name LIKE 'fts\\_%' ESCAPE '\\'",
            [],
            |r| r.get(0),
        )
        .expect("count fts children");
    assert_eq!(fts_children, 0, "every dynamic fts_% child must be dropped");
    let meta_rows: i64 = conn
        .query_row("SELECT count(*) FROM meta", [], |r| r.get(0))
        .expect("count meta");
    assert_eq!(meta_rows, 1, "the meta singleton survives a clear");
    let events: i64 = conn
        .query_row("SELECT count(*) FROM store_events", [], |r| r.get(0))
        .expect("count store_events");
    assert_eq!(events, 1, "the store_events ledger survives a clear");
    drop(conn);

    // Best-effort idempotence: clearing an already-empty store still succeeds.
    store.clear().expect("clear again");
    assert_eq!(probe(&db).expect("probe"), ProbeOutcome::Valid);
}

// ---------------------------------------------------------------------------
// (b) Open — quarantine rebuilt notice
// ---------------------------------------------------------------------------

/// `CacheStore::open` on a suspect database quarantines and rebuilds it,
/// emitting exactly one notice on stderr: `warning: anchor cache was
/// corrupt; rebuilt` — plain text, no JSON. A subsequent healthy open (the
/// rebuilt cache probes Valid) emits nothing, and a missing file (fresh
/// create, not corruption) emits nothing. The notice is process-stderr
/// output, so this check spawns itself — the test binary — with `--exact`
/// and `--nocapture` to capture the child's stderr.
#[test]
fn open_quarantine_emits_rebuilt_warning_exactly_once() {
    let exe = std::env::current_exe().expect("test binary path");
    let out = Command::new(&exe)
        .args([
            "--exact",
            "open_quarantine_rebuilt_warning_helper",
            "--nocapture",
        ])
        .output()
        .expect("run helper in a child process");
    assert!(
        out.status.success(),
        "helper failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        stderr
            .matches("warning: anchor cache was corrupt; rebuilt")
            .count(),
        1,
        "exactly one rebuilt notice across a corrupt open, a healthy reopen, \
         and a fresh create; got:\n{stderr}"
    );
}

/// Child-process fixture for the notice check: one process, three opens —
/// the first quarantines the garbage db (one notice), the second probes the
/// rebuilt cache Valid (no second notice — once-per-run holds without
/// suppression), and a missing file is a fresh create (no notice).
#[test]
fn open_quarantine_rebuilt_warning_helper() {
    let dir = temp_common_dir();
    let db = db_path_for(&dir);
    fs::write(&db, b"this is not a sqlite database file - plain garbage").expect("write garbage");
    drop(open_store(dir.path()));
    drop(open_store(dir.path()));
    let fresh = temp_common_dir();
    drop(open_store(fresh.path()));
}

// ---------------------------------------------------------------------------
// (c) Linked worktrees share one common dir
// ---------------------------------------------------------------------------

/// `git::common_dir()` resolves the same normalized path from the main
/// worktree and a linked worktree of one repository — the anchor cache is
/// shared across worktrees (plan decision 2), and the path is the real
/// common git dir (`.git` of the main repository).
#[test]
fn common_dir_is_identical_from_every_linked_worktree() {
    let repo = linked_worktree_repo();
    let _guard = CwdGuard::new();

    std::env::set_current_dir(repo.main.path()).expect("cd main worktree");
    let from_main = wiki::git::common_dir().expect("discover from the main worktree");

    std::env::set_current_dir(repo.linked.path()).expect("cd linked worktree");
    let from_linked = wiki::git::common_dir().expect("discover from a linked worktree");

    assert!(
        !from_main.as_os_str().is_empty(),
        "common_dir must resolve a real path"
    );
    assert_eq!(
        from_main, from_linked,
        "every worktree of one repository resolves the same common dir — one shared cache"
    );
    let expected = fs::canonicalize(repo.main.path().join(".git")).expect("canonicalize main .git");
    assert_eq!(
        fs::canonicalize(&from_main).expect("canonicalize common dir"),
        expected
    );
}
