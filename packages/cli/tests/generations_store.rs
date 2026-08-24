//! Acceptance checks for the generations freshness tier of the merged store
//! (plan merged-store-generations, D5/D6/D10; bootstrap Phases 1+2).
//!
//! Every check here is `#[ignore]`d: they exercise
//! `wiki::index::generations::GenerationsStore` AS A CONSUMER — the stub
//! bodies are `todo!()` sentinels until Phase 3 — so the contract's
//! ergonomics surface now while nothing runs. They compile against today's
//! tree and stay pending; Phase 3 unskips them batch by batch.
//!
//! Contracts pinned (check → source):
//!
//! 1. digest canonicalization injectivity — D5 encoding spec
//! 2. worktree signature canonicalization — D5 (`worktree_sig`)
//! 3. publish-on-conflict-discard incl. loser `fts_` drop — D5 publish
//! 4. serving isolation: divergent generations coexist — D5 mediation
//! 5. rankings equal a cold rebuild of the same digest — D5 per-gen FTS
//! 6. no physical delete under retained generations — D5 immutability
//! 7. path-granular carry-forward correctness — D5 refresh delta base
//! 8. deleted-directory carry-forward counts — D6 (rewrites
//!    `index_dir_mtimes_prune.rs`)
//! 9. hostile FS disables carry-forward (full rescan rows only) — rewrites
//!    `index_hostile_dir_mtimes.rs`
//! 10. carry-forward counts: only changed files re-ingest — rewrites
//!     `index_dir_mtime_merkle.rs`
//! 11. retention bound, recency liveness rule — D10
//! 12. never serve an unverified row — D5 serve-time verification
//! 13. compute-outside-write-txn invariant — D5 refresh ordering
//! 14. best-effort access-bucket update never fails a read — D5 gate hit

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use rusqlite::{Connection, OpenFlags};
use tempfile::TempDir;
use wiki::index::blob::compute_blob_oid;
use wiki::index::generations::{
    worktree_signature, Generation, GenerationsStore, GenPathRow, PublishCandidate,
    PublishOutcome, RETAINED_GENERATIONS, StateFingerprint, EMPTY_TREE_BASE, UNBORN_HEAD_OID,
    ZERO_INDEX_CHECKSUM, ZERO_WIKIIGNORE_HASH,
};
use wiki::index::ingest::WikiBlobFields;
use wiki::index::{BlobOid, Source};

// ── fixture helpers ─────────────────────────────────────────────────────

/// Open a fresh store whose common dir is the tempdir root; the store file
/// derives to `<tmp>/wiki/store.sqlite`.
fn open_store() -> (TempDir, GenerationsStore) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let store = GenerationsStore::open(tmp.path()).expect("open generations store");
    (tmp, store)
}

/// Publish and demand a fresh generation (conflicts fail the fixture).
fn published(store: &GenerationsStore, candidate: PublishCandidate) -> Generation {
    match store.publish(candidate).expect("publish") {
        PublishOutcome::Published { generation } => generation,
        PublishOutcome::ConflictDiscarded { existing } => {
            panic!("fixture expected a fresh publish, got conflict with gen {}", existing.gen_id)
        }
    }
}

/// Distinct-but-valid fingerprint per seed byte.
fn fingerprint(seed: u8) -> StateFingerprint {
    StateFingerprint {
        head_oid: format!("{:040x}", seed as u64 + 1),
        head_tree_oid: format!("{:040x}", seed as u64 + 2),
        index_checksum: [seed; 20],
        wikiignore_hash: [seed ^ 0xA5; 20],
        worktree_sig: [seed; 32],
    }
}

/// One wiki page: canonical bytes ⇒ real blob oid + parsed fields.
fn page_blob(title: &str, body: &str) -> (BlobOid, WikiBlobFields) {
    let raw = format!("---\ntitle: {title}\nsummary: Summary of {title}.\n---\n\n{body}\n");
    (
        compute_blob_oid(raw.as_bytes()),
        WikiBlobFields {
            title: title.to_string(),
            summary: format!("Summary of {title}."),
            body: format!("\n{body}\n"),
            aliases_text: String::new(),
            tags_text: String::new(),
            keywords_text: String::new(),
        },
    )
}

/// A worktree-source page fixture with a controllable walk mtime.
#[derive(Clone)]
struct Page {
    rel: String,
    title: String,
    mtime_ns: i64,
}

impl Page {
    fn new(rel: &str, title: &str, mtime_ns: i64) -> Self {
        Self { rel: rel.to_string(), title: title.to_string(), mtime_ns }
    }

