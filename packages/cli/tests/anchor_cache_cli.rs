//! CLI-surface acceptance for the anchor cache (plan decisions 2, 7, 8):
//! `--clear-cache`, the `WIKI_ANCHOR_CACHE=0` kill-switch oracle, the
//! single-fault diagnostic line, and the fix-mode single-fault guard.
//!
//! Written in tdd-bootstrap Phase 2: every check is `#[ignore]`d against
//! today's P1 stubs (`git::common_dir()` resolves to an empty path and the
//! store's `clear()` is a no-op) and unskipped one at a time in T3 as the
//! real resolution and store land. The store's own contract lives in
//! [anchor_cache_core.rs](anchor_cache_core.rs); this file covers the CLI
//! surface that consumes it.
//!
//! Fault forcing: a run "resolves no common dir" by setting `GIT_DIR` to a
//! nonexistent path. `git::repo_root()` uses plain gix discovery, which
//! ignores `GIT_DIR`, so the check still runs; Phase 1's `git::common_dir()`
//! uses `discover_with_environment_overrides`, which fails closed on a bogus
//! `GIT_DIR` (spike S2) — exactly the divergence the fault line is designed
//! for. The fault fixtures carry no line-range links, so the drift engine's
//! git subprocesses never run and the bogus `GIT_DIR` disturbs nothing else.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod common;

/// Spawn the wiki binary from `cwd` with a clean cache environment: both
/// `GIT_DIR` and `WIKI_ANCHOR_CACHE` are stripped so a run only sees the
/// settings a test explicitly applies.
fn wiki(cwd: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_wiki"));
    cmd.current_dir(cwd)
        .env_remove("GIT_DIR")
        .env_remove("WIKI_ANCHOR_CACHE")
        .args(args);
    cmd
}

/// Run `wiki <args>` from `cwd` and collect the output.
fn run(cwd: &Path, args: &[&str]) -> Output {
    wiki(cwd, args).output().expect("run wiki")
}

/// The cache directory the CLI must print and delete: `<common-dir>/wiki`
/// (plan decision 2), derived from git itself so the assertion survives
/// layout changes (a linked worktree resolves a different common dir).
fn expected_cache_dir(repo_root: &Path) -> PathBuf {
    let out = Command::new("git")
        .current_dir(repo_root)
        .args(["rev-parse", "--git-common-dir"])
        .output()
        .expect("git rev-parse --git-common-dir");
    assert!(
        out.status.success(),
        "git rev-parse failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let common = String::from_utf8_lossy(&out.stdout).trim().to_string();
    repo_root.join(common).join("wiki")
}

/// Every line of `stderr` that is the cache-fault warning (plan decision 7).
fn fault_warnings(stderr: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(stderr)
        .lines()
        .filter(|l| l.starts_with("warning: anchor cache unavailable ("))
        .map(str::to_owned)
        .collect()
}

/// `stderr` with the cache-fault warning lines removed — the fault warning
/// is the only difference a faulted run may have over its baseline.
fn non_fault_stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .filter(|l| !l.starts_with("warning: anchor cache unavailable ("))
        .collect::<Vec<_>>()
        .join("\n")
}

fn trim_stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A fixture whose pages carry no line-range links: the drift engine's git
/// subprocesses never run, so a bogus `GIT_DIR` disturbs nothing but
/// `git::common_dir()` — the fault the fault-line tests force.
fn plain_fixture() -> common::FixtureRepo {
    let repo = common::FixtureRepo::new();
    repo.write_wiki_md("wiki/page.md", "Page", "A plain page.", "Body without links.");
    repo.write_wiki_md(
        "wiki/other.md",
        "Other",
        "Another plain page.",
        "See [Page](page.md).",
    );
    repo.git_add("wiki/page.md");
    repo.git_add("wiki/other.md");
    repo.git_commit("add pages");
    repo
}

/// A certified line-range link (the drift_fix.rs `seed_certified` pattern):
/// classification runs the anchor walk and the fingerprint in every run, so
/// the kill-switch oracle exercises the real drift seams rather than an
/// empty corpus.
fn certified_fixture() -> common::FixtureRepo {
    let repo = common::FixtureRepo::new();
    std::fs::create_dir_all(repo.root.join("src")).expect("create src");
    std::fs::write(
        repo.root.join("src/target.rs"),
        "// preamble\nfn canonical() {\n    compute()\n    resolve()\n}\n",
    )
    .expect("write target");
    repo.write_file(
        "wiki/page.md",
        "---\ntitle: Page\nsummary: A page about the block.\nlinks-reviewed: 1\n---\n\nSee [code](../src/target.rs#L2-L4).\n",
    );
    repo.git_add("src/target.rs");
    repo.git_add("wiki/page.md");
    repo.git_commit("certify");
    repo
}

// ── --clear-cache (plan decision 8) ──────────────────────────────────────────

