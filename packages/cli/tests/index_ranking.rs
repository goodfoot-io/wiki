//! Ranking test: documents whose query token appears in higher-weighted fields
//! must rank above documents where it appears only in lower-weighted fields.
//!
//! Field weight tuple from CARD.md: `(title:5, aliases:4, tags:3, keywords:3, summary:2, body:1)`.
//!
//! One document per field — each contains the query token "quantum" only in
//! the tested field.  The expected descending rank order is:
//!   1. title_doc        (token in title)
//!   2. aliases_doc      (token in aliases)
//!   3. tags_doc         (token in tags)
//!   4. keywords_doc     (token in keywords)
//!   5. summary_doc      (token in summary)
//!   6. body_doc         (token in body only)

mod common;

use wiki::index::{WikiIndex, SEARCH_LIMIT};

/// Frozen expected title ordering for a "quantum" query.
const EXPECTED_ORDER: &[&str] = &[
    "Title Quantum",
    "Aliases Quantum",
    "Tags Quantum",
    "Keywords Quantum",
    "Summary Quantum",
    "Body Quantum",
];

#[test]
fn ranking_matches_bm25_weight_tuple() {
    let repo = common::FixtureRepo::new();

    // (1) token in title
    repo.write_file(
        "title_doc.md",
        "---\ntitle: Title Quantum\nsummary: A generic summary.\n---\n\nBody text.\n",
    );

    // (2) token in aliases (custom frontmatter field consumed by ingest)
    repo.write_file(
        "aliases_doc.md",
        "---\ntitle: Aliases Quantum\nsummary: A generic summary.\naliases: [quantum mechanics]\n---\n\nBody text.\n",
    );

    // (3) token in tags
    repo.write_file(
        "tags_doc.md",
        "---\ntitle: Tags Quantum\nsummary: A generic summary.\ntags: [quantum]\n---\n\nBody text.\n",
    );

    // (4) token in keywords
    repo.write_file(
        "keywords_doc.md",
        "---\ntitle: Keywords Quantum\nsummary: A generic summary.\nkeywords: [quantum]\n---\n\nBody text.\n",
    );

    // (5) token in summary only
    repo.write_file(
        "summary_doc.md",
        "---\ntitle: Summary Quantum\nsummary: A quantum summary.\n---\n\nBody text.\n",
    );

    // (6) token in body only
    repo.write_file(
        "body_doc.md",
        "---\ntitle: Body Quantum\nsummary: A generic summary.\n---\n\nQuantum body text.\n",
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
        titles, EXPECTED_ORDER,
        "ranking did not match BM25 weight tuple.\nExpected: {EXPECTED_ORDER:?}\nGot:      {titles:?}"
    );
}