    fn row(&self) -> GenPathRow {
        let (oid, _) = page_blob(&self.title, "Shared body prose.");
        GenPathRow {
            source: Source::Worktree,
            path_rel: self.rel.clone(),
            oid,
            parent_dir: self.rel.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default(),
            stat_mtime_ns: Some(self.mtime_ns),
        }
    }

    fn blob(&self) -> (BlobOid, WikiBlobFields) {
        page_blob(&self.title, "Shared body prose.")
    }
}

fn candidate(fingerprint: StateFingerprint, pages: &[Page]) -> PublishCandidate {
    PublishCandidate {
        fingerprint,
        publisher: Some("test-worktree".to_string()),
        paths: pages.iter().map(Page::row).collect(),
        new_blobs: pages.iter().map(Page::blob).collect(),
    }
}

/// Read-only direct access for physical-layout contracts.
fn raw_conn(path: &Path) -> Connection {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("open store db read-only")
}

fn writable_conn(path: &Path) -> Connection {
    Connection::open(path).expect("open store db read-write")
}

fn scalar(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |row| row.get(0)).expect("scalar query")
}

fn fts_tables(conn: &Connection) -> Vec<String> {
    // Only the virtual children themselves (`fts_<gen_id>`); FTS5's shadow
    // tables (`fts_1_data`, `fts_1_idx`, …) carry a second underscore and
    // are excluded — gen ids are integers and never contain one.
    let mut stmt = conn
        .prepare(
            "SELECT name FROM sqlite_master WHERE type = 'table'
             AND name LIKE 'fts\\_%' ESCAPE '\\'
             AND name NOT LIKE 'fts\\_%\\_%' ESCAPE '\\'
             ORDER BY name",
        )
        .expect("prepare sqlite_master");
    let rows = stmt.query_map([], |r| r.get::<_, String>(0)).expect("fts listing");
    rows.map(|r| r.expect("row")).collect()
}

/// BM25 ranking over one generation's FTS table, as document oids ordered
/// by score then oid — the order search must serve, in a store-independent
/// identity space (blob rowids are per-store insert order; scores and their
/// tie-break must match a cold rebuild exactly). Column weights match
/// production: bm25(fts_g, 5,4,3,3,2,1).
fn ranked_oids(conn: &Connection, fts_table: &str, query: &str) -> Vec<String> {
    let sql = format!(
        "SELECT b.oid FROM {fts_table} \
         JOIN blobs b ON b.rowid = {fts_table}.rowid \
         WHERE {fts_table} MATCH ?1 \
         ORDER BY bm25({fts_table}, 5, 4, 3, 3, 2, 1) ASC, b.oid ASC"
    );
    let mut stmt = conn.prepare(&sql).expect("prepare ranked match");
    let rows = stmt.query_map([query], |r| r.get::<_, String>(0)).expect("ranked match");
    rows.map(|r| r.expect("row")).collect()
}

// ── 1–2: digest canonicalization ────────────────────────────────────────

#[test]
fn state_digest_is_injective_over_field_order_and_boundaries() {
    let base = StateFingerprint {
        head_oid: format!("{:040x}", 1),
        head_tree_oid: format!("{:040x}", 2),
        index_checksum: [3u8; 20],
        wikiignore_hash: [4u8; 20],
        worktree_sig: [5u8; 32],
    };

    // Determinism: identical fingerprints hash identically.
    assert_eq!(base.digest(), base.clone().digest());
    assert_eq!(base.digest_hex(), base.clone().digest_hex());

    // Field-order injectivity: transposing two same-width fields' values
    // must change the digest — positional concatenation would collide.
    let transposed =
        StateFingerprint { index_checksum: [4u8; 20], wikiignore_hash: [3u8; 20], ..base.clone() };
    assert_ne!(base.digest(), transposed.digest());

    // Boundary-split injectivity across adjacent string fields:
    // ("ab","c") vs ("a","bc") vs ("abc","") — untagged concatenation maps
    // all three to the same bytes; length tags forbid it.
    let split_a = StateFingerprint { head_oid: "ab".into(), head_tree_oid: "c".into(), ..base.clone() };
    let split_b = StateFingerprint { head_oid: "a".into(), head_tree_oid: "bc".into(), ..base.clone() };
    let split_c =
        StateFingerprint { head_oid: "abc".into(), head_tree_oid: String::new(), ..base.clone() };
    assert_ne!(split_a.digest(), split_b.digest());
    assert_ne!(split_a.digest(), split_c.digest());
    assert_ne!(split_b.digest(), split_c.digest());

    // Binary boundary split across the fixed-width checksum field.
    let mut checksum_a = [0x11u8; 20];
    checksum_a[19] = 0x22;
    let mut checksum_b = [0x22u8; 20];
    checksum_b[0] = 0x11;
    let binary_a = StateFingerprint { index_checksum: checksum_a, ..base.clone() };
    let binary_b = StateFingerprint { index_checksum: checksum_b, ..base.clone() };
    assert_ne!(binary_a.digest(), binary_b.digest());

    // Sentinels are first-class digest inputs, not out-of-band values.
    let unborn = StateFingerprint { head_oid: UNBORN_HEAD_OID.into(), ..base.clone() };
    let empty_base = StateFingerprint { head_tree_oid: EMPTY_TREE_BASE.into(), ..base.clone() };
    let zero_checksum =
        StateFingerprint { index_checksum: ZERO_INDEX_CHECKSUM, ..base.clone() };
    let zero_ignore =
        StateFingerprint { wikiignore_hash: ZERO_WIKIIGNORE_HASH, ..base.clone() };
    assert_ne!(base.digest(), unborn.digest());
    assert_ne!(base.digest(), empty_base.digest());
    assert_ne!(base.digest(), zero_checksum.digest());
    assert_ne!(base.digest(), zero_ignore.digest());
}

