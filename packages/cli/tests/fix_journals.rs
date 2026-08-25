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

// ── F4: --print-applied must report replay-written files ─────────────────────

/// F4 control (a): a staged journal whose targets differ from disk is
/// replayed by the run; `--print-applied` must list every file the replay
/// physically wrote — scripts pipe this list into `git add`.
#[test]
fn print_applied_lists_replay_written_files() {
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

    let out = wiki_check_fix(root, &["--print-applied"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "replay run exits 0:\n{}",
        combined(&out)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut lines: Vec<&str> = stdout.lines().collect();
    lines.sort();
    assert_eq!(
        lines,
        vec!["wiki/a.md", "wiki/b.md"],
        "--print-applied must list every replay-written file; got:\n{stdout}"
    );
    assert_eq!(read_page(root, "wiki/a.md"), fixed_content(&content_a));
    assert_eq!(read_page(root, "wiki/b.md"), fixed_content(&content_b));
    assert!(
        !staged.dir.exists(),
        "the replayed journal must be consumed"
    );
}

/// F4 control (b): targets already at target bytes are pure idempotent
/// skips — the journal is consumed silently and `--print-applied` lists
/// nothing, because this run touched no working file.
#[test]
fn print_applied_excludes_idempotent_skips() {
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
    // The kill residue landed EVERYTHING before dying.
    std::fs::write(root.join("wiki/a.md"), fixed_content(&content_a)).unwrap();
    std::fs::write(root.join("wiki/b.md"), fixed_content(&content_b)).unwrap();

    let out = wiki_check_fix(root, &["--print-applied"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "all-skip replay exits 0:\n{}",
        combined(&out)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.trim().is_empty(),
        "--print-applied must exclude pure idempotent skips; got:\n{stdout}"
    );
    assert!(!staged.dir.exists(), "the journal is still consumed");
}

/// F4 discriminator (partial subset): the journal covers two files but only
/// B's bytes were left unapplied — `--print-applied` lists exactly B, not
/// the skipped A and not nothing.
#[test]
fn print_applied_lists_only_the_replay_written_subset() {
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
    // A was rewritten before the kill; B was not.
    std::fs::write(root.join("wiki/a.md"), fixed_content(&content_a)).unwrap();

    let out = wiki_check_fix(root, &["--print-applied"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "partial-subset replay exits 0:\n{}",
        combined(&out)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        stdout.lines().collect::<Vec<_>>(),
        vec!["wiki/b.md"],
        "--print-applied must list exactly the replay-written subset; got:\n{stdout}"
    );
    assert!(!staged.dir.exists(), "journal consumed");
}

// ── F5: dry-run previews against post-replay reality ─────────────────────────

/// F5 control (a): with a pending journal satisfying every pending fix, the
/// dry-run preview proposes NOTHING (a real run's replay would satisfy it
/// all), while touching neither working files nor journal state.
#[test]
fn dry_run_preview_reflects_pending_journal() {
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

    let out = wiki_check_fix(root, &["--fix-dry-run"]);
    let text = combined(&out);
    assert!(
        !text.contains("fix:"),
        "preview must not propose fixes the pending replay satisfies:\n{text}"
    );
    // Nothing written: both pages stay broken on disk.
    assert_eq!(read_page(root, "wiki/a.md"), content_a);
    assert_eq!(read_page(root, "wiki/b.md"), content_b);
    // The journal survives the preview untouched.
    assert!(staged.dir.exists(), "dry run must not consume the journal");
}

/// F5 control (b): preview ≈ execution. A third page outside the journal's
/// scope keeps one genuine fix pending; the dry-run proposes exactly that
/// fix, and the following real run reports exactly it too (plus completing
/// the replay). Final state converges to fully fixed.
#[test]
fn dry_run_preview_matches_subsequent_real_run() {
    let tmp = init_repo();
    let root = tmp.path();
    // Fixture + a third linking page committed BEFORE the rename, so its
    // link breaks identically but stays outside the journal scope below.
    write_page(root, "docs/old.md", "Old", "The old target page.");
    write_page(root, "wiki/a.md", "A", "See [alpha](../docs/old.md).");
    write_page(root, "wiki/b.md", "B", "See [beta](../docs/old.md).");
    write_page(root, "wiki/c.md", "C", "See [gamma](../docs/old.md).");
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "seed"]);
    git(root, &["mv", "docs/old.md", "docs/new.md"]);
    git(root, &["commit", "-q", "-m", "rename target"]);

    let content_a = read_page(root, "wiki/a.md");
    let content_b = read_page(root, "wiki/b.md");
    let content_c = read_page(root, "wiki/c.md");
    let _staged = stage_prepared_journal(
        root,
        &[
            ("wiki/a.md", &fixed_content(&content_a)),
            ("wiki/b.md", &fixed_content(&content_b)),
        ],
        now_ms(),
        None,
    );

    // Preview: only c.md's fix remains proposable.
    let preview = wiki_check_fix(root, &["--fix-dry-run"]);
    let preview_text = combined(&preview);
    assert!(
        preview_text.contains("fix: wiki/c.md"),
        "preview must propose exactly the out-of-journal fix:\n{preview_text}"
    );
    assert!(
        !preview_text.contains("wiki/a.md") && !preview_text.contains("wiki/b.md"),
        "preview must not propose journal-satisfied fixes:\n{preview_text}"
    );

    // Execution: reports exactly the same fix, completes the replay, and
    // converges everything.
    let real = wiki_check_fix(root, &[]);
    assert_eq!(
        real.status.code(),
        Some(0),
        "real run exits 0:\n{}",
        combined(&real)
    );
    let real_text = combined(&real);
    assert!(
        real_text.contains("fixed: wiki/c.md"),
        "real run must report exactly the previewed fix:\n{real_text}"
    );
    assert!(
        !real_text.contains("fixed: wiki/a.md") && !real_text.contains("fixed: wiki/b.md"),
        "real run must not re-report replay-satisfied fixes:\n{real_text}"
    );
    assert_eq!(read_page(root, "wiki/a.md"), fixed_content(&content_a));
    assert_eq!(read_page(root, "wiki/b.md"), fixed_content(&content_b));
    assert_eq!(read_page(root, "wiki/c.md"), fixed_content(&content_c));
}

// ── F-C: dry-run diagnostic layer + exit gate converge with pending replay ───

/// Extract every output line mentioning `rel`, sorted — the comparable
/// diagnostic surface across runs.
fn diagnostic_lines(out: &Output, rel: &str) -> Vec<String> {
    let mut lines: Vec<String> = combined(out)
        .lines()
        .filter(|l| l.contains(rel))
        .map(str::to_owned)
        .collect();
    lines.sort();
    lines
}

/// F-C control (a): a prepared journal holding the fixed pages must make the
/// dry run's diagnostics AND exit gate describe the execution it previews —
/// no stale broken-link lines, exit 0, "no fixes to apply" — while touching
/// neither working files nor journal state. The following real run replays
/// and exits 0 too.
#[test]
fn dry_run_exit_gate_converges_with_pending_journal() {
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

    let preview = wiki_check_fix(root, &["--fix-dry-run"]);
    let text = combined(&preview);
    assert_eq!(
        preview.status.code(),
        Some(0),
        "dry-run exit gate must reflect post-replay reality:\n{text}"
    );
    assert!(
        !text.contains("Broken Link") && !text.contains("broken_link"),
        "stale pre-replay diagnostics must not leak into the preview:\n{text}"
    );
    assert!(
        !text.contains("fix:"),
        "preview proposes nothing when replay satisfies everything:\n{text}"
    );
    // Nothing written; journal intact for the real run.
    assert_eq!(read_page(root, "wiki/a.md"), content_a);
    assert_eq!(read_page(root, "wiki/b.md"), content_b);
    assert!(staged.dir.exists(), "dry run must not consume the journal");

    let real = wiki_check_fix(root, &[]);
    assert_eq!(
        real.status.code(),
        Some(0),
        "the following real run exits 0 via replay:\n{}",
        combined(&real)
    );
    assert!(!staged.dir.exists(), "real run consumed the journal");
}

/// F-C control (b): the journaled file carries MULTIPLE diagnostic classes
/// pre-fix — broken_link from a renamed target plus broken_anchor on an
/// existing target whose heading was renamed (both auto-fixable, so both
/// land in one staged journal). A third page outside journal scope keeps a
/// permanent broken_link. The dry run must report neither stale class for
/// the journaled file while reporting the out-of-scope breakage exactly as
/// execution does; exit gates agree.
#[test]
fn dry_run_diagnostics_match_real_run_post_replay() {
    let tmp = init_repo();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("docs")).unwrap();
    std::fs::create_dir_all(root.join("wiki")).unwrap();
    // Two target pages whose `Setup guide` heading will be renamed; old.md
    // is ALSO renamed as a file (broken_link class), guide.md stays put
    // (broken_anchor class). gone.md is deleted outright with no successor.
    let target_v1 = "---\ntitle: T\nsummary: T.\n---\n\n## Setup guide\n\nbody\n";
    std::fs::write(root.join("docs/old.md"), target_v1).unwrap();
    std::fs::write(root.join("docs/guide.md"), target_v1).unwrap();
    std::fs::write(
        root.join("docs/gone.md"),
        "---\ntitle: Gone\nsummary: Gone.\n---\n\nbody\n",
    )
    .unwrap();
    std::fs::write(
        root.join("wiki/a.md"),
        "---\ntitle: A\nsummary: A.\n---\n\nSee [alpha](../docs/old.md) and [guide](../docs/guide.md#setup-guide).\n",
    )
    .unwrap();
    std::fs::write(
        root.join("wiki/c.md"),
        "---\ntitle: C\nsummary: C.\n---\n\nSee [gamma](../docs/gone.md).\n",
    )
    .unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "seed"]);
    git(root, &["mv", "docs/old.md", "docs/new.md"]);
    std::fs::write(
        root.join("docs/new.md"),
        "---\ntitle: T\nsummary: T.\n---\n\n## Install\n\nbody\n",
    )
    .unwrap();
    std::fs::write(
        root.join("docs/guide.md"),
        "---\ntitle: Guide\nsummary: Guide.\n---\n\n## Install\n\nbody\n",
    )
    .unwrap();
    git(root, &["rm", "-q", "docs/gone.md"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "rename file, rename heading, delete"]);

    // Exactly what an uninterrupted killed run would have staged: both
    // classes fixed in one content.
    let fixed_a = "---\ntitle: A\nsummary: A.\n---\n\nSee [alpha](../docs/new.md) and [guide](../docs/guide.md#install).\n";
    let _staged = stage_prepared_journal(root, &[("wiki/a.md", fixed_a)], now_ms(), None);

    // Preview: neither of a.md's stale classes appears; c.md's genuine
    // breakage remains as a skip proposal; gate exits 1 like execution.
    let preview = wiki_check_fix(root, &["--fix-dry-run"]);
    let preview_text = combined(&preview);
    assert_eq!(
        preview.status.code(),
        Some(1),
        "out-of-journal breakage keeps the preview at exit 1:\n{preview_text}"
    );
    assert!(
        !preview_text.contains("wiki/a.md"),
        "journaled file's stale diagnostics must not appear:\n{preview_text}"
    );
    assert!(
        preview_text.contains("skip: wiki/c.md")
            && preview_text.contains("no successor in git history"),
        "out-of-journal breakage must still be reported:\n{preview_text}"
    );

    // Execution: replays a.md, reports the same c.md breakage through its
    // own channels, same gate outcome.
    let real = wiki_check_fix(root, &[]);
    assert_eq!(
        real.status.code(),
        Some(1),
        "c.md keeps the real run at exit 1:\n{}",
        combined(&real)
    );
    let real_text = combined(&real);
    assert!(
        !real_text.contains("wiki/a.md"),
        "replay-satisfied file must not reappear in execution output:\n{real_text}"
    );
    assert!(
        real_text.contains("skipped: wiki/c.md:6")
            && real_text.contains("no successor in git history"),
        "execution must report the same c.md reason:\n{real_text}"
    );
    // The journaled file landed on disk through replay.
    assert_eq!(read_page(root, "wiki/a.md"), fixed_a);
}
