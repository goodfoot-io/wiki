//! Loop-closing tests for the `wiki mesh` reconciliation commands that
//! `wiki check` emits in its skip/advisory output.
//!
//! The card's core promise: every `wiki check` failure names an exact,
//! copy-pasteable `wiki mesh` command, and running that command verbatim makes
//! a follow-up check pass. These tests drive the real binary to prove the two
//! highest-risk classes actually close the loop:
//!
//! - F1 (no coverage): the emitted `wiki mesh add <slug> <wiki-file>
//!   <code-anchor> --why "…"` anchors BOTH the wiki page and the code target in
//!   one mesh — which is exactly what the coverage rule requires.
//! - F2 (ambiguous shift): the emitted add-then-remove pair succeeds for a
//!   single-anchor mesh WITHOUT an `--why` error and preserves the rationale.

use std::path::Path;
use std::process::{Command, Output};

fn git(workdir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(workdir)
        .env("GIT_AUTHOR_NAME", "Test Author")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "Test Author")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .status()
        .expect("git available");
    assert!(status.success(), "git {args:?} failed");
}

fn wiki(cwd: &Path, args: &[&str]) -> Output {
    let bin = env!("CARGO_BIN_EXE_wiki");
    Command::new(bin)
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("run wiki")
}

/// F1: the no-coverage skip command anchors BOTH the wiki page and the code
/// target in one mesh, so running it verbatim creates coverage that satisfies
/// `is_covered` (same mesh anchors page + target). We run the exact emitted
/// command shape and assert the resulting mesh file contains both anchors.
#[test]
fn no_coverage_add_command_anchors_both_targets() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("wiki")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "// l1\n// l2\n// l3\n").unwrap();
    std::fs::write(
        root.join("wiki/page.md"),
        "---\ntitle: Page\nsummary: A page.\n---\n\nSee [lib](../src/lib.rs#L1-L3).\n",
    )
    .unwrap();
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "init"]);

    // Run the exact command shape that the F1 skip emits:
    //   wiki mesh add <slug> <wiki-file> <code-anchor> --why "<rationale>"
    let out = wiki(
        root,
        &[
            "mesh",
            "add",
            "wiki/page-lib",
            "wiki/page.md",
            "src/lib.rs#L1-L3",
            "--why",
            "Page documents lib.",
        ],
    );
    assert!(
        out.status.success(),
        "add must succeed; stderr=\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let mesh = std::fs::read_to_string(root.join(".wiki/wiki/page-lib")).unwrap();
    assert!(
        mesh.contains("wiki/page.md"),
        "mesh must anchor the wiki page:\n{mesh}"
    );
    assert!(
        mesh.contains("src/lib.rs"),
        "mesh must anchor the code target:\n{mesh}"
    );
}

/// F2 + F8: the ambiguous-shift skip emits add-then-remove. For a mesh whose
/// only anchor is the stale one, the add must run while the mesh still exists
/// (so no `--why` is needed) and the remove must then drop the stale anchor —
/// the rationale survives throughout. Running the pair leaves exactly the new
/// anchor, exit 0, with the original `why` intact.
#[test]
fn ambiguous_shift_add_then_remove_single_anchor_preserves_why() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), "// l1\n// l2\n// l3\n// l4\n").unwrap();
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "init"]);

    // Seed a single-anchor mesh (the stale anchor) with a curated rationale.
    let seed = wiki(
        root,
        &[
            "mesh",
            "add",
            "m",
            "src/a.rs#L1-L2",
            "--why",
            "Curated rationale.",
        ],
    );
    assert!(seed.status.success());

    // The emitted F2 sequence: add the new anchor FIRST (no --why needed because
    // the mesh still exists), then remove the stale one.
    let add = wiki(root, &["mesh", "add", "m", "src/a.rs#L3-L4"]);
    assert!(
        add.status.success(),
        "add-without-why must succeed on an existing mesh; stderr=\n{}",
        String::from_utf8_lossy(&add.stderr)
    );

    let remove = wiki(root, &["mesh", "remove", "m", "src/a.rs#L1-L2"]);
    assert!(
        remove.status.success(),
        "remove of the stale anchor must succeed; stderr=\n{}",
        String::from_utf8_lossy(&remove.stderr)
    );

    let mesh = std::fs::read_to_string(root.join(".wiki/m")).unwrap();
    assert!(
        mesh.contains("Curated rationale."),
        "rationale must survive the add-then-remove sequence:\n{mesh}"
    );
    assert!(
        mesh.contains("L3-L4") && !mesh.contains("L1-L2"),
        "only the new anchor must remain:\n{mesh}"
    );
}

