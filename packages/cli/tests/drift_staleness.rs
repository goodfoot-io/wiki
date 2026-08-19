//! Integration tests for the drift pass in `wiki check` (plan Decision 7).
//!
//! The pass is the sole authority for line-range links: each range link is
//! classified against its page's `links-reviewed:` anchor epoch, derived from
//! git history, and every non-Healthy, non-Moved outcome fails the check.
//! These tests pin the CLI-level contract — exit codes, `--source` modes, the
//! pending-bump override, and shallow-clone fail-closed — over synthetic
//! histories; the classification engine itself is covered by the drift.rs
//! unit tests.

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

/// Run `wiki check` with the supplied extra args from `cwd`.
fn wiki_check(cwd: &Path, extra: &[&str]) -> Output {
    let bin = env!("CARGO_BIN_EXE_wiki");
    let mut args = vec!["check"];
    args.extend_from_slice(extra);
    args.push("**/*.md");
    Command::new(bin)
        .args(&args)
        .current_dir(cwd)
        .output()
        .expect("run wiki check")
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

fn init_repo() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    git(tmp.path(), &["init", "-q", "-b", "main"]);
    tmp
}

/// Commit the certified fixture: `[code](/src/lib.rs#L1-L3)` covering
/// `fn foo() {\n    42\n}\n`, page field `1`.
fn seed_certified(root: &Path) {
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "fn foo() {\n    42\n}\n").unwrap();
    write_certified_page(root, "page.md", "1", "See [code](/src/lib.rs#L1-L3).");
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

// ── Drift: committed content change at a certified range ─────────────────────

#[test]
fn drift_committed_content_change_exits_nonzero() {
    let tmp = init_repo();
    let root = tmp.path();
    seed_certified(root);

    // The certified range changes in a new commit; the field value is
    // unchanged, so the anchor epoch stays the certification commit.
    std::fs::write(root.join("src/lib.rs"), "fn foo() {\n    99\n}\n").unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "change"]);

    let out = wiki_check(root, &[]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "committed drift must exit 1; stdout=\n{}\nstderr=\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let text = combined(&out);
    assert!(
        text.contains("bump `links-reviewed:`"),
        "link_drift diagnostic must name the bump remedy; got:\n{text}"
    );

    // The JSON envelope carries the kind verbatim.
    let json = wiki_check(root, &["--format", "json"]);
    assert!(
        combined(&json).contains("\"kind\": \"link_drift\""),
        "JSON envelope must carry kind link_drift; got:\n{}",
        combined(&json)
    );
}

// ── Pending-bump override (Decision 2) ───────────────────────────────────────

#[test]
fn drift_pending_bump_suppresses_certification_outcomes() {
    let tmp = init_repo();
    let root = tmp.path();
    seed_certified(root);

    // Uncommitted: the target drifts AND the field bumps. The anchor epoch IS
    // the current state, so Uncertified/Drift are suppressed — the check
    // passes and `--fix` would do no certification work.
    std::fs::write(root.join("src/lib.rs"), "fn foo() {\n    99\n}\n").unwrap();
    write_certified_page(root, "page.md", "2", "See [code](/src/lib.rs#L1-L3).");

    let out = wiki_check(root, &[]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "pending bump must suppress drift; stdout=\n{}\nstderr=\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn drift_pending_bump_still_flags_broken_targets() {
    let tmp = init_repo();
    let root = tmp.path();
    seed_certified(root);

    // Uncommitted: field bump AND the target deleted. Broken is structural —
    // the pending-bump override never silences it.
    write_certified_page(root, "page.md", "2", "See [code](/src/lib.rs#L1-L3).");
    std::fs::remove_file(root.join("src/lib.rs")).unwrap();

    let out = wiki_check(root, &[]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "broken target must flag under a pending bump; stdout=\n{}\nstderr=\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let json = wiki_check(root, &["--format", "json"]);
    assert!(
        combined(&json).contains("\"kind\": \"link_broken\""),
        "JSON envelope must carry kind link_broken; got:\n{}",
        combined(&json)
    );
}

// ── Uncertified: a locator absent from the anchor epoch ──────────────────────

#[test]
fn drift_new_link_without_bump_is_uncertified() {
    let tmp = init_repo();
    let root = tmp.path();
    seed_certified(root);

    // A second link appears without a field bump; the locator was never
    // reviewed, so the check must not bless it.
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/other.rs"), "fn b() {}\n").unwrap();
    write_certified_page(
        root,
        "page.md",
        "1",
        "See [code](/src/lib.rs#L1-L3).\nSee [other](/src/other.rs#L1-L1).",
    );
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "add link"]);

    let json = wiki_check(root, &["--format", "json"]);
    assert_eq!(json.status.code(), Some(1), "uncertified link must exit 1");
    assert!(
        combined(&json).contains("\"kind\": \"link_uncertified\""),
        "JSON envelope must carry kind link_uncertified; got:\n{}",
        combined(&json)
    );
}

