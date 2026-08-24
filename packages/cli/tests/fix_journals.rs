//! Integration tests for fix journals (plan merged-store-generations D8):
//! crash-recovery for `wiki check --fix` multi-file materialization.
//!
//! The contract under test:
//!
//! * Killing `check --fix` mid-run and rerunning replays from the journal to
//!   the SAME final state as an uninterrupted run (interrupted-run
//!   equivalence).
//! * Expired (>7-day TTL) or corrupt journals (unparseable manifest, stage
//!   sha mismatch, scope-digest mismatch) are deleted and the pass
//!   recomputes cleanly — never partial application.
//! * `--fix-dry-run` leaves working files untouched and writes no journal.
//! * Clean delivery removes the journal directory.
//! * At most ONE stderr warning line per run across all replay/expiry
//!   events.
//!
//! Journals are staged manually (the exact bytes a killed run would have
//! left behind): stage blobs `blob-N`, `manifest.json` status `prepared`,
//! scope digest computed with the same length-tagged framing production
//! uses (`u64` LE byte length + UTF-8 bytes per field, fields in sorted
//! `(path_rel, sha256)` pair order, repo identity last).
//!
//! Landed per tdd-bootstrap (phases compressed): the checks below are the
//! executable specification of D8's journal state machine.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use wiki::cache::key::sha256_hex;

// ── harness ───────────────────────────────────────────────────────────────────

fn git(workdir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(workdir)
        .env("GIT_AUTHOR_NAME", "Test Author")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "Test Author")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed in {workdir:?}");
}

/// Run `wiki check --fix` (plus extra args) from `cwd`.
///
/// `WIKI_ANCHOR_CACHE=0` isolates the assertion surface: the anchor cache is
/// disposable by contract, and disabling it guarantees no unrelated
/// `warning:` line can pollute the journal-warning counting.
fn wiki_check_fix(cwd: &Path, extra: &[&str]) -> Output {
    let bin = env!("CARGO_BIN_EXE_wiki");
    let mut args = vec!["check", "--fix"];
    args.extend_from_slice(extra);
    args.push("**/*.md");
    Command::new(bin)
        .args(&args)
        .current_dir(cwd)
        .env("WIKI_ANCHOR_CACHE", "0")
        .output()
        .expect("run wiki check --fix")
}

fn combined(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn stderr_text(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn warning_line_count(out: &Output) -> usize {
    stderr_text(out)
        .lines()
        .filter(|l| l.starts_with("warning:"))
        .count()
}

fn init_repo() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    git(tmp.path(), &["init", "-q", "-b", "main"]);
    tmp
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_millis() as u64
}

const DAY_MS: u64 = 24 * 60 * 60 * 1000;

// ── fixture ───────────────────────────────────────────────────────────────────

fn write_page(root: &Path, rel: &str, title: &str, body: &str) {
    let abs = root.join(rel);
    std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
    std::fs::write(
        &abs,
        format!("---\ntitle: {title}\nsummary: A page about {title}.\n---\n\n{body}\n"),
    )
    .unwrap();
}

/// Commit two wiki pages linking `../docs/old.md`, then commit the rename
/// `docs/old.md` → `docs/new.md`. Both pages now carry broken links whose
/// unique rename successor exists — the deterministic Fix #1 scenario.
fn seed_rename_fixture(root: &Path) {
    write_page(root, "docs/old.md", "Old", "The old target page.");
    write_page(root, "wiki/a.md", "A", "See [alpha](../docs/old.md).");
    write_page(root, "wiki/b.md", "B", "See [beta](../docs/old.md).");
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "seed"]);
    git(root, &["mv", "docs/old.md", "docs/new.md"]);
    git(root, &["commit", "-q", "-m", "rename target"]);
}

/// The content a completed fix produces for a seeded page.
fn fixed_content(content: &str) -> String {
    content.replace("../docs/old.md", "../docs/new.md")
}

fn read_page(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).expect("read page")
}

// ── manual journal staging ────────────────────────────────────────────────────

/// Length-tagged framing mirror of `crate::cache::key`'s canonical encoding:
/// u64 LE byte length + bytes per field.
fn push_field(out: &mut Vec<u8>, field: &str) {
    out.extend_from_slice(&(field.len() as u64).to_le_bytes());
    out.extend_from_slice(field.as_bytes());
}