#[test]
fn worktree_signature_canonicalizes_order_and_boundaries() {
    let sorted = vec![("a.md".to_string(), 1), ("b.md".to_string(), 2)];
    let shuffled = vec![("b.md".to_string(), 2), ("a.md".to_string(), 1)];

    // Walk order never leaks into the signature.
    assert_eq!(worktree_signature(&sorted), worktree_signature(&shuffled));

    // Pair-boundary injectivity: ("ab",1)+("c",2) != ("a",1)+("bc",2).
    let boundary_a = vec![("ab".to_string(), 1), ("c".to_string(), 2)];
    let boundary_b = vec![("a".to_string(), 1), ("bc".to_string(), 2)];
    assert_ne!(worktree_signature(&boundary_a), worktree_signature(&boundary_b));

    // The mtime participates in the signature.
    assert_ne!(
        worktree_signature(&[("p.md".to_string(), 1)]),
        worktree_signature(&[("p.md".to_string(), 2)])
    );

    // The empty walk is a stable, distinct state.
    assert_eq!(worktree_signature(&[]), worktree_signature(&[]));
    assert_ne!(worktree_signature(&[]), worktree_signature(&sorted));
}

// ── 3–6: publish, isolation, immutability ───────────────────────────────

#[test]
fn publish_conflict_discards_loser_fts_table_and_serves_existing() {
    let (_tmp, store) = open_store();
    let fp = fingerprint(1);

    let winner = published(
        &store,
        candidate(fp.clone(), &[Page::new("a.md", "Alpha", 100), Page::new("b.md", "Beta", 200)]),
    );

    let fts_before = fts_tables(&raw_conn(store.path()));
    assert_eq!(fts_before.len(), 1, "one generation, one fts_ table");

    // Same canonical state, different corpus: the digest conflicts. The
    // loser's just-built fts_ table must be dropped and the existing
    // generation served unchanged (CAS-analog discard, never an error).
    let conflicting = candidate(
        fp.clone(),
        &[
            Page::new("a.md", "Alpha", 100),
            Page::new("b.md", "Beta", 200),
            Page::new("junk.md", "Junk", 300),
        ],
    );
    match store.publish(conflicting).expect("conflicting publish") {
        PublishOutcome::ConflictDiscarded { existing } => {
            assert_eq!(existing.gen_id, winner.gen_id);
            assert_eq!(existing.fingerprint, fp);
            assert_eq!(existing.blob_count, 2);
        }
        PublishOutcome::Published { generation } => {
            panic!("duplicate digest must not publish a second generation: {generation:?}")
        }
    }

    let conn = raw_conn(store.path());
    assert_eq!(scalar(&conn, "SELECT COUNT(*) FROM generations"), 1);
    assert_eq!(scalar(&conn, "SELECT COUNT(*) FROM gen_paths WHERE path_rel = 'junk.md'"), 0);
    assert_eq!(scalar(&conn, "SELECT COUNT(*) FROM blobs WHERE title = 'Junk'"), 0);
    assert_eq!(
        fts_tables(&conn),
        fts_before,
        "the loser's fts_ table must be dropped on conflict"
    );
}

