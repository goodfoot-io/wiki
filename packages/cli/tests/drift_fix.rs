//! Integration tests for the drift phase inside `wiki check --fix` (plan
//! Decision 6): relocations, fail-closed counts driving the exit gates,
//! field initialization, and `--print-applied` output.
//!
//! P2 skipped checks — unignored one at a time in P3 as the implementation
//! lands. The composed scenarios mirror the reviewer round-4 sub-blocking
//! case: a `--fix` relocation must leave the post-fix re-check green via the
//! relocation clause (same label + target-range content equality), not exit
//! 1 on its own fix.

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

/// Run `wiki check --fix` (plus extra args) from `cwd`.
fn wiki_check_fix(cwd: &Path, extra: &[&str]) -> Output {
    let bin = env!("CARGO_BIN_EXE_wiki");
    let mut args = vec!["check", "--fix"];
    args.extend_from_slice(extra);
    args.push("**/*.md");
    Command::new(bin)
        .args(&args)
        .current_dir(cwd)
        .output()
        .expect("run wiki check --fix")
}

fn init_repo() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    git(tmp.path(), &["init", "-q", "-b", "main"]);
    tmp
}

/// Write a wiki page (frontmatter + body) under `wiki/<name>`.
fn write_page(root: &Path, name: &str, body: &str) {
    std::fs::create_dir_all(root.join("wiki")).unwrap();
    let content = format!("---\ntitle: {name}\nsummary: A page about {name}.\n---\n\n{body}\n");
    std::fs::write(root.join("wiki").join(name), content).unwrap();
}

/// Write a certified wiki page: frontmatter carrying `links-reviewed: <value>`.
fn write_certified_page(root: &Path, name: &str, value: &str, body: &str) {
    std::fs::create_dir_all(root.join("wiki")).unwrap();
    let content = format!(
        "---\ntitle: {name}\nsummary: A page about {name}.\nlinks-reviewed: {value}\n---\n\n{body}\n"
    );
    std::fs::write(root.join("wiki").join(name), content).unwrap();
}

const BLOCK: &str = "fn canonical() {\n    compute()\n    resolve()\n}\n";

/// Commit the certified fixture: `[code](../src/target.rs#L2-L4)` covering
/// the block at `src/target.rs` lines 2-4, page field `1`.
fn seed_certified(root: &Path) {
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/target.rs"), format!("// preamble\n{BLOCK}")).unwrap();
    write_certified_page(root, "page.md", "1", "See [code](../src/target.rs#L2-L4).");
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "certify"]);
}

/// stdout + stderr concatenated, for content assertions.
fn combined(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

// ── Composed fix → re-check (reviewer round-4 sub-blocking case) ─────────────

/// Cross-file relocation: `--fix` rewrites the href to the moved block, the
/// post-fix re-check certifies it via the relocation clause, and the run
/// exits 0.
#[test]
#[ignore = "P3: drift fix phase implementation"]
fn fix_relocates_cross_file_and_exits_zero() {
    let tmp = init_repo();
    let root = tmp.path();
    seed_certified(root);

    // The block moves verbatim to src/moved.rs; the original target is
    // emptied.
    std::fs::write(root.join("src/target.rs"), "// target emptied\n").unwrap();
    std::fs::write(
        root.join("src/moved.rs"),
        format!("// preamble\n// x\n{BLOCK}"),
    )
    .unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "move block"]);

    let out = wiki_check_fix(root, &[]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "composed relocation must exit 0; got:\n{}",
        combined(&out)
    );
    let page = std::fs::read_to_string(root.join("wiki/page.md")).expect("read page");
    assert!(
        page.contains("../src/moved.rs#L3-L5"),
        "href must be rewritten to the moved block:\n{page}"
    );
    // Re-running the check over the rewritten page stays green.
    let out2 = wiki_check_fix(root, &[]);
    assert_eq!(out2.status.code(), Some(0), "second run stays green");
}

/// Drift with an unreviewed content edit: `--fix` cannot settle it — the
/// skip count drives exit 1 and the bump remedy names the field.
#[test]
#[ignore = "P3: drift fix phase implementation"]
fn fix_drift_with_unreviewed_edit_exits_1() {
    let tmp = init_repo();
    let root = tmp.path();
    seed_certified(root);

    std::fs::write(
        root.join("src/target.rs"),
        "// preamble\nfn canonical() {\n    recompute()\n    resolve()\n}\n",
    )
    .unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "edit block"]);

    let out = wiki_check_fix(root, &[]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "unreviewed drift must exit 1; got:\n{}",
        combined(&out)
    );
    assert!(
        combined(&out).contains("links-reviewed"),
        "skip remedy must name the field"
    );
}

/// Unknown (ambiguous move): `--fix` never first-hit-wins — the unverified
/// count drives exit 1.
#[test]
#[ignore = "P3: drift fix phase implementation"]
fn fix_unknown_ambiguous_move_exits_1() {
    let tmp = init_repo();
    let root = tmp.path();
    seed_certified(root);

    std::fs::write(root.join("src/target.rs"), "// target emptied\n").unwrap();
    std::fs::write(root.join("src/a.rs"), BLOCK).unwrap();
    std::fs::write(root.join("src/b.rs"), BLOCK).unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "duplicate block"]);

    let out = wiki_check_fix(root, &[]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "ambiguous move must exit 1; got:\n{}",
        combined(&out)
    );
    // The href is untouched — nothing was auto-applied.
    let page = std::fs::read_to_string(root.join("wiki/page.md")).expect("read page");
    assert!(
        page.contains("../src/target.rs#L2-L4"),
        "href must not be rewritten on an ambiguous move:\n{page}"
    );
}

/// A field-less page gets `links-reviewed: 1` through `--fix` and the run
/// exits 0; a second run stays green and leaves the value alone.
#[test]
#[ignore = "P3: drift fix phase implementation"]
fn fix_initializes_missing_field_and_exits_zero() {
    let tmp = init_repo();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/target.rs"), format!("// preamble\n{BLOCK}")).unwrap();
    write_page(root, "page.md", "See [code](../src/target.rs#L2-L4).");
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "field-less page"]);

    let out = wiki_check_fix(root, &[]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "field initialization must exit 0; got:\n{}",
        combined(&out)
    );
    let page = std::fs::read_to_string(root.join("wiki/page.md")).expect("read page");
    assert!(
        page.contains("links-reviewed: 1"),
        "field must be initialized:\n{page}"
    );

    let out2 = wiki_check_fix(root, &[]);
    assert_eq!(out2.status.code(), Some(0), "second run stays green");
    let page2 = std::fs::read_to_string(root.join("wiki/page.md")).expect("read page");
    assert_eq!(page, page2, "second run must not rewrite the field");
}

/// `--print-applied` prints exactly the repo-relative paths the run wrote —
/// the rewritten page — with advisories staying off stdout.
#[test]
#[ignore = "P3: drift fix phase implementation"]
fn fix_print_applied_lists_rewritten_paths() {
    let tmp = init_repo();
    let root = tmp.path();
    seed_certified(root);

    std::fs::write(root.join("src/target.rs"), "// target emptied\n").unwrap();
    std::fs::write(
        root.join("src/moved.rs"),
        format!("// preamble\n// x\n{BLOCK}"),
    )
    .unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "move block"]);

    let out = wiki_check_fix(root, &["--print-applied"]);
    assert_eq!(out.status.code(), Some(0), "{}", combined(&out));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        vec!["wiki/page.md"],
        "--print-applied stdout must be exactly the rewritten paths; got:\n{stdout}"
    );
}