/// F8: `remove` is idempotent — removing an already-absent anchor (e.g. re-running
/// the F2 sequence) exits 0 with a `nothing to remove` notice.
#[test]
fn remove_missing_anchor_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), "// l1\n").unwrap();
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "init"]);

    wiki(root, &["mesh", "add", "m", "src/a.rs", "--why", "Why."]);

    // Anchor not present.
    let out = wiki(root, &["mesh", "remove", "m", "src/a.rs#L1-L1"]);
    assert!(out.status.success(), "missing anchor remove must exit 0");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("nothing to remove"),
        "must print a nothing-to-remove notice"
    );

    // Mesh not present.
    let out = wiki(root, &["mesh", "remove", "no-such-mesh"]);
    assert!(out.status.success(), "missing mesh remove must exit 0");
}

/// F4 + F6: an anchor-less `wiki mesh add <slug> --why "…"` updates the rationale
/// of an existing mesh and announces the overwrite naming the previous value.
#[test]
fn anchorless_why_updates_rationale_with_overwrite_notice() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), "// l1\n").unwrap();
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "init"]);

    wiki(
        root,
        &["mesh", "add", "m", "src/a.rs", "--why", "Original."],
    );

    let out = wiki(root, &["mesh", "add", "m", "--why", "Revised."]);
    assert!(
        out.status.success(),
        "anchor-less --why must succeed; stderr=\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("was: \"Original.\""),
        "overwrite must name the previous rationale; stderr=\n{stderr}"
    );

    let mesh = std::fs::read_to_string(root.join(".wiki/m")).unwrap();
    assert!(
        mesh.contains("Revised."),
        "rationale must be updated:\n{mesh}"
    );
}

/// F6: anchor-less `--why` against a non-existent mesh is an error (nothing to
/// curate).
#[test]
fn anchorless_why_on_missing_mesh_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q", "-b", "main"]);
    let out = wiki(root, &["mesh", "add", "nope", "--why", "x"]);
    assert!(
        !out.status.success(),
        "curating a non-existent mesh must error"
    );
}

/// F5: a batch `add` with a malformed/out-of-bounds later anchor leaves NO
/// partial mesh on disk.
#[test]
fn batch_add_is_atomic_on_bad_anchor() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), "// l1\n// l2\n").unwrap();
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "init"]);

    // Second anchor is out of bounds (file has 2 lines).
    let out = wiki(
        root,
        &[
            "mesh",
            "add",
            "m",
            "src/a.rs#L1-L1",
            "src/a.rs#L1-L9",
            "--why",
            "Why.",
        ],
    );
    assert!(!out.status.success(), "out-of-bounds anchor must error");
    assert!(
        !root.join(".wiki/m").exists(),
        "no partial mesh may be left on disk after a failed batch add"
    );
}

/// F9: `--format json` is rejected on mesh subcommands (fail closed).
#[test]
fn format_json_rejected_on_mesh() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q", "-b", "main"]);
    let out = wiki(root, &["--format", "json", "mesh", "show", "whatever"]);
    assert!(
        !out.status.success(),
        "mesh + --format json must fail closed"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("does not support --format json"),
        "must explain JSON is unsupported"
    );
}