#[test]
fn divergent_generations_coexist_serving_their_own_corpus() {
    let (_tmp, store) = open_store();

    let old_gen = published(
        &store,
        candidate(
            fingerprint(1),
            &[
                Page::new("alpha.md", "Alpha", 100),
                Page::new("beta.md", "Beta", 200),
                Page::new("gamma.md", "Gamma", 300),
            ],
        ),
    );

    // Newer state: alpha edited in place, gamma deleted, delta added.
    let new_gen = published(
        &store,
        candidate(
            fingerprint(2),
            &[
                Page::new("alpha.md", "Alpha Edited", 400),
                Page::new("beta.md", "Beta", 200),
                Page::new("delta.md", "Delta", 500),
            ],
        ),
    );
    assert_ne!(old_gen.gen_id, new_gen.gen_id);

    // Both generations remain resolvable by their own canonical state —
    // publishing never invalidates a retained predecessor.
    let served_old = store.lookup_digest(&fingerprint(1)).expect("old lookup").expect("old hit");
    let served_new = store.lookup_digest(&fingerprint(2)).expect("new lookup").expect("new hit");
    assert_eq!(served_old.gen_id, old_gen.gen_id);
    assert_eq!(served_new.gen_id, new_gen.gen_id);

    // Non-FTS leg: each generation serves exactly its own corpus mapping.
    let old_paths = store.generation_paths(old_gen.gen_id, Source::Worktree).expect("old paths");
    let new_paths = store.generation_paths(new_gen.gen_id, Source::Worktree).expect("new paths");
    let has = |rows: &[GenPathRow], rel: &str| rows.iter().any(|r| r.path_rel == rel);
    assert!(has(&old_paths, "gamma.md"), "deleted page stays resolvable through the older generation");
    assert!(!has(&new_paths, "gamma.md"));
    assert!(has(&new_paths, "delta.md"));
    assert!(!has(&old_paths, "delta.md"));

    // FTS leg: token corpora stay disjoint per generation.
    let conn = raw_conn(store.path());
    let old_fts = format!("fts_{}", old_gen.gen_id);
    let new_fts = format!("fts_{}", new_gen.gen_id);
    assert_eq!(ranked_oids(&conn, &old_fts, "gamma").len(), 1);
    assert_eq!(ranked_oids(&conn, &new_fts, "gamma").len(), 0);
    assert_eq!(ranked_oids(&conn, &new_fts, "delta").len(), 1);
    assert_eq!(ranked_oids(&conn, &old_fts, "delta").len(), 0);
    assert_eq!(ranked_oids(&conn, &old_fts, "edited").len(), 0);
    assert_eq!(ranked_oids(&conn, &new_fts, "edited").len(), 1);
}

#[test]
fn warm_rankings_equal_cold_rebuild_of_same_digest() {
    let (_warm_tmp, warm) = open_store();
    let (_cold_tmp, cold) = open_store();
    let fp = fingerprint(7);
    let pages = [
        Page::new("docs/alpha.md", "Gadget Alpha", 100),
        Page::new("docs/beta.md", "Gadget Gadget", 200),
        Page::new("other/gamma.md", "Gamma", 300),
    ];

    // Warm: reached through an earlier divergent state, then this one.
    published(
        &warm,
        candidate(fingerprint(1), &[Page::new("seed.md", "Seed Page", 900)]),
    );
    let warm_gen = published(&warm, candidate(fp.clone(), &pages));

    // Cold: a fresh store rebuilt straight to the same canonical state.
    let cold_gen = published(&cold, candidate(fp.clone(), &pages));

    assert_eq!(fp.digest(), cold_gen.digest);
    assert_eq!(
        cold_gen.digest,
        warm_gen.digest,
        "same canonical state must hash identically across stores"
    );

    let warm_conn = raw_conn(warm.path());
    let cold_conn = raw_conn(cold.path());
    for query in ["gadget", "alpha", "prose"] {
        assert_eq!(
            ranked_oids(&warm_conn, &format!("fts_{}", warm_gen.gen_id), query),
            ranked_oids(&cold_conn, &format!("fts_{}", cold_gen.gen_id), query),
            "BM25 ranking must be identical warm vs cold for query {query:?}"
        );
    }
}

