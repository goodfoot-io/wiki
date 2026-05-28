//! Parity test: asserts the three-pass output matches a frozen expected
//! `(path, source, title)` snapshot. Regression guard called for by
//! `notes/gix-capabilities.md`.

mod common;

use wiki::index::{Source, WikiIndex};

#[test]
fn parity_fixture_three_pass_snapshot() {
    let repo = common::make_parity_fixture();
    let index = WikiIndex::prepare(repo.root.as_path()).expect("WikiIndex::prepare");

    let mut rows = index.debug_dump_paths().expect("debug_dump_paths");
    rows.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));

    let actual: Vec<(String, Source, String)> = rows;

    // Each (path_rel, source) pair is an independent primary-key row.
    // committed.md and staged.md land in Tree+Index+Worktree because the
    // parity fixture commits .gitignore *after* staging staged.md, so both
    // files appear in HEAD. untracked.md and ignored.md have no git history.
    let expected: Vec<(&str, Source, &str)> = vec![
        ("committed.md", Source::Tree,     "Committed Page"),
        ("committed.md", Source::Index,    "Committed Page"),
        ("committed.md", Source::Worktree, "Committed Page"),
        ("ignored.md",   Source::Worktree, "Ignored Page"),
        ("staged.md",    Source::Tree,     "Staged Page"),
        ("staged.md",    Source::Index,    "Staged Page"),
        ("staged.md",    Source::Worktree, "Staged Page"),
        ("untracked.md", Source::Worktree, "Untracked Page"),
    ];

    assert_eq!(actual.len(), expected.len(), "row count: {actual:?}");
    for (got, want) in actual.iter().zip(expected.iter()) {
        assert_eq!(got.0, want.0, "path");
        assert_eq!(got.1, want.1, "source for {}", want.0);
        assert_eq!(got.2, want.2, "title for {}", want.0);
    }
}
