//! When a tree rename lands on a destination path whose `paths` row
//! already exists (from prior-state skew such as dangling rows left by
//! other defects, or case-colliding tree entries), `apply_rename`'s
//! `INSERT OR REPLACE` silently deletes the displaced row without calling
//! `decrement_blob`. The displaced blob's refcount is overcounted and its
//! `blobs`+FTS rows leak forever.
//!
//! This test artificially creates the skew by directly inserting a
//! dangling `paths` row into the index database, then triggers a pure
//! rename that lands on the occupied destination. After the refresh the
//! displaced blob row survives with a positive refcount and zero `paths`
//! rows — the leaked-ghost-blob signature.

mod common;

use std::path::Path;

use rusqlite::Connection;
use wiki::index::WikiIndex;
use wiki::index::blob::compute_blob_oid;

/// Open the index database for direct manipulation.
fn open_index_db(repo_root: &Path) -> Connection {
    let db_path = repo_root.join(".wiki").join("wiki-index.sqlite");
    Connection::open(&db_path).expect("open index db")
}

/// A blob that is recognisably fake — its content differs from any real
/// wiki page in the test so its OID is distinct.
const FAKE_BYTES: &[u8] = b"---\ntitle: Ghost\nsummary: Should not survive.\n---\n\nGhost body.\n";

#[test]
fn rename_onto_occupied_destination_decrements_displaced_blob() {
    let repo = common::FixtureRepo::new();

    // ── Phase 1: create and then delete `dest.md` so the index is clean ──
    let dest_bytes = "---\ntitle: Destination\nsummary: Original occupant.\n---\n\nDest body.\n";
    let dest_oid = compute_blob_oid(dest_bytes.as_bytes());

    repo.write_file("dest.md", dest_bytes);
    repo.git_add("dest.md");
    repo.git_commit("add dest.md");
    drop(WikiIndex::prepare(repo.root.as_path()).expect("prepare dest"));

    // Verify initial state: dest OID has 1 blobs row, 3 paths rows.
    {
        let index = WikiIndex::prepare(repo.root.as_path()).expect("prepare check dest");
        let (blobs, paths) = index
            .debug_blob_path_counts(&dest_oid.0)
            .expect("debug_blob_path_counts dest");
        assert_eq!((blobs, paths), (1, 3), "dest.md indexed: 1 blob, 3 paths");
    }

    // Delete dest.md so the tree no longer contains it.
    repo.git_rm("dest.md");
    repo.git_commit("remove dest.md");
    drop(WikiIndex::prepare(repo.root.as_path()).expect("prepare after remove"));

    // Confirm dest.md is fully cleaned up.
    {
        let index = WikiIndex::prepare(repo.root.as_path()).expect("prepare check clean");
        let (blobs, paths) = index
            .debug_blob_path_counts(&dest_oid.0)
            .expect("debug_blob_path_counts after remove");
        assert_eq!((blobs, paths), (0, 0), "dest.md fully removed");
    }

    // ── Phase 2: inject prior-state skew ──
    let fake_oid = compute_blob_oid(FAKE_BYTES);

    {
        let conn = open_index_db(repo.root.as_path());
        // Insert a blobs row for the fake OID with refcount=1 so it looks
        // like a legitimate blob that was left behind by a prior defect.
        conn.execute(
            "INSERT INTO blobs (oid, refcount, title, summary, body, aliases_text, tags_text, keywords_text)
             VALUES (?1, 1, ?2, ?3, ?4, '', '', '')",
            rusqlite::params![fake_oid.0, "Ghost", "Should not survive.", "Ghost body."],
        )
        .expect("insert fake blobs row");

        // Insert a dangling paths row at (dest.md, Tree) referencing the
        // fake OID — simulating the FK-brick skew where a Tree row was
        // left behind after its blob was already "deleted."
        conn.execute(
            "INSERT INTO paths (path_rel, source, oid, parent_dir)
             VALUES (?1, 0, ?2, '')",
            rusqlite::params!["dest.md", fake_oid.0],
        )
        .expect("insert fake paths row");
    }

    // Verify the skew is in place.
    {
        let conn = open_index_db(repo.root.as_path());
        let (blob_count,): (i64,) = conn
            .query_row(
                "SELECT COUNT(*) FROM blobs WHERE oid = ?1",
                rusqlite::params![fake_oid.0],
                |r| Ok((r.get(0)?,)),
            )
            .expect("count fake blobs");
        assert_eq!(blob_count, 1, "fake blob row injected");

        let (path_count,): (i64,) = conn
            .query_row(
                "SELECT COUNT(*) FROM paths WHERE oid = ?1",
                rusqlite::params![fake_oid.0],
                |r| Ok((r.get(0)?,)),
            )
            .expect("count fake paths");
        assert_eq!(path_count, 1, "fake paths row injected");
    }

    // ── Phase 3: create source.md and rename it onto the occupied dest.md ──
    let source_bytes =
        "---\ntitle: Source\nsummary: Will be renamed.\n---\n\nSource body.\n";
    let source_oid = compute_blob_oid(source_bytes.as_bytes());

    repo.write_file("source.md", source_bytes);
    repo.git_add("source.md");
    repo.git_commit("add source.md");
    drop(WikiIndex::prepare(repo.root.as_path()).expect("prepare source"));

    // Rename source.md -> dest.md, keeping content identical so the tree
    // diff produces a pure Rename delta (same OID, no retokenization).
    repo.git_mv("source.md", "dest.md");
    repo.git_commit("rename source.md -> dest.md");

    let index = WikiIndex::prepare(repo.root.as_path()).expect("prepare after rename");

    // ── Phase 4: assert correctness ──

    // The displaced fake blob MUST be fully released: its lone paths row
    // was clobbered by the rename, so its refcount should have been
    // decremented from 1 → 0, deleting the blobs row entirely.
    let (fake_blobs, fake_paths) = index
        .debug_blob_path_counts(&fake_oid.0)
        .expect("debug_blob_path_counts fake oid");
    assert_eq!(
        (fake_blobs, fake_paths),
        (0, 0),
        "displaced blob must be released when rename clobbers its paths row; \
         got ({fake_blobs}, {fake_paths}) — leaked ghost blob"
    );

    // The source blob (now at dest.md) must be intact: 1 blobs row, 3
    // paths rows (Tree rename swap + Index Add + Worktree Add).
    let (src_blobs, src_paths) = index
        .debug_blob_path_counts(&source_oid.0)
        .expect("debug_blob_path_counts source oid");
    assert_eq!(src_blobs, 1, "exactly one blobs row for the renamed content");
    assert_eq!(src_paths, 3, "Tree + Index + Worktree paths rows for dest.md");

    // The rename must not re-tokenize: same OID, same content.
    let stats = index.stats();
    assert_eq!(
        stats.fts_retokenizations, 0,
        "pure rename must not retokenize"
    );

    // The page must resolve at its new location.
    let page = index
        .resolve_page("Source")
        .expect("resolve_page")
        .expect("page must survive the rename");
    assert!(
        page.file.ends_with("dest.md"),
        "resolves to dest.md: {}",
        page.file
    );
}