#[test]
fn retained_generation_is_never_physically_deleted_by_later_publish() {
    let (_tmp, store) = open_store();

    let old_page = Page::new("old_only.md", "Old Only", 100);
    let shared_page = Page::new("shared.md", "Shared", 200);
    // The oids the fixture pages actually publish under (Page::blob() is
    // deterministic per title).
    let (keep_old_oid, _) = old_page.blob();
    let (shared_oid, _) = shared_page.blob();

    // Newer publish drops old_only.md from its corpus — and deletes
    // nothing physical: publish is insert-only under immutability.
    let old_gen = published(
        &store,
        candidate(fingerprint(1), &[old_page, shared_page.clone()]),
    );
    let new_gen = published(
        &store,
        candidate(fingerprint(2), &[shared_page, Page::new("new_only.md", "New Only", 300)]),
    );

    let conn = raw_conn(store.path());
    assert_eq!(scalar(&conn, "SELECT COUNT(*) FROM generations"), 2);

    // The displaced page's blob row survives with positive refcount...
    let refcount = conn
        .query_row("SELECT refcount FROM blobs WHERE oid = ?1", [&keep_old_oid.0], |r| {
            r.get::<_, i64>(0)
        })
        .expect("displaced blob row must survive");
    assert!(refcount >= 1, "the retained generation still references it");

    // ...its gen_paths rows stay intact through the older generation...
    assert_eq!(
        scalar(
            &conn,
            &format!("SELECT COUNT(*) FROM gen_paths WHERE gen_id = {}", old_gen.gen_id)
        ),
        2
    );
    // ...and its fts_ child still answers its own corpus.
    assert!(fts_tables(&conn).contains(&format!("fts_{}", old_gen.gen_id)));
    let old_fts = format!("fts_{}", old_gen.gen_id);
    let new_fts = format!("fts_{}", new_gen.gen_id);
    assert_eq!(
        ranked_oids(&conn, &old_fts, "old").len(),
        1,
        "older generation still serves its displaced page"
    );
    assert_eq!(
        ranked_oids(&conn, &new_fts, "old").len(),
        0,
        "newer generation does not see the displaced page"
    );
    let shared_hex = shared_oid.0;
    assert!(scalar(&conn, &format!("SELECT COUNT(*) FROM blobs WHERE oid = '{shared_hex}'")) > 0);
    assert_eq!(scalar(&conn, "SELECT COUNT(*) FROM blobs WHERE title = 'New Only'"), 1);

    // Membership-refcount ruling: after every gen_paths-mutating operation,
    // blobs.refcount must equal COUNT(DISTINCT gen_id) referencing the oid.
    // keep_old is referenced only by the retained older generation; shared
    // by both.
    let refcount_invariant = |oid: &str| -> i64 {
        conn.query_row(
            "SELECT b.refcount = COALESCE(
                 (SELECT COUNT(DISTINCT gp.gen_id) FROM gen_paths gp WHERE gp.oid = b.oid), 0)
             FROM blobs b WHERE b.oid = ?1",
            [oid],
            |r| r.get::<_, i64>(0),
        )
        .expect("refcount invariant query")
    };
    assert_eq!(refcount_invariant(&keep_old_oid.0), 1);
    let old_refcount: i64 = conn
        .query_row(
            "SELECT refcount FROM blobs WHERE oid = ?1",
            [&keep_old_oid.0],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(old_refcount, 1, "exactly one retained generation holds the displaced blob");
    assert_eq!(refcount_invariant(&shared_hex), 1);
    let shared_refcount: i64 = conn
        .query_row("SELECT refcount FROM blobs WHERE oid = ?1", [&shared_hex], |r| r.get(0))
        .unwrap();
    assert_eq!(shared_refcount, 2, "both retained generations hold the shared blob");
}

// ── 7–10: carry-forward contracts (dir-mtimes suite rewrites) ───────────

#[test]
fn carry_forward_matches_path_and_mtime_pairs() {
    let (_tmp, store) = open_store();

    let gen1 = published(
        &store,
        candidate(
            fingerprint(1),
            &[Page::new("p1.md", "P1", 100), Page::new("sub/p2.md", "P2", 200), Page::new("sub/p3.md", "P3", 300)],
        ),
    );

    // Next refresh: p1/p3 stat-identical (carried verbatim), p2 edited.
    // The published generation carries rows whose (path_rel,
    // stat_mtime_ns) pairs match the walk and re-ingests the rest — never
    // keyed on directory mtimes alone (D6 replaces dir_mtimes).
    let gen2 = published(
        &store,
        candidate(
            fingerprint(2),
            &[Page::new("p1.md", "P1", 100), Page::new("sub/p2.md", "P2 Edited", 999), Page::new("sub/p3.md", "P3", 300)],
        ),
    );

    let original = store.generation_paths(gen1.gen_id, Source::Worktree).expect("gen1 paths");
    let carried = store.generation_paths(gen2.gen_id, Source::Worktree).expect("gen2 paths");
    fn find<'a>(rows: &'a [GenPathRow], rel: &str) -> &'a GenPathRow {
        rows.iter()
            .find(|r| r.path_rel == rel)
            .unwrap_or_else(|| panic!("{rel} missing"))
    }

    assert_eq!(find(&carried, "p1.md"), find(&original, "p1.md"), "carried row byte-identical");
    assert_eq!(find(&carried, "sub/p3.md"), find(&original, "sub/p3.md"));
    assert_ne!(
        find(&carried, "sub/p2.md"),
        find(&original, "sub/p2.md"),
        "edited file re-ingested with the walk's fresh mtime"
    );

    // Immutability: the older generation is untouched by the newer publish.
    assert_eq!(
        store.generation_paths(gen1.gen_id, Source::Worktree).expect("still intact").len(),
        3
    );
}

