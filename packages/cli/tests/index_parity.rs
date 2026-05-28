//! Parity test: asserts the three-pass output matches a frozen expected JSON
//! snapshot. This is the regression guard called for by `notes/gix-capabilities.md`.

mod common;

use wiki::index::WikiIndex;

/// Frozen snapshot of the expected three-pass output for the parity fixture.
///
/// Each entry captures `{ path, source, title }` for the four fixture files.
/// The `blob_oid` field is intentionally omitted from the snapshot because it
/// is content-derived and stable; Phase 3 will add an OID column once the
/// real compute path lands.
const EXPECTED_PARITY: &str = r#"[
  {"path": "committed.md",  "source": "Tree",     "title": "Committed Page"},
  {"path": "ignored.md",    "source": "Worktree",  "title": "Ignored Page"},
  {"path": "staged.md",     "source": "Index",     "title": "Staged Page"},
  {"path": "untracked.md",  "source": "Worktree",  "title": "Untracked Page"}
]"#;

#[test]
#[ignore = "tdd-bootstrap"]
fn parity_fixture_three_pass_snapshot() {
    let repo = common::make_parity_fixture();

    // Phase 3 wires this up; for now the body panics.
    let _index = WikiIndex::prepare(repo.root.as_path())
        .expect("WikiIndex::prepare");

    // In Phase 3 we will expose a method that dumps the `paths` table as JSON
    // and compare it against EXPECTED_PARITY after sorting by path.
    let _ = EXPECTED_PARITY;
    unimplemented!("Phase 3: drive the three-pass walk and compare snapshot")
}
