//! Wire-level acceptance checks for the anchor cache (plan checks b–k minus
//! f and i): the cache's two tiers engaging through the `wiki check` CLI —
//! the oracle (b), warm-run economy (c), deletion equivalence (d), corruption
//! quarantine (e), concurrent invocations (g), anchor invalidation (h),
//! shallow transition (j), and availability-appears (k, both forms).
//!
//! Written in tdd-bootstrap Phase 2: every check is `#[ignore]`d and
//! unskipped one at a time as the phases land — Phase 1 (store skeleton) for
//! the concurrency check, Phase 2 (fingerprint tier) for the
//! oracle/economy/deletion/corruption checks, Phase 3 (anchor tier) for
//! invalidation/shallow/availability. All three phases are landed, so every
//! check runs.
//!
//! The perf contract (binding, as amended): tier F reports its git leg
//! through the `cache.fingerprint` span (miss path only — never on a served
//! hit) and the `cache.fingerprint.hit` / `cache.fingerprint.miss` counters;
//! `cache.fingerprint.bypass` is dropped — bypass is a tier-A concept, and
//! the `cache.walk.*` names are unchanged. Byte-identity legs must NOT set
//! `WIKI_PERF` — its stderr echo would pollute the comparison; the
//! perf-counting legs run separately with `WIKI_PERF=1` over a freshly
//! deleted `wiki.log` (the perf_integration pattern), so a counting run sees
//! only its own events.
//!
//! All fixtures live in temp dirs — never in the workspace tree. The cache
//! path assertions derive from `git rev-parse --git-common-dir` and the
//! schema's own path functions so they survive layout changes (a linked
//! worktree resolves a different common dir).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Instant;

use serde_json::Value;
use tempfile::TempDir;

use wiki::cache::schema::{cache_dir, db_path, open_connection, probe, ProbeOutcome};

// ── git helpers ───────────────────────────────────────────────────────────────

/// Run `git` in `workdir` with a fixed identity, asserting success.
fn git(workdir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(workdir)
        .env("GIT_AUTHOR_NAME", "Test Author")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test Author")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed in {workdir:?}");
}

