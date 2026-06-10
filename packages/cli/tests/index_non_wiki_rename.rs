//! A committed pure rename of a frontmatter-less (non-wiki) `.md` file must
//! be a silent no-op for the index. A non-wiki markdown file has no `blobs`
//! row and no `paths` row — `apply_add` skips it. When the Tree pass
//! classifies the rename as a pure rename (same blob OID, is_markdown on both
//! sides), `apply_rename` issues `INSERT OR REPLACE INTO paths` referencing a
//! blob OID with no `blobs` row. The FK constraint `paths.oid REFERENCES
//! blobs(oid)` aborts the refresh with "FOREIGN KEY constraint failed", and
//! the rolled-back transaction means the state row never advances past the
//! bad commit — every subsequent invocation retries the same delta and fails.
//!
//! Expected: the rename is a no-op and the index refresh succeeds.

mod common;

use wiki::index::WikiIndex;

#[test]
fn pure_rename_of_non_wiki_md_is_index_noop() {
    let repo = common::FixtureRepo::new();

    // A wiki page so the index has valid content to build.
    repo.write_wiki_md(
        "alpha.md",
        "Alpha Page",
        "Summary of alpha.",
        "Body text for alpha.",
    );

    // A non-wiki markdown file — no YAML frontmatter, just plain text.
    repo.write_file("plain.md", "Just a plain markdown file.\n");

    repo.git_add("alpha.md");
    repo.git_add("plain.md");
    repo.git_commit("add alpha.md and plain.md");

    // First prepare succeeds: alpha.md is indexed; plain.md is silently
    // skipped because it has no `title`+`summary` frontmatter.
    drop(WikiIndex::prepare(repo.root.as_path()).expect("first prepare"));

    // Pure rename of the non-wiki file — same content, same blob OID.
    repo.git_mv("plain.md", "renamed-plain.md");
    repo.git_commit("rename plain.md -> renamed-plain.md");

    // This should succeed (rename is a no-op for non-wiki files), but under
    // the current code it fails with FOREIGN KEY constraint failed.
    WikiIndex::prepare(repo.root.as_path()).expect("second prepare after non-wiki rename");
}
