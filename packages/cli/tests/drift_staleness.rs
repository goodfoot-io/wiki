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

// ── RangeDiffered: a hand-edited href, the certified block untouched ─────────

/// A deliberate re-point of the href with the certified block untouched is
/// RangeDiffered, not Drift: the range no longer points at the certified
/// block, so "content changed since the anchor epoch" would misdescribe it.
/// The check reports it (kind `link_drift`, exit 1) and `--fix` skips it —
/// the href stays byte-untouched; only a `links-reviewed` bump settles it.
#[test]
fn drift_repointed_href_prints_range_message_and_skips_fix() {
    let tmp = init_repo();
    let root = tmp.path();
    seed_certified(root);

    // Re-point the href to a different range; the certified block at L1-L3
    // never moves.
    std::fs::write(
        root.join("wiki/page.md"),
        "---\ntitle: page\nsummary: A page about page.\nlinks-reviewed: 1\n---\n\n\
         See [code](/src/lib.rs#L1-L1).\n",
    )
    .unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "re-point href"]);

    let out = wiki_check(root, &[]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "range-differed href must exit 1; stdout=\n{}\nstderr=\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let text = combined(&out);
    assert!(
        text.contains("the link's range no longer points at the certified block"),
        "RangeDiffered diagnostic must name the range remedy; got:\n{text}"
    );
    assert!(
        !text.contains("changed since the anchor epoch"),
        "RangeDiffered must not reuse the in-place Drift message; got:\n{text}"
    );

    // `--fix` skips the link: the href must stay byte-untouched.
    let page = std::fs::read_to_string(root.join("wiki/page.md")).unwrap();
    let fix_out = wiki_check(root, &["--fix"]);
    assert_eq!(
        fix_out.status.code(),
        Some(1),
        "unreviewed range edit must keep exiting 1; got:\n{}",
        combined(&fix_out)
    );
    assert!(
        combined(&fix_out).contains("the link's range no longer points at the certified block"),
        "--fix skip must name the range remedy; got:\n{}",
        combined(&fix_out)
    );
    assert_eq!(
        std::fs::read_to_string(root.join("wiki/page.md")).unwrap(),
        page,
        "--fix must leave the re-pointed href byte-untouched"
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

/// Control for the `--no-exit-code` escape hatch (witness W5): the same
/// shallow clone reports the error on stderr but exits 0 under the flag.
/// The flag's contract is "report but never fail", so it must gate the
/// hard-error arms (the shallow-clone EpochError lands in the collect
/// arms) exactly as it gates validation errors.
#[test]
fn drift_shallow_clone_no_exit_code_reports_and_exits_zero() {
    let tmp = init_repo();
    let root = tmp.path();
    seed_certified(root);

    let dst = tempfile::tempdir().unwrap();
    let src = format!("file://{}", root.display());
    let status = Command::new("git")
        .args(["clone", "-q", "--depth", "1"])
        .arg(&src)
        .arg(dst.path())
        .status()
        .expect("spawn git clone");
    assert!(status.success(), "git clone failed");

    let out = wiki_check(dst.path(), &["--no-exit-code"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "--no-exit-code must suppress the hard-error exit; stdout=\n{}\nstderr=\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined(&out).contains("shallow"),
        "the shallow-clone error must still be surfaced; got:\n{}",
        combined(&out)
    );

    // Under `--format json` the same suppressed hard error prints a JSON
    // envelope on stderr instead of the plain text line.
    let outj = wiki_check(dst.path(), &["--no-exit-code", "--format", "json"]);
    assert_eq!(
        outj.status.code(),
        Some(0),
        "json mode must also respect --no-exit-code; stdout=\n{}\nstderr=\n{}",
        String::from_utf8_lossy(&outj.stdout),
        String::from_utf8_lossy(&outj.stderr)
    );
    let errj = String::from_utf8_lossy(&outj.stderr);
    assert!(
        errj.contains("\"error\"") && errj.contains("shallow"),
        "json mode must print the error envelope; got:\n{errj}"
    );
}

/// Control for the empty-corpus decision: with no pages matched, the
/// "no wiki pages found" fatal diagnostic still prints, and `--no-exit-code`
/// gates its exit the same way it gates every other hard error.
#[test]
fn drift_empty_corpus_no_exit_code_reports_and_exits_zero() {
    let tmp = init_repo();
    let root = tmp.path();

    let out = wiki_check(root, &["--no-exit-code"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "--no-exit-code must suppress the empty-corpus exit; stdout=\n{}\nstderr=\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined(&out).contains("no wiki pages found"),
        "the empty-corpus diagnostic must still be surfaced; got:\n{}",
        combined(&out)
    );

    // Without the flag the empty corpus keeps failing closed.
    let out2 = wiki_check(root, &[]);
    assert_eq!(
        out2.status.code(),
        Some(2),
        "empty corpus must keep exit 2 without the flag"
    );
}

// ── Unparseable YAML: not a value, not an epoch event ─────────────────────────

/// Witness W6 (finding yaml-breakage-rebaselines): a commit that breaks the
/// page's YAML is not a value and not an epoch event. After a repair commit
/// that adds a new line-range link WITHOUT bumping the field, the anchor
/// stays at the newest readable value change — the repair commit cannot
/// silently re-certify links no human reviewed, and the new link classifies
/// Uncertified (exit 1).
#[test]
fn drift_broken_yaml_repair_cannot_reanchor() {
    let tmp = init_repo();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "fn foo() {\n    42\n}\n").unwrap();
    write_certified_page(root, "page.md", "1", "See [a](/src/lib.rs#L1-L3).");
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "certify"]);

    // Commit B breaks the YAML block entirely.
    std::fs::write(
        root.join("wiki/page.md"),
        "---\ntitle: [unclosed\nlinks-reviewed: 1\n---\n\nSee [a](/src/lib.rs#L1-L3).\n",
    )
    .unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "break yaml"]);

    // Commit C repairs the YAML with the SAME field value and adds a new
    // line-range link. Under the old conflating read, the broken commit
    // parsed as `None`, so the (C, B) pair "differed" and C re-anchored at
    // itself — silently certifying a link no human reviewed.
    write_certified_page(
        root,
        "page.md",
        "1",
        "See [a](/src/lib.rs#L1-L3) and [b](/src/lib.rs#L2-L2).",
    );
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "repair, no bump"]);

    let out = wiki_check(root, &[]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "the un-reviewed link must classify Uncertified; stdout=\n{}\nstderr=\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let both = combined(&out);
    assert!(
        both.contains("/src/lib.rs#L2-L2")
            && both.contains("not present at the page's anchor epoch"),
        "the new link must be flagged as un-reviewed at the true anchor; got:\n{both}"
    );
}