/// The scope digest production computes: SHA-256 over sorted
/// `(path_rel, target_sha256)` pairs plus the repo identity string, all
/// length-tagged in pair order.
fn scope_digest(entries: &[(&str, &str)], identity: &str) -> String {
    let mut sorted: Vec<(String, String)> = entries
        .iter()
        .map(|(p, c)| (p.to_string(), sha256_hex(c.as_bytes())))
        .collect();
    sorted.sort();
    let mut out = Vec::new();
    for (p, s) in &sorted {
        push_field(&mut out, p);
        push_field(&mut out, s);
    }
    push_field(&mut out, identity);
    sha256_hex(&out)
}

/// The repository identity string production hashes: the resolved common
/// git dir, absolutized against the repo root.
fn repo_identity(root: &Path) -> String {
    let out = Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .current_dir(root)
        .output()
        .expect("git rev-parse --git-common-dir");
    assert!(out.status.success(), "rev-parse failed");
    let resolved = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let path = PathBuf::from(&resolved);
    let abs = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };
    abs.to_string_lossy().into_owned()
}

/// The per-worktree journal root production uses: `<dot-git>/wiki/journal`.
fn journal_root(root: &Path) -> PathBuf {
    root.join(".git").join("wiki").join("journal")
}

struct StagedJournal {
    /// Absolute path of the staged `<scope16>` directory.
    dir: PathBuf,
}

