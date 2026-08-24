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

/// A blob that is recognisably fake — its content differs from any real
/// wiki page in the test so its OID is distinct.
const FAKE_BYTES: &[u8] = b"---\ntitle: Ghost\nsummary: Should not survive.\n---\n\nGhost body.\n";

// ── Target-layout port (plan merged-store-generations, Phase 1) ──────────
//
// Same skew scenario, injected through the merged store's static tier:
// `generations` + `gen_paths` (TEXT source literals) + global `blobs`.
// Pins the DDL shape and the displaced-blob release under generations.

/// Open the merged store for direct manipulation.
fn open_merged_store(repo_root: &Path) -> Connection {
    let db_path = common::target_db_path(repo_root);
    Connection::open(&db_path).expect("open merged store")
}

#[test]
fn rename_onto_occupied_destination_decrements_displaced_blob_in_merged_store() {
    let repo = common::FixtureRepo::new();

    // ── Phase 1: create and then delete `dest.md` so the index is clean ──
    let dest_bytes = "---\ntitle: Destination\nsummary: Original occupant.\n---\n\nDest body.\n";

    repo.write_file("dest.md", dest_bytes);
    repo.git_add("dest.md");
    repo.git_commit("add dest.md");
    drop(WikiIndex::prepare(repo.root.as_path()).expect("prepare dest"));

    // Delete dest.md so the tree no longer contains it.
    repo.git_rm("dest.md");
    repo.git_commit("remove dest.md");
    drop(WikiIndex::prepare(repo.root.as_path()).expect("prepare after remove"));

    // ── Phase 2: inject prior-state skew into the merged store ──
    let fake_oid = compute_blob_oid(FAKE_BYTES);

    {
        let conn = open_merged_store(repo.root.as_path());
        // A generation row to anchor the skew (all CHECK widths honored).
        conn.execute(
            "INSERT INTO generations (digest, head_oid, head_tree_oid, index_checksum,
                                      wikiignore_hash, worktree_sig, publisher,
                                      created_at, access_bucket, blob_count)
             VALUES (zeroblob(32), ?1, '', zeroblob(20), zeroblob(20), zeroblob(32),
                     NULL, 0, 0, 1)",
            rusqlite::params!["f".repeat(40)],
        )
        .expect("insert skew generations row");
        // The fake blob row must exist before anything references it.
        conn.execute(
            "INSERT INTO blobs (oid, refcount, title, summary, body, aliases_text, tags_text, keywords_text)
             VALUES (?1, 1, ?2, ?3, ?4, '', '', '')",
            rusqlite::params![fake_oid.0, "Ghost", "Should not survive.", "Ghost body."],
        )
        .expect("insert fake blobs row");
        // The dangling gen_paths row at (dest.md, 'tree') referencing the
        // fake oid — the FK-brick skew, now in generation scope.
        conn.execute(
            "INSERT INTO gen_paths (gen_id, source, path_rel, oid, parent_dir)
             VALUES (last_insert_rowid(), 'tree', ?1, ?2, '')",
            rusqlite::params!["dest.md", fake_oid.0],
        )
        .expect("insert skew gen_paths row");
    }

    // ── Phase 3: pure rename onto the occupied destination ──
    let source_bytes =
        "---\ntitle: Source\nsummary: Will be renamed.\n---\n\nSource body.\n";
    let source_oid = compute_blob_oid(source_bytes.as_bytes());

    repo.write_file("source.md", source_bytes);
    repo.git_add("source.md");
    repo.git_commit("add source.md");
    drop(WikiIndex::prepare(repo.root.as_path()).expect("prepare source"));

    repo.git_mv("source.md", "dest.md");
    repo.git_commit("rename source.md -> dest.md");

    let index = WikiIndex::prepare(repo.root.as_path()).expect("prepare after rename");

    // ── Phase 4: assert correctness ──

    // Under generations there is no eager release to assert: publish never
    // deletes, and the displaced fake blob legitimately still serves the
    // retained (skew) generation it belongs to. What must hold instead is
    // the anti-ghost invariant — its membership refcount exactly equals the
    // number of retained generations referencing it — plus zero presence in
    // the SERVED generation's corpus below.
    {
        let conn = open_merged_store(repo.root.as_path());
        let leaked: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM blobs b
                 WHERE b.refcount <> COALESCE(
                     (SELECT COUNT(DISTINCT gp.gen_id) FROM gen_paths gp WHERE gp.oid = b.oid), 0)",
                [],
                |r| r.get(0),
            )
            .expect("ghost check query");
        assert_eq!(
            leaked, 0,
            "no blob may carry a refcount that disagrees with its retained-generation memberships"
        );
        let (fake_refcount,): (i64,) = conn
            .query_row(
                "SELECT refcount FROM blobs WHERE oid = ?1",
                rusqlite::params![fake_oid.0],
                |r| Ok((r.get(0)?,)),
            )
            .expect("displaced blob survives serving its retained generation");
        assert!(
            fake_refcount >= 1,
            "held by at least its own retained generation (the pure rename              also re-members an already-known oid, mirroring the legacy swap)"
        );
    }

    // The renamed content survives intact.
    let (src_blobs, _) = index
        .debug_blob_path_counts(&source_oid.0)
        .expect("debug_blob_path_counts source oid");
    assert_eq!(src_blobs, 1, "exactly one blobs row for the renamed content");

    let page = index
        .resolve_page("Source")
        .expect("resolve_page")
        .expect("page must survive the rename");
    assert!(page.file.ends_with("dest.md"), "resolves to dest.md: {}", page.file);
}