#[test]
fn deleted_directory_leaves_no_trace_in_the_serving_generation() {
    let (_tmp, store) = open_store();

    // Rewrite of index_dir_mtimes_prune: the stale-dir contract becomes a
    // carry-forward count. Cold world recorded dir_mtimes rows for root +
    // `.wiki` + subdir (3) and pruned none on delete; here there is no
    // working-tree artifact at all — the serving generation simply holds no
    // stale subdir rows, while retained history keeps them.
    let gen1 = published(
        &store,
        candidate(
            fingerprint(1),
            &[
                Page::new("root.md", "Root", 100),
                Page::new("subdir/page1.md", "Sub One", 200),
                Page::new("subdir/page2.md", "Sub Two", 300),
            ],
        ),
    );
    assert_eq!(
        store.generation_paths(gen1.gen_id, Source::Worktree).expect("gen1 paths").len(),
        3
    );

    // Delete the entire subdirectory; next publish serves without it.
    let gen2 = published(
        &store,
        candidate(fingerprint(2), &[Page::new("root.md", "Root", 100)]),
    );

    let serving = store.generation_paths(gen2.gen_id, Source::Worktree).expect("gen2 paths");
    assert_eq!(serving.len(), 1, "stale subdir rows pruned from the serving generation");
    assert!(serving.iter().all(|r| r.parent_dir.is_empty()));

    // Retained history still shows the deleted subtree.
    assert_eq!(
        store.generation_paths(gen1.gen_id, Source::Worktree).expect("gen1 retained").len(),
        3
    );
    let conn = raw_conn(store.path());
    assert!(fts_tables(&conn).contains(&format!("fts_{}", gen1.gen_id)));
    assert_eq!(ranked_oids(&conn, &format!("fts_{}", gen1.gen_id), "one").len(), 1);
    assert_eq!(ranked_oids(&conn, &format!("fts_{}", gen2.gen_id), "one").len(), 0);
}

#[test]
fn hostile_fs_disables_carry_forward_full_rescan_only() {
    let (_tmp, store) = open_store();

    // Rewrite of index_hostile_dir_mtimes: hostile FS never consults stored
    // mtimes (pass3_full_rescans upstream), so every walked file lands in
    // the next generation from THIS walk — no row enters on stored-mtime
    // evidence, and stored rows cannot poison served mtimes.
    published(
        &store,
        candidate(
            fingerprint(1),
            &[Page::new("dir0/a.md", "A", 100), Page::new("dir1/b.md", "B", 200)],
        ),
    );

    // Sabotage the stored mtimes: a hostile refresh ignores them entirely.
    {
        let conn = writable_conn(store.path());
        conn.execute_batch("UPDATE gen_paths SET stat_mtime_ns = 555;").expect("sabotage mtimes");
    }

    let gen2 = published(
        &store,
        candidate(
            fingerprint(2),
            &[Page::new("dir0/a.md", "A", 100), Page::new("dir1/b.md", "B", 200)],
        ),
    );

    let rows = store.generation_paths(gen2.gen_id, Source::Worktree).expect("gen2 paths");
    assert_eq!(rows.len(), 2, "full rescan publishes the complete walked corpus");
    assert!(
        rows.iter().all(|r| r.stat_mtime_ns == Some(100) || r.stat_mtime_ns == Some(200)),
        "rows must reflect walk truth, never sabotaged stored mtimes"
    );
    assert_eq!(scalar(&raw_conn(store.path()), "SELECT COUNT(*) FROM generations"), 2);
}

