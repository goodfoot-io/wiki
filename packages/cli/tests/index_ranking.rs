//! Ranking test: documents whose query token appears in higher-weighted fields
//! must rank above documents where it appears only in lower-weighted fields.
//!
//! Field weight tuple from CARD.md: `(title:5, aliases:4, tags:3, keywords:3, summary:2, body:1)`.
//!
//! One document per field — each contains the query token "quantum" in
//! EXACTLY ONE field so BM25 with `bm25(fts, 5, 4, 3, 3, 2, 1)` can rank
//! by the per-column weight alone (no cross-field contamination).  The
//! expected descending rank order is:
//!   1. title_doc        (token in title)
//!   2. aliases_doc      (token in aliases)
//!   3. tags_doc         (token in tags)
//!   4. keywords_doc     (token in keywords)
//!   5. summary_doc      (token in summary)
//!   6. body_doc         (token in body only)

mod common;

use wiki::index::WikiIndex;

/// Expected ordering for a "quantum" query.  Tags and keywords share BM25
/// weight 3, so positions 2-3 are a tied bucket whose internal order is
/// not specified by the CARD; only the bucket boundaries are asserted.
const EXPECTED_HEAD: &[&str] = &["Quantum", "Alpha"];
const TIED_BUCKET: &[&str] = &["Beta", "Gamma"];
const EXPECTED_TAIL: &[&str] = &["Delta", "Epsilon"];

#[test]
fn ranking_matches_bm25_weight_tuple() {
    let repo = common::FixtureRepo::new();

    // (1) token in title only
    repo.write_file(
        "title_doc.md",
        "---\ntitle: Quantum\nsummary: A generic summary.\n---\n\nBody text.\n",
    );

    // (2) token in aliases only
    repo.write_file(
        "aliases_doc.md",
        "---\ntitle: Alpha\nsummary: A generic summary.\naliases: [quantum mechanics]\n---\n\nBody text.\n",
    );

    // (3) token in tags only
    repo.write_file(
        "tags_doc.md",
        "---\ntitle: Beta\nsummary: A generic summary.\ntags: [quantum]\n---\n\nBody text.\n",
    );

    // (4) token in keywords only
    repo.write_file(
        "keywords_doc.md",
        "---\ntitle: Gamma\nsummary: A generic summary.\nkeywords: [quantum]\n---\n\nBody text.\n",
    );

    // (5) token in summary only
    repo.write_file(
        "summary_doc.md",
        "---\ntitle: Delta\nsummary: A quantum summary.\n---\n\nBody text.\n",
    );

    // (6) token in body only
    repo.write_file(
        "body_doc.md",
        "---\ntitle: Epsilon\nsummary: A generic summary.\n---\n\nQuantum body text.\n",
    );

    repo.git_add(".");
    repo.git_commit("add ranking fixtures");

    // Raise SEARCH_LIMIT for this test so all 6 results come back.
    let index = WikiIndex::prepare(repo.root.as_path()).expect("prepare");
    let (results, _total) = index
        .search_weighted("quantum", 10, 0)
        .expect("search_weighted");

    let titles: Vec<&str> = results.iter().map(|r| r.title.as_str()).collect();

    assert_eq!(
        titles.len(),
        6,
        "expected 6 results; got {}: {titles:?}",
        titles.len()
    );
    assert_eq!(
        &titles[0..2],
        EXPECTED_HEAD,
        "head bucket (title, aliases) ranking did not match.\nExpected: {EXPECTED_HEAD:?}\nGot:      {:?}",
        &titles[0..2]
    );
    let mut middle: Vec<&str> = titles[2..4].to_vec();
    middle.sort();
    let mut expected_middle: Vec<&str> = TIED_BUCKET.to_vec();
    expected_middle.sort();
    assert_eq!(
        middle, expected_middle,
        "tied-weight bucket (tags, keywords) did not contain expected titles.\nExpected (sorted): {expected_middle:?}\nGot      (sorted): {middle:?}"
    );
    assert_eq!(
        &titles[4..6],
        EXPECTED_TAIL,
        "tail bucket (summary, body) ranking did not match.\nExpected: {EXPECTED_TAIL:?}\nGot:      {:?}",
        &titles[4..6]
    );
}