/// Run `git` and return trimmed stdout.
fn git_output(workdir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(workdir)
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Run `git` and return raw stdout bytes (binary-safe).
fn git_bytes(workdir: &Path, args: &[&str]) -> Vec<u8> {
    let out = Command::new("git")
        .args(args)
        .current_dir(workdir)
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

// ── fixtures ──────────────────────────────────────────────────────────────────

/// A scratch git repository on `main`.
struct Fixture {
    _dir: TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path().to_path_buf();
        git(&root, &["init", "-q", "-b", "main"]);
        git(&root, &["config", "user.email", "test@example.com"]);
        git(&root, &["config", "user.name", "Test Author"]);
        Self { _dir: dir, root }
    }

    /// The repository's common git dir (`.git` for a plain checkout).
    fn common_dir(&self) -> PathBuf {
        self.root
            .join(git_output(&self.root, &["rev-parse", "--git-common-dir"]))
    }

    /// The cache directory `<common-dir>/wiki` (plan decision 2).
    fn cache_dir(&self) -> PathBuf {
        cache_dir(&self.common_dir())
    }

    /// The cache database file.
    fn db(&self) -> PathBuf {
        db_path(&self.common_dir())
    }
}

/// The certified block `src/target.rs` starts at line 2 (preamble above).
const BLOCK: &str = "fn canonical() {\n    compute()\n    resolve()\n}\n";

fn write_target(root: &Path, name: &str, body: &str) {
    fs::create_dir_all(root.join("src")).expect("create src dir");
    fs::write(root.join("src").join(name), body).expect("write target");
}

fn write_page(root: &Path, name: &str, links: &str) {
    fs::create_dir_all(root.join("wiki")).expect("create wiki dir");
    let body = format!(
        "---\ntitle: {name}\nsummary: A page about {name}.\nlinks-reviewed: 1\n---\n\nSee {links}.\n"
    );
    fs::write(root.join("wiki").join(name), body).expect("write page");
}

fn append(path: &Path, text: &str) {
    let mut f = fs::OpenOptions::new()
        .append(true)
        .open(path)
        .expect("open for append");
    f.write_all(text.as_bytes()).expect("append");
}

/// Three certified pages, two commits of history: every page's walk crosses
/// a real commit pair, and the certified links exercise the fingerprint tier
/// in every run.
fn certified_fixture() -> Fixture {
    let repo = Fixture::new();
    write_target(&repo.root, "target.rs", &format!("// preamble\n{BLOCK}"));
    write_target(&repo.root, "other.rs", "// preamble\nfn other() {\n    work()\n}\n");
    write_target(&repo.root, "gone.rs", "// preamble\nfn gone() {\n    vanish()\n}\n");
    write_page(&repo.root, "guide.md", "[code](../src/target.rs#L2-L4)");
    write_page(&repo.root, "other.md", "[code](../src/other.rs#L2-L4)");
    write_page(&repo.root, "gone.md", "[gone](../src/gone.rs#L2-L4)");
    git(&repo.root, &["add", "-A"]);
    git(&repo.root, &["commit", "-q", "-m", "certify"]);
    append(&repo.root.join("wiki/guide.md"), "\nMore context.\n");
    append(&repo.root.join("wiki/other.md"), "\nMore context.\n");
    append(&repo.root.join("wiki/gone.md"), "\nMore context.\n");
    git(&repo.root, &["add", "-A"]);
    git(&repo.root, &["commit", "-q", "-m", "body context"]);
    repo
}

/// The certified corpus plus a committed drift: the block in `src/target.rs`
/// shifts down one line (a `// x` line inserted above it — RangeDiffered and
/// exactly relocatable), and `src/gone.rs` is deleted out from under its
/// page (Broken — a diagnostic in every mode, so the oracle legs exit 1).
fn drift_fixture() -> Fixture {
    let repo = certified_fixture();
    write_target(&repo.root, "target.rs", &format!("// preamble\n// x\n{BLOCK}"));
    git(&repo.root, &["rm", "-q", "src/gone.rs"]);
    git(&repo.root, &["add", "-A"]);
    git(&repo.root, &["commit", "-q", "-m", "drift the block and delete gone"]);
    repo
}

/// One page in `wiki/` certifying a target in its own `docs/` tree, two
/// commits (the second touches only the page) — the plan's discriminating
/// form: the target's tree object is separate from the page's own tree.
fn separated_tree_fixture() -> Fixture {
    let repo = Fixture::new();
    fs::create_dir_all(repo.root.join("docs")).expect("create docs dir");
    fs::write(
        repo.root.join("docs/target.md"),
        format!("// preamble\n{BLOCK}"),
    )
    .expect("write target");
    write_page(&repo.root, "guide.md", "[code](../docs/target.md#L2-L4)");
    git(&repo.root, &["add", "-A"]);
    git(&repo.root, &["commit", "-q", "-m", "certify"]);
    append(&repo.root.join("wiki/guide.md"), "\nMore context.\n");
    git(&repo.root, &["add", "-A"]);
    git(&repo.root, &["commit", "-q", "-m", "body context"]);
    repo
}

// ── wiki runners ──────────────────────────────────────────────────────────────

/// Spawn the wiki binary from `cwd` with a clean cache environment: `GIT_DIR`,
/// `WIKI_ANCHOR_CACHE`, and `WIKI_PERF` are stripped so a run only sees the
/// settings a test explicitly applies.
fn wiki(cwd: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_wiki"));
    cmd.current_dir(cwd)
        .env_remove("GIT_DIR")
        .env_remove("WIKI_ANCHOR_CACHE")
        .env_remove("WIKI_PERF")
        .args(args);
    cmd
}

/// Run `wiki <args>` from `cwd` and collect the output.
fn run(cwd: &Path, args: &[&str]) -> Output {
    wiki(cwd, args).output().expect("run wiki")
}

/// Run with the kill switch — a guaranteed-uncached baseline.
fn run_cached_off(cwd: &Path, args: &[&str]) -> Output {
    wiki(cwd, args)
        .env("WIKI_ANCHOR_CACHE", "0")
        .output()
        .expect("run wiki (kill switch)")
}

/// Run with `WIKI_PERF=1` over a fresh perf log (deleted first, per the
/// perf_integration pattern), so the log holds only this run's events.
fn run_perf(cwd: &Path, args: &[&str]) -> Output {
    let _ = fs::remove_file(perf_log_path(cwd));
    wiki(cwd, args)
        .env("WIKI_PERF", "1")
        .output()
        .expect("run wiki (perf)")
}

// ── assertions and event counting ─────────────────────────────────────────────

/// The quarantine-rebuilt diagnostic (plan decision 4, delivered by
/// cache-core from `CacheStore::open` on successful quarantine): one line
/// per run on stderr, plain in every mode.
const QUARANTINE_WARNING: &str = "warning: anchor cache was corrupt; rebuilt";

/// Every line of `stderr` that is the quarantine-rebuilt warning.
fn quarantine_warnings(stderr: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(stderr)
        .lines()
        .filter(|l| l.contains(QUARANTINE_WARNING))
        .map(str::to_owned)
        .collect()
}

/// `stderr` with the quarantine-rebuilt warning lines removed — the warning
/// is the only difference a quarantined run may have over its baseline.
fn non_fault_stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .filter(|l| !l.contains(QUARANTINE_WARNING))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Render an output for failure messages.
fn combined(out: &Output) -> String {
    format!(
        "status: {:?}\nstdout:\n{}\nstderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// Assert two runs agree on exit code, stdout, and stderr, byte for byte.
fn assert_byte_identical(a: &Output, b: &Output, label: &str) {
    assert_eq!(
        a.status.code(),
        b.status.code(),
        "{label}: exit codes must match\n{}",
        combined(a)
    );
    assert_eq!(
        a.stdout, b.stdout,
        "{label}: stdout must be byte-identical\n{}",
        combined(a)
    );
    assert_eq!(
        a.stderr, b.stderr,
        "{label}: stderr must be byte-identical\n{}",
        combined(a)
    );
}

/// Assert a faulted run over a broken cache keeps stdout and the exit code
/// byte-identical to a healthy baseline, emits exactly one
/// quarantine-rebuilt warning, and differs on stderr only by that warning.
fn assert_byte_identical_with_fault(baseline: &Output, faulted: &Output, label: &str) {
    assert_eq!(
        faulted.status.code(),
        baseline.status.code(),
        "{label}: exit code must be unchanged"
    );
    assert_eq!(
        faulted.stdout, baseline.stdout,
        "{label}: stdout must be byte-identical"
    );
    let warnings = quarantine_warnings(&faulted.stderr);
    assert_eq!(
        warnings.len(),
        1,
        "{label}: exactly one quarantine-rebuilt warning: {warnings:?}"
    );
    assert_eq!(
        non_fault_stderr(faulted),
        non_fault_stderr(baseline),
        "{label}: the warning must be the only stderr difference"
    );
}

/// The perf log's D12 location: `<common-git-dir>/wiki/wiki.log`.
fn perf_log_path(repo_root: &Path) -> PathBuf {
    let resolved = git_output(repo_root, &["rev-parse", "--git-common-dir"]);
    let path = PathBuf::from(&resolved);
    let common = if path.is_absolute() {
        path
    } else {
        repo_root.join(path)
    };
    common.join("wiki").join("wiki.log")
}

/// Parse every perf event line in the perf log.
fn log_events(repo_root: &Path) -> Vec<Value> {
    let contents = fs::read_to_string(perf_log_path(repo_root)).expect("read wiki.log");
    contents
        .lines()
        .map(|l| serde_json::from_str(l).expect("parse perf event"))
        .collect()
}

/// The run's aggregated `anchor_cache` event (plan decision 7), or `None`
/// when the log has none.
fn anchor_cache_event(repo_root: &Path) -> Option<Value> {
    log_events(repo_root)
        .into_iter()
        .find(|e| e["event"].as_str() == Some("anchor_cache"))
}

/// The aggregate's `meta` counter as a u64 (0 when missing).
fn aggregate_counter(repo_root: &Path, name: &str) -> u64 {
    anchor_cache_event(repo_root)
        .and_then(|e| e["meta"][name].as_u64())
        .unwrap_or(0)
}

/// The aggregate's summed cache-git leg milliseconds (fingerprint + walk) —
/// the memoized cost, zero on a fully served run.
fn aggregate_legs_ms(repo_root: &Path) -> f64 {
    let event = anchor_cache_event(repo_root).expect("anchor_cache event");
    event["meta"]["fingerprint_ms"].as_f64().unwrap_or(0.0)
        + event["meta"]["walk_ms"].as_f64().unwrap_or(0.0)
}

/// Count rows in one cache table filtered by a column equality.
fn count_rows(db: &Path, table: &str, column: &str, value: &str) -> i64 {
    let conn = open_connection(db).expect("open cache db");
    let sql = format!("SELECT COUNT(*) FROM {table} WHERE {column} = ?1");
    conn.query_row(&sql, [value], |row| row.get(0)).expect("count rows")
}

// ── (b) oracle ────────────────────────────────────────────────────────────────

/// Plan check (b) — the oracle: `wiki check` over a real drift corpus with
/// the cache on (cold and warm) and off (kill switch) is byte-identical in
/// stdout, stderr, and exit code — for plain check, `--fix`, and
/// `--fix --dry-run`, all four cache × mode combinations the brief names.
#[test]
fn oracle_all_modes_are_byte_identical_across_cache_states() {
    let repo = drift_fixture();

    // Plain check: cold-on, warm-on, kill-switch off — all byte-identical.
    // The corpus must diagnose the deleted target, so the oracle legs run
    // the nonzero-exit path too.
    let check_on = run(&repo.root, &["check"]);
    assert_eq!(
        check_on.status.code(),
        Some(1),
        "the drift corpus must produce a diagnostic: {}",
        combined(&check_on)
    );
    let check_warm = run(&repo.root, &["check"]);
    let check_off = run_cached_off(&repo.root, &["check"]);
    assert_byte_identical(&check_on, &check_warm, "check: cold vs warm");
    assert_byte_identical(&check_on, &check_off, "check: on vs kill-switch");

    // `--fix` rewrites the drifted href in place; reset the worktree between
    // legs so every leg runs over the same drift state. The cache is cleared
    // before the first fix leg so each mode still sees a genuinely cold run.
    fs::remove_dir_all(repo.cache_dir()).expect("clear cache before fix legs");
    let fix_on = run(&repo.root, &["check", "--fix"]);
    assert_eq!(
        fix_on.status.code(),
        Some(1),
        "the deleted target must survive the fix pass: {}",
        combined(&fix_on)
    );
    let fixed_guide = fs::read_to_string(repo.root.join("wiki/guide.md")).expect("read fixed page");
    git(&repo.root, &["checkout", "--", "wiki/guide.md"]);
    let fix_warm = run(&repo.root, &["check", "--fix"]);
    git(&repo.root, &["checkout", "--", "wiki/guide.md"]);
    let fix_off = run_cached_off(&repo.root, &["check", "--fix"]);
    assert_byte_identical(&fix_on, &fix_warm, "--fix: cold vs warm");
    assert_byte_identical(&fix_on, &fix_off, "--fix: on vs kill-switch");
    let refixed_guide =
        fs::read_to_string(repo.root.join("wiki/guide.md")).expect("read fixed page");
    assert_eq!(
        fixed_guide, refixed_guide,
        "--fix must relocate the same href bytes regardless of the cache"
    );

    // `--fix --dry-run` never rewrites files, but the fix legs above left
    // guide.md relocated — reset so the dry-run legs see the same drifted
    // state the fix legs saw. In fix mode the pre-fix diagnostics exclude
    // the drift pass and a `Broken` link drives a skip, not an exit-driving
    // count, so a dry-run over this corpus exits 0 while still reporting
    // the pending relocation and the unfixable broken link (verified with
    // the built binary before pinning).
    fs::remove_dir_all(repo.cache_dir()).expect("clear cache before dry-run legs");
    git(&repo.root, &["checkout", "--", "wiki/guide.md"]);
    let dry_on = run(&repo.root, &["check", "--fix", "--fix-dry-run"]);
    assert_eq!(
        dry_on.status.code(),
        Some(0),
        "dry-run reports the pending fix and the skip without failing: {}",
        combined(&dry_on)
    );
    let dry_stdout = String::from_utf8_lossy(&dry_on.stdout).into_owned();
    assert!(
        dry_stdout.contains("fix: wiki/guide.md line 7: ../src/target.rs#L2-L4 -> ../src/target.rs#L3-L5"),
        "the dry-run must report the pending relocation: {dry_stdout}"
    );
    assert!(
        dry_stdout.contains("skip: wiki/gone.md"),
        "the dry-run must report the unfixable broken link: {dry_stdout}"
    );
    let dry_warm = run(&repo.root, &["check", "--fix", "--fix-dry-run"]);
    let dry_off = run_cached_off(&repo.root, &["check", "--fix", "--fix-dry-run"]);
    assert_byte_identical(&dry_on, &dry_warm, "--fix --dry-run: cold vs warm");
    assert_byte_identical(&dry_on, &dry_off, "--fix --dry-run: on vs kill-switch");
}

// ── (c) warm-run economy ──────────────────────────────────────────────────────

/// Plan check (c) — warm-run economy: a warm cache pays zero cache-git legs
/// (the aggregate's summed `fingerprint_ms` + `walk_ms` — the memoized
/// per-commit and per-link costs, per plan decision 7's one-per-run event)
/// and recomputes nothing, while the cold run pays the legs and misses.
#[test]
fn warm_run_performs_strictly_fewer_cache_git_legs_than_cold() {
    let repo = certified_fixture();

    let cold = run_perf(&repo.root, &["check"]);
    assert_eq!(cold.status.code(), Some(0), "{}", combined(&cold));
    let cold_events = log_events(&repo.root);
    assert!(
        aggregate_counter(&repo.root, "misses") >= 1,
        "the cold run must miss at least one row: {cold_events:?}"
    );
    assert!(
        aggregate_legs_ms(&repo.root) > 0.0,
        "the cold run must pay the cache-git legs: {cold_events:?}"
    );

    let warm = run_perf(&repo.root, &["check"]);
    assert_eq!(warm.status.code(), Some(0), "{}", combined(&warm));
    let warm_events = log_events(&repo.root);
    assert_eq!(
        aggregate_counter(&repo.root, "misses"),
        0,
        "the warm run must recompute nothing: {warm_events:?}"
    );
    assert_eq!(
        aggregate_legs_ms(&repo.root),
        0.0,
        "the warm run must pay zero cache-git legs: {warm_events:?}"
    );
    assert!(
        aggregate_counter(&repo.root, "hits") >= 1,
        "the warm run must serve at least one row: {warm_events:?}"
    );
}

// ── (d) deletion equivalence ──────────────────────────────────────────────────

/// Plan check (d) — deletion equivalence: run, delete the cache directory,
/// run again; the two runs agree byte for byte and the cache rebuilds.
#[test]
fn deleting_the_cache_directory_changes_nothing() {
    let repo = certified_fixture();

    let first = run(&repo.root, &["check"]);
    assert_eq!(first.status.code(), Some(0), "{}", combined(&first));
    assert!(repo.db().exists(), "the first run must create the cache database");

    let dir = repo.cache_dir();
    fs::remove_dir_all(&dir).expect("delete cache directory");
    assert!(!dir.exists(), "the cache directory must be gone");

    let second = run(&repo.root, &["check"]);
    assert_eq!(second.status.code(), Some(0), "{}", combined(&second));
    assert_byte_identical(&first, &second, "before vs after deleting the cache directory");
    assert_eq!(
        probe(&repo.db()).expect("probe rebuilt cache"),
        ProbeOutcome::Valid,
        "the second run must rebuild a valid cache"
    );
}

// ── (e) corruption quarantine ─────────────────────────────────────────────────

/// Plan check (e) — corruption: garbage over `anchor-cache.sqlite`, and
/// separately a truncated valid database, must each leave the check passing
/// with exactly one cache-fault diagnostic line and a rebuilt cache.
///
/// Contract note: "exactly one diagnostic line" pins plan decision 4's
/// quarantine contract — "at most one recreate, then disable for the run" —
/// with the fault surfaced through the quarantine-rebuilt line that
/// cache-core delivers from `CacheStore::open` on successful quarantine
/// (binding wording `warning: anchor cache was corrupt; rebuilt`, one line
/// per run, plain stderr in every mode). The phase that unskips this check
/// must have that line in place; the assertion matches it by substring.
#[test]
fn corrupted_cache_faults_once_and_rebuilds() {
    let repo = certified_fixture();

    let baseline = run(&repo.root, &["check"]);
    assert_eq!(baseline.status.code(), Some(0), "{}", combined(&baseline));
    assert_eq!(
        quarantine_warnings(&baseline.stderr).len(),
        0,
        "a healthy run must not warn"
    );

    // Leg 1: plain garbage over the database file.
    fs::write(repo.db(), b"this is not a sqlite database - plain garbage").expect("garbage over db");
    let garbage = run(&repo.root, &["check"]);
    assert_byte_identical_with_fault(&baseline, &garbage, "garbage over the database");
    assert_eq!(
        probe(&repo.db()).expect("probe rebuilt cache"),
        ProbeOutcome::Valid,
        "the garbage run must rebuild a valid cache"
    );

    // Leg 2: a truncated valid database (header intact, pages cut).
    let conn = open_connection(&repo.db()).expect("open cache db");
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .expect("checkpoint");
    drop(conn);
    let len = fs::metadata(repo.db()).expect("db metadata").len();
    fs::File::options()
        .write(true)
        .open(repo.db())
        .expect("open db")
        .set_len(len / 2)
        .expect("truncate db");
    let truncated = run(&repo.root, &["check"]);
    assert_byte_identical_with_fault(&baseline, &truncated, "truncated database");
    assert_eq!(
        probe(&repo.db()).expect("probe rebuilt cache"),
        ProbeOutcome::Valid,
        "the truncated-db run must rebuild a valid cache"
    );
}

/// A meta-valid database with either tier's binding table malformed is
/// schema skew (plan D2), not corruption: the next run repairs it with
/// tier-scoped structural invalidation — silently, byte-identical to the
/// baseline (no quarantine warning) — and warms normally on the following
/// run.
#[test]
fn malformed_tier_schemas_rebuild_transparently_and_then_hit() {
    for (table, ddl) in [
        ("fingerprint", "CREATE TABLE fingerprint (key_digest TEXT PRIMARY KEY) STRICT;"),
        ("anchor_walk", "CREATE TABLE anchor_walk (key_digest TEXT PRIMARY KEY) STRICT;"),
    ] {
        let repo = certified_fixture();
        let baseline = run(&repo.root, &["check"]);
        assert_eq!(baseline.status.code(), Some(0), "{}", combined(&baseline));

        let conn = rusqlite::Connection::open(repo.db()).expect("open cache for schema damage");
        conn.execute_batch(&format!("DROP TABLE {table}; {ddl}"))
            .expect("malform tier table");
        drop(conn);

        let rebuilt = run(&repo.root, &["check"]);
        assert_byte_identical(&baseline, &rebuilt, table);
        assert_eq!(probe(&repo.db()).unwrap(), ProbeOutcome::Valid, "{table}");

        let warm = run_perf(&repo.root, &["check"]);
        assert_eq!(warm.status.code(), Some(0), "{}", combined(&warm));
        assert_eq!(aggregate_counter(&repo.root, "misses"), 0, "rebuilt {table} must warm");
    }
}

// ── (g) concurrent invocations ────────────────────────────────────────────────

/// Plan check (g) — concurrent invocations: two `wiki check` processes
/// racing on a fresh cache (mirroring index_concurrent.rs) both finish
/// without blocking beyond the busy timeout and produce byte-identical,
/// correct output.
#[test]
fn two_concurrent_checks_on_a_fresh_cache_both_succeed() {
    let repo = certified_fixture();
    assert!(
        !repo.cache_dir().exists(),
        "the race must start on a fresh cache"
    );

    let start = Instant::now();
    let first = wiki(&repo.root, &["check"]).spawn().expect("spawn first wiki");
    let second = wiki(&repo.root, &["check"]).spawn().expect("spawn second wiki");
    let first_out = first.wait_with_output().expect("first wiki");
    let second_out = second.wait_with_output().expect("second wiki");
    let elapsed = start.elapsed();

    // The busy timeout (1000 ms) and the bounded retry wrapper bound any
    // SQLite wait to a few seconds; a genuine block would hang until the
    // test runner kills it. 30 s is generous for VM/CI noise while still
    // catching a deadlock.
    assert!(
        elapsed.as_secs() < 30,
        "concurrent runs must not block: {elapsed:?}"
    );

    assert_eq!(
        first_out.status.code(),
        Some(0),
        "first racer: {}",
        combined(&first_out)
    );
    assert_eq!(
        second_out.status.code(),
        Some(0),
        "second racer: {}",
        combined(&second_out)
    );
    assert_byte_identical(&first_out, &second_out, "concurrent racers must agree");

    // A post-race control run on the (now warm) cache agrees with both.
    let control = run(&repo.root, &["check"]);
    assert_byte_identical(&first_out, &control, "racers vs warm control");
}

// ── (h) anchor invalidation ───────────────────────────────────────────────────

/// Plan check (h) — anchor invalidation: a commit touching one page moves
/// its walk key (its `git log --follow` output changes), so that page's
/// walk misses and recomputes, while untouched walks and fingerprint rows
/// still hit. The three-page, one-link fixture makes the aggregated
/// per-run counters (plan decision 7) discriminate per page: after the
/// touch exactly one miss and five hits is the only consistent reading.
#[test]
fn commit_touching_a_page_moves_its_walk_key_but_not_others() {
    let repo = certified_fixture();

    // Cold run lands the rows; the warm run serves every walk and fingerprint.
    run(&repo.root, &["check"]);
    let warm = run_perf(&repo.root, &["check"]);
    assert_eq!(warm.status.code(), Some(0), "{}", combined(&warm));
    let warm_events = log_events(&repo.root);
    assert_eq!(
        aggregate_counter(&repo.root, "misses"),
        0,
        "the warm run must serve every row: {warm_events:?}"
    );

    // A commit touching only `guide.md` changes its log output → new key.
    append(
        &repo.root.join("wiki/guide.md"),
        "\nNew paragraph after the touch.\n",
    );
    git(&repo.root, &["add", "-A"]);
    git(&repo.root, &["commit", "-q", "-m", "touch guide"]);

    let after = run_perf(&repo.root, &["check"]);
    assert_eq!(after.status.code(), Some(0), "{}", combined(&after));
    let events = log_events(&repo.root);
    assert_eq!(
        aggregate_counter(&repo.root, "misses"),
        1,
        "exactly the touched page's walk must miss and recompute: {events:?}"
    );
    assert_eq!(
        aggregate_counter(&repo.root, "hits"),
        5,
        "the untouched walks and every fingerprint row must still hit: {events:?}"
    );
}

// ── (j) shallow transition ────────────────────────────────────────────────────

/// Plan check (j) — shallow transition: after the cache warms on a full
/// checkout, shallow-ifying the repo (`git fetch --depth=1`) makes the next
/// run fail closed at exit 2, byte-identical to an uncached shallow run —
/// warm rows must never serve through the shallow gate.
#[test]
fn shallow_transition_bypasses_and_fails_closed() {
    let repo = certified_fixture();

    let warm = run(&repo.root, &["check"]);
    assert_eq!(warm.status.code(), Some(0), "{}", combined(&warm));
    assert!(repo.db().exists(), "the warm run must have cached rows");

    // Shallow-ify in place: a bare clone of the repo, added as a remote, then
    // `git fetch --depth=1` — the shallow-CI path the plan cites. This works
    // even though the tip is already present: the fetch writes `.git/shallow`.
    let remote = TempDir::new().expect("tempdir for remote");
    let remote_path = remote.path().join("remote.git");
    let remote_str = remote_path.to_str().expect("utf8 remote path");
    git(&repo.root, &["clone", "-q", "--bare", ".", remote_str]);
    git(&repo.root, &["remote", "add", "origin", remote_str]);
    git(&repo.root, &["fetch", "-q", "--depth=1", "origin", "main"]);
    assert_eq!(
        git_output(&repo.root, &["rev-parse", "--is-shallow-repository"]),
        "true",
        "the fixture must be shallow after the depth-1 fetch"
    );

    // Cache-on and cache-off shallow runs are byte-identical and fail
    // closed — the warm rows must not serve through the gate.
    let on = run(&repo.root, &["check"]);
    let off = run_cached_off(&repo.root, &["check"]);
    assert_byte_identical(&on, &off, "shallow: cache-on vs kill-switch");
    assert_eq!(
        on.status.code(),
        Some(2),
        "a shallow clone must fail closed: {}",
        combined(&on)
    );

    // No serve and no compute-write: the gate bypasses both tiers.
    let perf = run_perf(&repo.root, &["check"]);
    assert_eq!(
        perf.status.code(),
        Some(2),
        "the counting leg must fail closed like the others: {}",
        combined(&perf)
    );
    let events = log_events(&repo.root);
    assert_eq!(
        aggregate_counter(&repo.root, "hits"),
        0,
        "a shallow run must never serve: {events:?}"
    );
    assert_eq!(
        aggregate_counter(&repo.root, "misses"),
        0,
        "a shallow run must never compute-write: {events:?}"
    );
    assert!(
        aggregate_counter(&repo.root, "bypasses") >= 1,
        "the shallow gate must bypass the walk tier: {events:?}"
    );
}

// ── (k) availability-appears ──────────────────────────────────────────────────

/// Plan check (k), first form — availability-appears with an unreadable
/// historical blob: the page's path stays in the tree, so the three-valued
/// availability probe reads *present* and no anchor-walk row is written for
/// the page (its walk's git legs already failed); the run stays byte-identical
/// to the cache-off run. Restoring the blob lets the next run cache and the
/// one after serve.
#[test]
fn unreadable_page_blob_is_probed_present_and_skips_the_walk_row() {
    let repo = certified_fixture();
    let damaged = "wiki/guide.md";
    let control = "wiki/other.md";

    // Capture the historical blob's content, then remove the loose object at
    // the oldest commit: the path stays in the tree, so the probe reads
    // present, and the walk's failed read resolves to a healthy epoch.
    let blob_sha = git_output(&repo.root, &["rev-parse", "HEAD~1:wiki/guide.md"]);
    let content = git_bytes(&repo.root, &["show", "HEAD~1:wiki/guide.md"]);
    let loose = repo
        .root
        .join(".git/objects")
        .join(&blob_sha[0..2])
        .join(&blob_sha[2..]);
    fs::remove_file(&loose).expect("remove loose blob");

    let on = run(&repo.root, &["check"]);
    let off = run_cached_off(&repo.root, &["check"]);
    assert_byte_identical(&on, &off, "unreadable blob: cache-on vs kill-switch");

    // No anchor-walk row for the damaged page; the control page's row lands.
    let db = repo.db();
    assert_eq!(
        count_rows(&db, "anchor_walk", "page_path", damaged),
        0,
        "the damaged page's walk must not be cached"
    );
    assert_eq!(
        count_rows(&db, "anchor_walk", "page_path", control),
        1,
        "the control page's walk must still be cached"
    );

    // Restore the blob (`git hash-object -w` is content-addressed, so the
    // SHA comes back identical); the next run caches the row and the one
    // after serves it.
    let mut child = Command::new("git")
        .current_dir(&repo.root)
        .args(["hash-object", "-w", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("spawn git hash-object");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(&content)
        .expect("write blob content");
    let status = child.wait().expect("git hash-object");
    assert!(status.success(), "git hash-object -w failed");
    assert_eq!(
        git_output(&repo.root, &["rev-parse", "HEAD~1:wiki/guide.md"]),
        blob_sha,
        "the restored blob must resolve to the same content-addressed sha"
    );

    let restored = run(&repo.root, &["check"]);
    assert_eq!(
        restored.status.code(),
        on.status.code(),
        "restored state must exit identically"
    );
    assert_eq!(
        restored.stdout, on.stdout,
        "restored state must check identically"
    );
    assert_eq!(
        count_rows(&db, "anchor_walk", "page_path", damaged),
        1,
        "the restored run must cache the damaged page's walk"
    );

    let serve = run_perf(&repo.root, &["check"]);
    assert_eq!(serve.status.code(), Some(0), "{}", combined(&serve));
    let events = log_events(&repo.root);
    assert_eq!(
        aggregate_counter(&repo.root, "misses"),
        0,
        "the restored row must serve — nothing recomputed: {events:?}"
    );
    assert!(
        aggregate_counter(&repo.root, "hits") >= 1,
        "the restored row must serve: {events:?}"
    );
}

/// Plan check (k), second form — availability-appears with an unreadable
/// target tree: the tree object containing the target path (not the page's
/// own tree) is removed, so the run must fail closed — byte-identical to the
/// cache-off run, with no fingerprint row written for the target.
///
/// The target lives in its own directory tree (`docs/`, the page in
/// `wiki/`) so the damage hits a tree the page's own path never names —
/// removing the page's own tree would kill the walk before the fingerprint
/// tier even opens. That separation is required, but it does not rescue the
/// plan's original premise that the three-valued probe would resolve
/// *unknown*: empirically, `git log --follow --name-status` reads every tree
/// of every walked commit (the `--follow` rename machinery needs the full
/// diff — the same walk without `--follow` survives a missing foreign tree),
/// so a missing tree object anywhere in the page's walked history aborts the
/// walk at exit 2 before the fingerprint tier or the probe ever engages.
/// The check therefore pins the plan's literal observables under tree-level
/// damage — byte-identity, fail-closed exit 2, zero fingerprint rows — the
/// fail-closed guarantee the three-valued probe exists to protect (a
/// two-valued probe would misread an unreadable tree as absence and record
/// an `fp = 0` row). The unknown probe outcome is not reachable through the
/// CLI; the absent branch (probe exit 0 + empty → `fp = 0` row) and the
/// present branch (form 1) are the reachable ones.
#[test]
fn unreadable_target_tree_fails_closed_without_fingerprint_rows() {
    let repo = separated_tree_fixture();

    // The docs tree is identical at both commits (the body commit touches
    // only the page), so one object removal damages the anchor side too.
    let tree_sha = git_output(&repo.root, &["rev-parse", "HEAD~1:docs"]);
    let loose = repo
        .root
        .join(".git/objects")
        .join(&tree_sha[0..2])
        .join(&tree_sha[2..]);
    fs::remove_file(&loose).expect("remove loose tree");

    let on = run(&repo.root, &["check"]);
    let off = run_cached_off(&repo.root, &["check"]);
    assert_byte_identical(&on, &off, "unreadable tree: cache-on vs kill-switch");
    assert_eq!(
        on.status.code(),
        Some(2),
        "tree-level damage must fail closed (exit 2): {}",
        combined(&on)
    );

    // The run aborts at the first walk, so no fingerprint row for the target
    // is ever written.
    assert_eq!(
        count_rows(&repo.db(), "fingerprint", "target_path", "docs/target.md"),
        0,
        "no fingerprint row may be written for the unreadable target"
    );
}