#[test]
fn carry_forward_reingests_only_changed_files() {
    let (_tmp, store) = open_store();

    // Rewrite of index_dir_mtime_merkle: the Merkle short-circuit contract
    // becomes a carry-forward count. Eight files across four dirs; editing
    // exactly one re-ingests exactly one row — seven carry forward
    // byte-identical, including the unchanged sibling inside the changed
    // directory (carry-forward is path-granular, not dir-granular).
    let mut cold = Vec::new();
    let mut warm = Vec::new();
    for d in 0..4usize {
        for p in 0..2usize {
            let rel = format!("docs{d}/{}.md", if p == 0 { 'a' } else { 'b' });
            let title = format!("{}{}", ['A', 'B'][p], d);
            let mtime = 1000 + (d * 10 + p) as i64;
            cold.push(Page::new(&rel, &title, mtime));
            if d == 1 && p == 0 {
                warm.push(Page::new(&rel, &format!("{title} Edited"), 9901));
            } else {
                warm.push(Page::new(&rel, &title, mtime));
            }
        }
    }

    let gen1 = published(&store, candidate(fingerprint(1), &cold));
    let gen2 = published(&store, candidate(fingerprint(2), &warm));

    let old = store.generation_paths(gen1.gen_id, Source::Worktree).expect("gen1 paths");
    let new = store.generation_paths(gen2.gen_id, Source::Worktree).expect("gen2 paths");
    assert_eq!(old.len(), 8);
    assert_eq!(new.len(), 8);

    let differing: Vec<&String> =
        old.iter().zip(new.iter()).filter(|(o, n)| o != n).map(|(o, _)| &o.path_rel).collect();
    assert_eq!(differing.len(), 1, "exactly the edited file re-ingests");
    assert_eq!(differing[0], "docs1/a.md");
}

// ── 11–14: retention, verification, invariants ──────────────────────────

#[test]
fn retention_keeps_newest_plus_recency_bound_per_d10() {
    let (_tmp, store) = open_store();

    // Twelve sequential publishes; each adds one unique page plus carries
    // the shared corpus forward. created_at must increase monotonically so
    // retention ordering is total even within one wall-clock second.
    let mut gens = Vec::new();
    for seed in 1..=12u8 {
        let cand = candidate(
            fingerprint(seed),
            &[
                Page::new("shared.md", "Shared", 42),
                Page::new(&format!("unique_{seed}.md"), &format!("Unique {seed}"), seed as i64),
            ],
        );
        gens.push(published(&store, cand));
    }
    for pair in gens.windows(2) {
        assert!(
            pair[1].created_at > pair[0].created_at,
            "created_at must be monotonic: {} then {}",
            pair[0].created_at,
            pair[1].created_at
        );
    }

    // Recency-liveness divergence (D10): touch the three oldest so their
    // access buckets beat everything; give the newest the worst bucket —
    // recency protection wins over access recency.
    {
        let conn = writable_conn(store.path());
        conn.execute_batch(
            "UPDATE generations SET access_bucket = 1000000 WHERE gen_id IN (1, 2, 3);
             UPDATE generations SET access_bucket = 0 WHERE gen_id = 10;",
        )
        .expect("diverge buckets");
    }

    let stats = store.maintain().expect("maintain");

    // Candidates = everything except the newest (12): eleven, sorted by
    // (access_bucket ASC, created_at ASC) ⇒ 10, then 4..9, then 1..3.
    // Retaining the newest 8 of those evicts exactly [10, 4, 5]; the touched
    // elders 1–3 survive despite their age, and protected-newest 12 survives
    // despite its dead bucket. Bound = RETAINED_GENERATIONS + 1 total.
    assert_eq!(stats.evicted_gen_ids, vec![10, 4, 5]);
    assert_eq!(stats.generations_before, 12);
    assert_eq!(stats.generations_after, RETAINED_GENERATIONS as u64 + 1);
    assert!(stats.bytes_before > 0);
    assert!(stats.bytes_after > 0);

    let conn = raw_conn(store.path());
    assert_eq!(scalar(&conn, "SELECT COUNT(*) FROM generations"), RETAINED_GENERATIONS as i64 + 1);
    let survivors = fts_tables(&conn);
    for id in [1i64, 2, 3, 6, 7, 8, 9, 11, 12] {
        assert!(survivors.contains(&format!("fts_{id}")), "fts_{id} must survive");
        assert_eq!(
            scalar(&conn, &format!("SELECT COUNT(*) FROM gen_paths WHERE gen_id = {id}")),
            2,
            "gen {id} rows intact"
        );
    }
    for id in [10i64, 4, 5] {
        assert!(!survivors.contains(&format!("fts_{id}")), "fts_{id} must be evicted");
        assert_eq!(
            scalar(&conn, &format!("SELECT COUNT(*) FROM gen_paths WHERE gen_id = {id}")),
            0,
            "evicted gen {id} paths reclaimed"
        );
    }
    // Blobs referenced only by evicted generations are reclaimed; anything
    // any survivor references stays — shared.md above all.
    for seed in [4u8, 5, 10] {
        assert_eq!(
            scalar(&conn, &format!("SELECT COUNT(*) FROM blobs WHERE title = 'Unique {seed}'")),
            0,
            "unique_{seed} unreferenced after eviction"
        );
    }
    assert!(scalar(&conn, "SELECT COUNT(*) FROM blobs WHERE title = 'Unique 6'") > 0);
    assert!(scalar(&conn, "SELECT COUNT(*) FROM blobs WHERE title = 'Shared'") > 0);
}