// ── Exit-code plumbing ───────────────────────────────────────────────────────

#[test]
fn drift_no_exit_code_suppresses() {
    let tmp = init_repo();
    let root = tmp.path();
    seed_certified(root);

    std::fs::write(root.join("src/lib.rs"), "fn foo() {\n    99\n}\n").unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "change"]);

    let out = wiki_check(root, &["--no-exit-code"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "--no-exit-code suppresses the drift exit; stderr=\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ── The field is only demanded of pages with range links ─────────────────────

#[test]
fn drift_pages_without_range_links_exit_zero() {
    let tmp = init_repo();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "fn foo() {}\n").unwrap();
    write_page(root, "page.md", "See [code](/src/lib.rs).");
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "seed"]);

    let out = wiki_check(root, &[]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "a page with no range links needs no field; stdout=\n{}\nstderr=\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

// ── `--source` plumbing ──────────────────────────────────────────────────────

#[test]
fn drift_source_head_ignores_dirty_worktree() {
    let tmp = init_repo();
    let root = tmp.path();
    seed_certified(root);

    // Uncommitted drift at the certified range.
    std::fs::write(root.join("src/lib.rs"), "fn foo() {\n    99\n}\n").unwrap();

    let head = wiki_check(root, &["--source=head"]);
    assert_eq!(
        head.status.code(),
        Some(0),
        "HEAD content is unchanged → exit 0; stderr=\n{}",
        String::from_utf8_lossy(&head.stderr)
    );

    let work = wiki_check(root, &["--source=worktree"]);
    assert_eq!(
        work.status.code(),
        Some(1),
        "worktree content drifted → exit 1; stderr=\n{}",
        String::from_utf8_lossy(&work.stderr)
    );
}

// ── Broken targets ───────────────────────────────────────────────────────────

#[test]
fn drift_deleted_target_exits_nonzero() {
    let tmp = init_repo();
    let root = tmp.path();
    seed_certified(root);

    git(root, &["rm", "-q", "src/lib.rs"]);
    git(root, &["commit", "-q", "-m", "delete target"]);

    let out = wiki_check(root, &[]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "deleted target must exit 1; stdout=\n{}\nstderr=\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let json = wiki_check(root, &["--format", "json"]);
    assert!(
        combined(&json).contains("\"kind\": \"link_broken\""),
        "JSON envelope must carry kind link_broken; got:\n{}",
        combined(&json)
    );
}

// ── Fail-closed: missing field, shallow clones ───────────────────────────────

#[test]
fn drift_range_link_without_field_exits_nonzero() {
    let tmp = init_repo();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "fn foo() {}\n").unwrap();
    write_page(root, "page.md", "See [code](/src/lib.rs#L1-L1).");
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "seed"]);

    let out = wiki_check(root, &[]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "a range link without a field must fail closed; stdout=\n{}\nstderr=\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined(&out).contains("links-reviewed"),
        "anchor_epoch_missing must name the field; got:\n{}",
        combined(&out)
    );
}

#[test]
fn drift_shallow_clone_fails_closed() {
    let tmp = init_repo();
    let root = tmp.path();
    seed_certified(root);

    // A real shallow clone — the `file://` transport matters: git ignores
    // `--depth` on local-path clones and copies full history.
    let dst = tempfile::tempdir().unwrap();
    let src = format!("file://{}", root.display());
    let status = Command::new("git")
        .args(["clone", "-q", "--depth", "1"])
        .arg(&src)
        .arg(dst.path())
        .status()
        .expect("spawn git clone");
    assert!(status.success(), "git clone failed");

    let out = wiki_check(dst.path(), &[]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "shallow clone must fail closed with exit 2; stdout=\n{}\nstderr=\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined(&out).contains("shallow"),
        "shallow-clone error must be surfaced; got:\n{}",
        combined(&out)
    );
}

// ── Rename tracking ──────────────────────────────────────────────────────────

#[test]
fn drift_page_rename_keeps_certification() {
    let tmp = init_repo();
    let root = tmp.path();
    seed_certified(root);

    // Rename the page without touching the field: the anchor walk follows the
    // rename (R### rows) and resolves the anchor-side page under its old name.
    git(root, &["mv", "wiki/page.md", "wiki/renamed.md"]);
    git(root, &["commit", "-q", "-m", "rename page"]);

    let out = wiki_check(root, &[]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "a renamed certified page must stay Healthy; stdout=\n{}\nstderr=\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