/// `wiki check --clear-cache` prints the resolved `<common-dir>/wiki` path
/// on stdout and exits 0 — whether or not the cache directory exists.
/// Pending against the P1 stub (which prints the empty common dir): the
/// real path only exists once Phase 1 resolves `git::common_dir()`.
#[test]
fn clear_cache_prints_resolved_cache_path_and_exits_zero() {
    let repo = plain_fixture();
    let expected = expected_cache_dir(&repo.root);

    let first = run(&repo.root, &["check", "--clear-cache"]);
    assert_eq!(
        first.status.code(),
        Some(0),
        "clear is best-effort: exits 0 even with nothing to delete"
    );
    assert_eq!(
        trim_stdout(&first),
        expected.display().to_string(),
        "stdout must be exactly the resolved cache directory path"
    );
    assert!(
        non_fault_stderr(&first).is_empty(),
        "a resolvable common dir must not warn: {}",
        non_fault_stderr(&first)
    );

    // The cache directory may or may not exist; the clear path behaves the
    // same either way.
    std::fs::create_dir_all(&expected).expect("create cache dir");
    std::fs::write(expected.join("anchor-cache.sqlite"), "stale bytes").expect("write db");
    std::fs::write(expected.join("anchor-cache.init.lock"), "").expect("write lock");

    let second = run(&repo.root, &["check", "--clear-cache"]);
    assert_eq!(second.status.code(), Some(0));
    assert_eq!(
        trim_stdout(&second),
        expected.display().to_string(),
        "stdout must be exactly the resolved cache directory path"
    );
}

