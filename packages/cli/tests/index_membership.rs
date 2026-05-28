//! Membership test: only markdown files with both non-empty `title` AND
//! non-empty `summary` appear in `search_weighted` results.
//!
//! Four files are indexed:
//!   (a) title + summary  — MUST appear
//!   (b) title only       — must NOT appear
//!   (c) summary only     — must NOT appear
//!   (d) no frontmatter   — must NOT appear

mod common;

use wiki::index::{WikiIndex, SEARCH_LIMIT};

#[test]
fn only_full_wiki_pages_appear_in_results() {
    let repo = common::FixtureRepo::new();

    // (a) full wiki page
    repo.write_wiki_md("full.md", "Full Page", "A full summary.", "Body alpha.");

    // (b) title only — not a wiki page
    repo.write_file(
        "title_only.md",
        "---\ntitle: Title Only\n---\n\nBody beta.\n",
    );

    // (c) summary only — not a wiki page
    repo.write_file(
        "summary_only.md",
        "---\nsummary: Summary Only\n---\n\nBody gamma.\n",
    );

    // (d) no frontmatter — not a wiki page
    repo.write_file("no_frontmatter.md", "# Heading\n\nBody delta.\n");

    // Commit all four so Pass 1 picks them up.
    repo.git_add(".");
    repo.git_commit("add membership fixtures");

    let index = WikiIndex::prepare(repo.root.as_path()).expect("prepare");

    // Search for "body" which appears in all four files.
    let (results, _total) = index
        .search_weighted("body", SEARCH_LIMIT, 0)
        .expect("search_weighted");

    let titles: Vec<&str> = results.iter().map(|r| r.title.as_str()).collect();

    assert!(
        titles.contains(&"Full Page"),
        "expected 'Full Page' in results, got: {titles:?}"
    );
    assert!(
        !titles.iter().any(|t| *t == "Title Only"),
        "title-only file must not appear in results, got: {titles:?}"
    );
    assert!(
        !titles.iter().any(|t| *t == "Summary Only"),
        "summary-only file must not appear in results, got: {titles:?}"
    );
}
