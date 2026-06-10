//! Reproduction test: [`resolve_page`](crate::index::search::resolve_page) builds a LIKE
//! pattern by interpolating user input directly, so `_` and `%` in the query
//! are interpreted as SQL wildcards rather than literals.
//!
//! **Hypothesis 1 — LIKE wildcard injection:**
//! `format!("%{input}%")` at
//! [`packages/cli/src/index/search.rs`](../../src/index/search.rs#L357) passes
//! user input directly into a SQL LIKE pattern. Characters `%` and `_` are
//! interpreted as SQL LIKE wildcards instead of literals.
//!
//! **Hypothesis 2 — Nondeterministic row pick:**
//! [`L286`](../../src/index/search.rs#L286) and
//! [`L354`](../../src/index/search.rs#L354) both use `LIMIT 1` without
//! `ORDER BY`. When multiple rows match the WHERE clause, SQLite picks
//! arbitrarily — making results nondeterministic and masking the LIKE wildcard
//! issue during casual testing.
//!
//! Both are in the same code path; one test covers both.
//!
//! ## How the test reproduces both bugs
//!
//! - Page A has the file name `test_page.md` (literal underscore).
//! - Page B has `test-page.md` (hyphen at the same position).
//! - Querying `resolve_page("test_page.md")` enters the path‑lookup branch
//!   (the input ends with `.md`). The SQL becomes
//!   `LIKE '%test_page.md%'`, where `_` matches any single character — so
//!   `test-page.md` also matches the WHERE clause.
//! - `LIMIT 1` without `ORDER BY` means SQLite can legally return either row.
//!
//! A correct implementation must escape `_` and `%` in LIKE patterns, and
//! either add an `ORDER BY` or restructure the query so the exact `path_rel`
//! match is tried first.

mod common;

use wiki::index::WikiIndex;

#[test]
fn resolve_page_treats_like_metacharacters_as_literals() {
    let repo = common::FixtureRepo::new();

    // Page whose filename contains a literal underscore.
    repo.write_wiki_md(
        "test_page.md",
        "Underscore Page",
        "A page whose filename has a literal underscore.",
        "Body of the underscore page.",
    );
    repo.git_add("test_page.md");

    // Page whose filename has a hyphen at the same character position.
    // In SQL LIKE the `_` wildcard matches any single character, so
    // `LIKE '%test_page.md%'` matches BOTH paths — the hyphen here also
    // satisfies the `_` wildcard.
    repo.write_wiki_md(
        "test-page.md",
        "Dash Page",
        "A page whose filename has a hyphen at the underscore position.",
        "Body of the dash page.",
    );
    repo.git_add("test-page.md");

    repo.git_commit("add test_page.md and test-page.md");

    let index = WikiIndex::prepare(repo.root.as_path()).expect("prepare");

    // Query with `.md` suffix to enter the `input.contains('/') ||
    // input.ends_with(".md")` guard at L350 and reach the LIKE-based path
    // lookup.  The `_` in the query is unescaped and acts as a LIKE wildcard,
    // matching the `-` in `test-page.md`.
    let page = index
        .resolve_page("test_page.md")
        .expect("resolve_page")
        .expect("expected resolve_page to find a match");

    assert!(
        page.file.ends_with("test_page.md"),
        "expected resolve_page('test_page.md') to return the page whose path \
         contains a literal underscore (test_page.md), got file: {} with title: {} \
         — this means the LIKE wildcard matched a hyphen via _ and/or LIMIT 1 \
         without ORDER BY picked the wrong row",
        page.file,
        page.title,
    );
}
