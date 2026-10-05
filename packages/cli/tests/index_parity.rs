//! Parity test: asserts the three-pass output matches a frozen expected
//! `(path, source, title)` snapshot. Regression guard called for by
//! `notes/gix-capabilities.md`.

mod common;

use wiki::index::{DocSource, WikiIndex};

#[test]
fn parity_fixture_three_pass_snapshot() {
    let repo = common::make_parity_fixture();
    WikiIndex::prepare_for_source(repo.root.as_path(), DocSource::WorkingTree)
        .expect("WikiIndex::prepare_for_source");

    let actual = common::served_path_rows(repo.root.as_path());

    // Each (path_rel, source) pair is an independent primary-key row.
    // committed.md and staged.md land in Tree+Index+Worktree because the
    // parity fixture commits .gitignore *after* staging staged.md, so both
    // files appear in HEAD. untracked.md and ignored.md have no git history.
    // Rows are ordered by path, then by the stored source literal.
    let expected: Vec<(&str, &str, &str)> = vec![
        ("committed.md", "index", "Committed Page"),
        ("committed.md", "tree", "Committed Page"),
        ("committed.md", "worktree", "Committed Page"),
        ("ignored.md", "worktree", "Ignored Page"),
        ("staged.md", "index", "Staged Page"),
        ("staged.md", "tree", "Staged Page"),
        ("staged.md", "worktree", "Staged Page"),
        ("untracked.md", "worktree", "Untracked Page"),
    ];

    assert_eq!(actual.len(), expected.len(), "row count: {actual:?}");
    for (got, want) in actual.iter().zip(expected.iter()) {
        assert_eq!(got.0, want.0, "path");
        assert_eq!(got.1, want.1, "source for {}", want.0);
        assert_eq!(got.2, want.2, "title for {}", want.0);
    }
}