/// `--clear-cache` deletes the cache directory itself, not just the
/// database file inside it. Stays ignored until Phase 1's real store
/// `clear()` lands — the P1 stub is a no-op, so this check would fail
/// today.
#[test]
fn clear_cache_deletes_the_cache_directory() {
    let repo = plain_fixture();
    let cache_dir = expected_cache_dir(&repo.root);
    std::fs::create_dir_all(&cache_dir).expect("create cache dir");
    std::fs::write(cache_dir.join("anchor-cache.sqlite"), "stale bytes").expect("write db");
    std::fs::write(cache_dir.join("anchor-cache.init.lock"), "").expect("write lock");
    assert!(cache_dir.exists());

    let out = run(&repo.root, &["check", "--clear-cache"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "clear must exit 0 whether or not anything was deleted"
    );
    assert!(
        !cache_dir.exists(),
        "clear must remove the cache directory itself (database, sidecars, and dir)"
    );
}

/// With no resolvable common dir (the cwd is inside a repo but `GIT_DIR`
/// points at a nonexistent path — see the module doc), `--clear-cache`
/// exits 0, prints no path, and reports the fault exactly once on stderr:
/// best-effort, and the fault line is never a counted diagnostic.
#[test]
fn clear_cache_with_unresolvable_common_dir_warns_once_and_prints_nothing() {
    let repo = plain_fixture();
    let mut cmd = wiki(&repo.root, &["check", "--clear-cache"]);
    cmd.env("GIT_DIR", repo.root.join("bogus-git-dir"));
    let out = cmd.output().expect("run wiki check --clear-cache");

    assert_eq!(
        out.status.code(),
        Some(0),
        "clear is best-effort: a resolution fault must not change the exit code"
    );
    assert!(
        trim_stdout(&out).is_empty(),
        "no common dir means no path to print; got: {}",
        trim_stdout(&out)
    );
    assert_eq!(
        fault_warnings(&out.stderr).len(),
        1,
        "exactly one cache-fault warning, first fault only: {:?}",
        fault_warnings(&out.stderr)
    );
    assert!(
        non_fault_stderr(&out).is_empty(),
        "the warning must be the only stderr line: {}",
        non_fault_stderr(&out)
    );
}

/// `--format json` + `--clear-cache` still prints the plain path on stdout
/// — the clear path never emits a JSON envelope (plan decision 8).
#[test]
fn clear_cache_json_mode_still_prints_the_plain_path() {
    let repo = plain_fixture();
    let expected = expected_cache_dir(&repo.root);

    let out = run(&repo.root, &["check", "--format", "json", "--clear-cache"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        trim_stdout(&out),
        expected.display().to_string(),
        "the clear path must print the plain path, not a JSON envelope"
    );
}

// ── Kill-switch oracle (plan decision 8) ─────────────────────────────────────

/// `WIKI_ANCHOR_CACHE=0` vs unset over the certified fixture: byte-identical
/// stdout, stderr, and exit code — cold cache, kill switch, and warm cache
/// (which serves rows) all agree. The full-corpus oracle is the
/// drift-wiring peer's test; this is the oracle-lite on a small fixture.
#[test]
fn kill_switch_check_is_byte_identical_to_cache_on() {
    let repo = certified_fixture();

    let mut runs = Vec::new();
    // Cache on, cold: lookups miss, writes land.
    runs.push(run(&repo.root, &["check"]));
    // Kill switch: NoopCache, no store at all.
    let mut switched = wiki(&repo.root, &["check"]);
    switched.env("WIKI_ANCHOR_CACHE", "0");
    runs.push(switched.output().expect("run wiki check (kill switch)"));
    // Cache on, warm: rows serve from the store.
    runs.push(run(&repo.root, &["check"]));

    for pair in runs.windows(2) {
        assert_eq!(
            pair[0].stdout, pair[1].stdout,
            "cache on/off must not change stdout"
        );
        assert_eq!(
            pair[0].stderr, pair[1].stderr,
            "cache on/off must not change stderr"
        );
        assert_eq!(
            pair[0].status.code(),
            pair[1].status.code(),
            "cache on/off must not change the exit code"
        );
    }

    // The same equivalence holds for the JSON envelope.
    let on_json = run(&repo.root, &["check", "--format", "json"]);
    let mut off_json = wiki(&repo.root, &["check", "--format", "json"]);
    off_json.env("WIKI_ANCHOR_CACHE", "0");
    let off_json = off_json.output().expect("run wiki check --format json (kill switch)");
    assert_eq!(on_json.stdout, off_json.stdout, "json stdout must be byte-identical");
    assert_eq!(on_json.stderr, off_json.stderr, "json stderr must be byte-identical");
    assert_eq!(on_json.status.code(), off_json.status.code(), "json exit code must match");
}

// ── Fault line (plan decision 7) ─────────────────────────────────────────────

/// A forced fault (bogus `GIT_DIR`): exactly one `warning: anchor cache
/// unavailable (` line on stderr, stdout byte-identical to the kill-switch
/// run, exit code unchanged. The fault warning is never a counted
/// diagnostic — it must not touch stdout or the exit code.
#[test]
fn check_forced_fault_warns_once_and_keeps_stdout_and_exit_identical() {
    let repo = plain_fixture();

    let mut faulted = wiki(&repo.root, &["check"]);
    faulted.env("GIT_DIR", repo.root.join("bogus-git-dir"));
    let faulted = faulted.output().expect("run wiki check (faulted)");

    let mut switched = wiki(&repo.root, &["check"]);
    switched.env("WIKI_ANCHOR_CACHE", "0");
    let switched = switched.output().expect("run wiki check (kill switch)");

    assert_eq!(
        faulted.status.code(),
        switched.status.code(),
        "a cache fault must not change the exit code"
    );
    assert_eq!(
        faulted.stdout, switched.stdout,
        "a cache fault must not touch stdout"
    );
    assert_eq!(
        fault_warnings(&faulted.stderr).len(),
        1,
        "exactly one cache-fault warning, first fault only: {:?}",
        fault_warnings(&faulted.stderr)
    );
    assert_eq!(
        fault_warnings(&switched.stderr).len(),
        0,
        "the kill switch is silent by design"
    );
    assert_eq!(
        non_fault_stderr(&faulted),
        non_fault_stderr(&switched),
        "the warning must be the only stderr difference"
    );
}

/// With `--format json` and a forced fault, stdout stays a valid JSON
/// envelope — the warning goes to stderr only, so the envelope shape is
/// untouched.
#[test]
fn check_forced_fault_json_stdout_remains_valid_json() {
    let repo = plain_fixture();
    let mut cmd = wiki(&repo.root, &["check", "--format", "json"]);
    cmd.env("GIT_DIR", repo.root.join("bogus-git-dir"));
    let out = cmd.output().expect("run wiki check --format json (faulted)");

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let parsed: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout is not JSON: {e}\nstdout: {stdout}"));
    assert!(
        parsed.is_object(),
        "expected JSON object envelope, got: {stdout}"
    );
    assert!(
        parsed.get("errors").is_some(),
        "missing `errors` key; got: {stdout}"
    );
    assert_eq!(
        fault_warnings(&out.stderr).len(),
        1,
        "exactly one cache-fault warning: {:?}",
        fault_warnings(&out.stderr)
    );
}

// ── Fix-mode single fault (plan decision 7) ──────────────────────────────────

/// `--fix` with a forced fault: the fix phase and the post-fix re-check
/// each construct their own cache handle, but the shared per-run guard
/// keeps the warning at exactly one line across both construction sites.
#[test]
fn fix_mode_forced_fault_warns_at_most_once_across_phases() {
    let repo = plain_fixture();

    let mut faulted = wiki(&repo.root, &["check", "--fix"]);
    faulted.env("GIT_DIR", repo.root.join("bogus-git-dir"));
    let faulted = faulted.output().expect("run wiki check --fix (faulted)");

    let mut switched = wiki(&repo.root, &["check", "--fix"]);
    switched.env("WIKI_ANCHOR_CACHE", "0");
    let switched = switched.output().expect("run wiki check --fix (kill switch)");

    assert_eq!(
        faulted.status.code(),
        switched.status.code(),
        "a cache fault must not change the exit code"
    );
    assert_eq!(
        faulted.stdout, switched.stdout,
        "a cache fault must not touch stdout"
    );
    assert_eq!(
        fault_warnings(&faulted.stderr).len(),
        1,
        "the fix phase and the post-fix re-check share one guard — exactly one warning: {:?}",
        fault_warnings(&faulted.stderr)
    );
    assert_eq!(
        fault_warnings(&switched.stderr).len(),
        0,
        "the kill switch is silent by design"
    );
    assert_eq!(
        non_fault_stderr(&faulted),
        non_fault_stderr(&switched),
        "the warning must be the only stderr difference"
    );
}
