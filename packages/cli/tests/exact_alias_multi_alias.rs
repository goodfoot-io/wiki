//! Reproduction test: the [exact-alias search stage](./packages/cli/src/index/search.rs#L59-L90)
//! in [`search_weighted`](./packages/cli/src/index/search.rs#L48-L54) compares
//! `lower(b.aliases_text) = ?1` — the entire space-joined alias list — against
//! the query, so a page with aliases `[Foo, Bar]` whose `aliases_text` is
//! `"Foo Bar"` never matches a query of `"foo"` in the exact short-circuit
//! stage.  The page falls through to the FTS stage, which *does* find it, so
//! the page still surfaces — but with snippets extracted from body text rather
//! than with the empty snippets that signal an exact match.
//!
//! After the fix the exact-alias stage must do token-wise matching (split
//! `aliases_text` on whitespace, compare each token individually, as
//! [`resolve_page`](./packages/cli/src/index/search.rs#L246-L252) already
//! does), so a single-alias query against a multi-alias page matches in stage 1
//! and produces a result with empty snippets.

mod common;

use wiki::index::WikiIndex;

#[test]
fn exact_alias_matches_token_wise_for_multi_alias_pages() {
    let repo = common::FixtureRepo::new();

    // Page with multiple aliases; the query token "UniqueAlias" appears ONLY
    // in the aliases field — not in title, summary, or body.  This isolates
    // the exact-alias stage: a title match is impossible, and while FTS *will*
    // reach this page (it indexes aliases_text), the stage-1 exact-match
    // result (empty snippets) is the observable difference vs an FTS-only
    // fallback (body-text snippets).
    repo.write_file(
        "multi_alias.md",
        "---\ntitle: Multi Alias Page\nsummary: A page with multiple aliases.\naliases:\n  - UniqueAlias\n  - OtherAlias\n---\n\nBody text for the multi-alias page.\n",
    );
    repo.git_add("multi_alias.md");
    repo.git_commit("add multi-alias page");

    let index = WikiIndex::prepare(repo.root.as_path()).expect("prepare");
    let (results, _total) = index
        .search_weighted("UniqueAlias", 10, 0)
        .expect("search_weighted");

    assert_eq!(
        results.len(),
        1,
        "expected exactly one result for UniqueAlias query; got {}: {results:?}",
        results.len()
    );

    let result = &results[0];
    assert_eq!(result.title, "Multi Alias Page");

    // The key assertion: an exact-alias match produces empty snippets (stage 1
    // pushes before FTS and dedup guards against overwrite).  Non-empty
    // snippets mean the page only matched via FTS fallback — the exact-alias
    // stage failed.
    assert!(
        result.snippets.is_empty(),
        "expected empty snippets (exact-alias match); got {:?} — page matched via FTS fallback",
        result.snippets
    );
}