#[test]
fn unverified_generation_row_never_serves() {
    let (_tmp, store) = open_store();
    let fp = fingerprint(3);
    let generation = published(
        &store,
        candidate(fp.clone(), &[Page::new("a.md", "Alpha", 100), Page::new("b.md", "Beta", 200)]),
    );

    assert!(store.lookup_digest(&fp).expect("clean lookup").is_some());
    assert!(store.lookup_digest(&fingerprint(99)).expect("unknown lookup").is_none());

    // blob_count mismatching its member set ⇒ miss (fail-open toward
    // rehash), restorable once the row verifies again.
    {
        let conn = writable_conn(store.path());
        conn.execute_batch("UPDATE generations SET blob_count = blob_count + 1;")
            .expect("corrupt blob_count");
    }
    assert!(
        store.lookup_digest(&fp).expect("lookup after corruption").is_none(),
        "a row failing serve-time verification must be reported as a miss"
    );
    {
        let conn = writable_conn(store.path());
        conn.execute(
            "UPDATE generations SET blob_count = ?1 WHERE gen_id = ?2",
            rusqlite::params![generation.blob_count, generation.gen_id],
        )
        .expect("restore blob_count");
    }
    assert!(store.lookup_digest(&fp).expect("lookup after repair").is_some());

    // Missing fts_ child ⇒ miss; recreating the exact schema restores
    // serviceability.
    {
        let conn = writable_conn(store.path());
        let gid = generation.gen_id;
        conn.execute_batch(&format!("DROP TABLE fts_{gid};")).expect("drop fts child");
    }
    assert!(
        store.lookup_digest(&fp).expect("lookup without fts").is_none(),
        "missing fts_ table must be reported as a miss"
    );
    {
        let conn = writable_conn(store.path());
        conn.execute_batch(&format!(
            "CREATE VIRTUAL TABLE fts_{gid} USING fts5(
                 title, aliases_text, tags_text, keywords_text, summary, body,
                 tokenize='unicode61 remove_diacritics 2',
                 prefix='2 3 4'
             );",
            gid = generation.gen_id
        ))
        .expect("recreate fts child");
    }
    assert!(store.lookup_digest(&fp).expect("lookup after rebuild").is_some());
}

#[test]
fn compute_happens_outside_write_txn() {
    let (_tmp, store) = open_store();
    assert!(!store.is_in_write_txn(), "freshly opened handle is autocommit");

    // The sanctioned write window exposes itself; heavy parse/ingest never
    // runs inside one because PublishCandidate is inert data built outside.
    store
        .with_write_txn(|inner| {
            assert!(inner.is_in_write_txn(), "inside the explicit write txn");
            Ok(())
        })
        .expect("write txn closure");
    assert!(!store.is_in_write_txn(), "txn closed on scope exit");

    let cand = candidate(fingerprint(4), &[Page::new("a.md", "Alpha", 100)]);
    assert!(!store.is_in_write_txn(), "candidate construction is outside any txn");
    published(&store, cand);
    assert!(!store.is_in_write_txn(), "publish leaves no txn open");
}

#[test]
fn best_effort_access_touch_never_fails_lookup() {
    let (_tmp, store) = open_store();
    let fp = fingerprint(5);
    published(&store, candidate(fp.clone(), &[Page::new("a.md", "Alpha", 100)]));

    // Hold the store's write lock from a second connection: the gate-hit
    // SELECT proceeds on its WAL snapshot, the best-effort access_bucket
    // UPDATE times out busy and is swallowed — the read neither fails nor
    // waits unboundedly (hot reads are not writes).
    let holder = writable_conn(store.path());
    holder
        .execute_batch("BEGIN IMMEDIATE; CREATE TABLE hold_the_write_lock (x INTEGER);")
        .expect("acquire write lock");

    let start = Instant::now();
    let hit = store.lookup_digest(&fp).expect("gate hit must survive concurrent write-lock hold");
    let elapsed = start.elapsed();

    assert!(hit.is_some(), "hot reads are never failed by bookkeeping");
    assert!(elapsed < Duration::from_secs(10), "best-effort touch must be bounded, took {elapsed:?}");
}