/// Stage a `prepared` journal exactly as a killed run would have left it:
/// stage blobs written first, `manifest.json` last. `digest_override`
/// simulates corruption of the recorded scope digest; `created_at_ms`
/// backdates the journal for TTL tests.
fn stage_prepared_journal(
    root: &Path,
    entries: &[(&str, &str)],
    created_at_ms: u64,
    digest_override: Option<String>,
) -> StagedJournal {
    assert!(!entries.is_empty(), "journals always have entries");
    let identity = repo_identity(root);
    let digest = scope_digest(entries, &identity);

    let mut sorted: Vec<(String, String, String)> = entries
        .iter()
        .map(|(p, c)| (p.to_string(), sha256_hex(c.as_bytes()), c.to_string()))
        .collect();
    sorted.sort();

    let scope16 = &digest[..16];
    let dir = journal_root(root).join(scope16);
    // Production journals live in fd-hardened 0700 subtrees; a killed run
    // can only ever leave private directories behind, so the fixture does
    // too (the recompute pass refuses non-private journal ground).
    for path in [journal_root(root), dir.clone()] {
        std::fs::create_dir_all(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    let mut json_entries = Vec::new();
    for (i, (path_rel, sha256, content)) in sorted.iter().enumerate() {
        let stage_file = format!("blob-{i}");
        std::fs::write(dir.join(&stage_file), content.as_bytes()).unwrap();
        json_entries.push(serde_json::json!({
            "path_rel": path_rel,
            "stage_file": stage_file,
            "sha256": sha256,
        }));
    }

    let manifest = serde_json::json!({
        "version": 1,
        "created_at": created_at_ms,
        "status": "prepared",
        "scope_digest": digest_override.unwrap_or_else(|| digest.clone()),
        "entries": json_entries,
    });
    // Manifest written last, like production.
    std::fs::write(dir.join("manifest.json"), serde_json::to_vec(&manifest).unwrap()).unwrap();
    StagedJournal { dir }
}

// ── checks ────────────────────────────────────────────────────────────────────

/// Interrupted-run equivalence: a prepared journal with one of its two
/// targets already applied (the arbitrary partial subset a kill leaves
/// behind) replays on the next run to byte-identical final state as an
/// uninterrupted run. The replay consumes the pending fixes before fresh
/// planning, so the rerun reports nothing new to fix and emits no warning.
#[test]
fn interrupted_run_replays_to_uninterrupted_final_state() {
    // Uninterrupted reference run.
    let reference = init_repo();
    seed_rename_fixture(reference.path());
    let out = wiki_check_fix(reference.path(), &[]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "uninterrupted run exits 0:\n{}",
        combined(&out)
    );
    let ref_a = read_page(reference.path(), "wiki/a.md");
    let ref_b = read_page(reference.path(), "wiki/b.md");

    // Interrupted twin: journal staged, target A applied, target B not.
    let interrupted = init_repo();
    seed_rename_fixture(interrupted.path());
    let content_a = read_page(interrupted.path(), "wiki/a.md");
    let content_b = read_page(interrupted.path(), "wiki/b.md");
    let fixed_a = fixed_content(&content_a);
    let fixed_b = fixed_content(&content_b);
    let staged = stage_prepared_journal(
        interrupted.path(),
        &[("wiki/a.md", &fixed_a), ("wiki/b.md", &fixed_b)],
        now_ms(),
        None,
    );
    // The kill residue: an arbitrary subset rewritten on disk.
    std::fs::write(interrupted.path().join("wiki/a.md"), &fixed_a).unwrap();

    let out = wiki_check_fix(interrupted.path(), &[]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "rerun after interruption must succeed:\n{}",
        combined(&out)
    );
    assert!(
        !combined(&out).contains("fixed:"),
        "replay must consume the pending fixes before planning; got:\n{}",
        combined(&out)
    );
    assert_eq!(
        warning_line_count(&out),
        0,
        "a valid replay emits no warning:\n{}",
        stderr_text(&out)
    );
    assert_eq!(read_page(interrupted.path(), "wiki/a.md"), ref_a);
    assert_eq!(read_page(interrupted.path(), "wiki/b.md"), ref_b);
    // Sanity: the reference really did rewrite both pages.
    assert_eq!(ref_a, fixed_a, "reference run rewrote a.md");
    assert_eq!(ref_b, fixed_b, "reference run rewrote b.md");
    assert_eq!(ref_a, fixed_content(&read_page(reference.path(), "wiki/a.md")));
    // Clean delivery: the replayed journal directory is gone.
    assert!(
        !staged.dir.exists(),
        "replayed journal directory must be removed"
    );
}

/// An expired journal (>7 days) is deleted and the pass recomputes cleanly:
/// the broken pages are fixed by a fresh pass, exactly one warning line
/// fires, and no journal directory survives.
#[test]
fn expired_journal_is_discarded_and_fix_recomputes() {
    let tmp = init_repo();
    let root = tmp.path();
    seed_rename_fixture(root);

    let content_a = read_page(root, "wiki/a.md");
    let content_b = read_page(root, "wiki/b.md");
    let staged = stage_prepared_journal(
        root,
        &[
            ("wiki/a.md", &fixed_content(&content_a)),
            ("wiki/b.md", &fixed_content(&content_b)),
        ],
        now_ms() - 8 * DAY_MS,
        None,
    );

    let out = wiki_check_fix(root, &[]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "expired-journal run must recompute and succeed:\n{}",
        combined(&out)
    );
    assert_eq!(read_page(root, "wiki/a.md"), fixed_content(&content_a));
    assert_eq!(read_page(root, "wiki/b.md"), fixed_content(&content_b));
    assert!(
        !staged.dir.exists(),
        "expired journal directory must be removed"
    );
    assert_eq!(
        warning_line_count(&out),
        1,
        "exactly one expiry warning expected:\n{}",
        stderr_text(&out)
    );
}

/// A journal whose stage blob no longer hash-matches its recorded sha256 is
/// corrupt: it is deleted and the pass recomputes cleanly instead of
/// writing corrupt bytes into working files.
#[test]
fn corrupt_stage_journal_is_discarded_and_fix_recomputes() {
    let tmp = init_repo();
    let root = tmp.path();
    seed_rename_fixture(root);

    let content_a = read_page(root, "wiki/a.md");
    let content_b = read_page(root, "wiki/b.md");
    let staged = stage_prepared_journal(
        root,
        &[
            ("wiki/a.md", &fixed_content(&content_a)),
            ("wiki/b.md", &fixed_content(&content_b)),
        ],
        now_ms(),
        None,
    );

    // Corrupt one stage blob AFTER staging (crash-residue simulation):
    // its bytes no longer match the recorded sha256.
    std::fs::write(staged.dir.join("blob-1"), b"torn write\0garbage").unwrap();

    let out = wiki_check_fix(root, &[]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "corrupt-stage run must recompute and succeed:\n{}",
        combined(&out)
    );
    assert_eq!(read_page(root, "wiki/a.md"), fixed_content(&content_a));
    assert_eq!(read_page(root, "wiki/b.md"), fixed_content(&content_b));
    assert!(
        !staged.dir.exists(),
        "corrupt journal directory must be removed"
    );
    assert_eq!(
        warning_line_count(&out),
        1,
        "exactly one corruption warning expected:\n{}",
        stderr_text(&out)
    );
}

/// A journal whose recorded scope digest does not match a recomputation
/// from its own entries is treated as corrupt: removed and recomputed,
/// never applied.
#[test]
fn digest_mismatch_journal_is_removed_and_recomputed() {
    let tmp = init_repo();
    let root = tmp.path();
    seed_rename_fixture(root);

    let content_a = read_page(root, "wiki/a.md");
    let content_b = read_page(root, "wiki/b.md");
    let staged = stage_prepared_journal(
        root,
        &[
            ("wiki/a.md", &fixed_content(&content_a)),
            ("wiki/b.md", &fixed_content(&content_b)),
        ],
        now_ms(),
        Some("0".repeat(64)),
    );

    let out = wiki_check_fix(root, &[]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "digest-mismatch run must recompute and succeed:\n{}",
        combined(&out)
    );
    assert_eq!(read_page(root, "wiki/a.md"), fixed_content(&content_a));
    assert_eq!(read_page(root, "wiki/b.md"), fixed_content(&content_b));
    assert!(
        !staged.dir.exists(),
        "mismatched journal directory must be removed"
    );
    assert_eq!(
        warning_line_count(&out),
        1,
        "exactly one mismatch warning expected:\n{}",
        stderr_text(&out)
    );
}

/// `--fix-dry-run` is side-effect-free for journals: working files stay
/// broken and no journal directory is written anywhere.
#[test]
fn dry_run_touches_no_files_and_writes_no_journal() {
    let tmp = init_repo();
    let root = tmp.path();
    seed_rename_fixture(root);

    let before_a = read_page(root, "wiki/a.md");
    let before_b = read_page(root, "wiki/b.md");

    let out = wiki_check_fix(root, &["--fix-dry-run"]);
    assert_eq!(out.status.code(), Some(1), "dry run reports pending work");
    assert!(
        combined(&out).contains("../docs/new.md"),
        "dry run prints the proposed rewrites:\n{}",
        combined(&out)
    );
    assert_eq!(read_page(root, "wiki/a.md"), before_a, "dry run must not rewrite");
    assert_eq!(read_page(root, "wiki/b.md"), before_b, "dry run must not rewrite");
    assert!(
        !journal_root(root).exists(),
        "dry run must not create the journal root"
    );
}

/// A normal (non-interrupted) fix materializes through a journal and clean
/// delivery removes it: after the run the journal root holds no directories
/// and a second run stays green.
#[test]
fn clean_delivery_removes_journal_dir() {
    let tmp = init_repo();
    let root = tmp.path();
    seed_rename_fixture(root);

    let out = wiki_check_fix(root, &[]);
    assert_eq!(out.status.code(), Some(0), "first run succeeds:\n{}", combined(&out));
    assert!(
        combined(&out).contains("fixed:"),
        "first run reports its fixes:\n{}",
        combined(&out)
    );
    let jroot = journal_root(root);
    let leftover_dirs: Vec<_> = std::fs::read_dir(&jroot)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        leftover_dirs.is_empty(),
        "clean delivery must remove every journal directory; found {leftover_dirs:?}"
    );

    let out2 = wiki_check_fix(root, &[]);
    assert_eq!(out2.status.code(), Some(0), "second run stays green");
}

/// Multiple stale journals in one run collapse into AT MOST ONE stderr
/// warning line total — the once-per-run budget mirrors the anchor cache
/// reporter's first-call-wins pattern.
#[test]
fn replay_emits_at_most_one_warning_line() {
    let tmp = init_repo();
    let root = tmp.path();
    seed_rename_fixture(root);

    let content_a = read_page(root, "wiki/a.md");
    let content_b = read_page(root, "wiki/b.md");
    let stale = now_ms() - 8 * DAY_MS;
    // Two distinct scopes, both expired.
    let j1 = stage_prepared_journal(
        root,
        &[
            ("wiki/a.md", &fixed_content(&content_a)),
            ("wiki/b.md", &fixed_content(&content_b)),
        ],
        stale,
        None,
    );
    let j2 = stage_prepared_journal(root, &[("wiki/a.md", &fixed_content(&content_a))], stale, None);
    assert_ne!(j1.dir, j2.dir, "distinct scopes stage distinct directories");

    let out = wiki_check_fix(root, &[]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "both stale journals discarded, fix recomputes:\n{}",
        combined(&out)
    );
    assert_eq!(read_page(root, "wiki/a.md"), fixed_content(&content_a));
    assert_eq!(read_page(root, "wiki/b.md"), fixed_content(&content_b));
    let warnings = warning_line_count(&out);
    assert!(
        warnings <= 1,
        "at most one warning line per run allowed; got {warnings}:\n{}",
        stderr_text(&out)
    );
    assert_eq!(
        warnings,
        1,
        "discarding two stale journals warns exactly once"
    );
    assert!(
        !j1.dir.exists() && !j2.dir.exists(),
        "both stale journals are removed"
    );
}