/// Control for the repair-cannot-reanchor witness: a genuine bump DOES
/// anchor at the bump commit, so a link added in the same commit as the
/// bump is certified by it.
#[test]
fn drift_bump_anchors_at_the_bump_commit() {
    let tmp = init_repo();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "fn foo() {\n    42\n}\n").unwrap();
    write_certified_page(root, "page.md", "1", "See [a](/src/lib.rs#L1-L3).");
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "field 1"]);
    write_certified_page(
        root,
        "page.md",
        "2",
        "See [a](/src/lib.rs#L1-L3) and [b](/src/lib.rs#L2-L2).",
    );
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "bump to 2, add link"]);

    let out = wiki_check(root, &[]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "a bumped page certifies the link added with the bump; got:\n{}",
        combined(&out)
    );
}

/// Control: when no pair of readable values differs, the anchor is the
/// oldest readable commit — the field's introduction.
#[test]
fn drift_field_introduction_anchors_the_epoch() {
    let tmp = init_repo();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "fn foo() {\n    42\n}\n").unwrap();
    write_page(root, "page.md", "See [a](/src/lib.rs#L1-L3).");
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "no field yet"]);
    write_certified_page(root, "page.md", "1", "See [a](/src/lib.rs#L1-L3).");
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "field introduced"]);
    write_certified_page(root, "page.md", "1", "See [a](/src/lib.rs#L1-L3) — body edit only.");
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "body edit"]);

    let out = wiki_check(root, &[]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "the introduction commit anchors the epoch and certifies; got:\n{}",
        combined(&out)
    );
}

/// Decision control for the unparseable current side (lead tiebreak: fail
/// closed — an explicit error, never a silent pass): a page whose CURRENT
/// YAML cannot be parsed errors with exit 2 in both read-only and `--fix`
/// modes, and `--fix` leaves the page untouched (broken YAML is a human
/// edit, not an auto-repair).
#[test]
fn drift_unparseable_current_yaml_fails_closed() {
    let tmp = init_repo();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "fn foo() {\n    42\n}\n").unwrap();
    write_certified_page(root, "page.md", "1", "See [a](/src/lib.rs#L1-L3).");
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "certify"]);
    std::fs::write(
        root.join("wiki/page.md"),
        "---\ntitle: [unclosed\nlinks-reviewed: 1\n---\n\nSee [a](/src/lib.rs#L1-L3).\n",
    )
    .unwrap();

    let out = wiki_check(root, &[]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "an unparseable current page must fail closed; stdout=\n{}\nstderr=\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined(&out).contains("unparseable YAML frontmatter"),
        "the error must name the broken YAML; got:\n{}",
        combined(&out)
    );

    let before = std::fs::read_to_string(root.join("wiki/page.md")).unwrap();
    let out_fix = wiki_check(root, &["--fix"]);
    assert_eq!(
        out_fix.status.code(),
        Some(2),
        "--fix must fail closed too, never initializing the field on broken YAML; \
         stdout=\n{}\nstderr=\n{}",
        String::from_utf8_lossy(&out_fix.stdout),
        String::from_utf8_lossy(&out_fix.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(root.join("wiki/page.md")).unwrap(),
        before,
        "--fix must not touch a page with unparseable YAML"
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
