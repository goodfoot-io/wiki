//! Concurrency test: a second `wiki` process must return results without
//! blocking forever on a held rendezvous lock (plan D7).
//!
//! Contract: when `<common>/wiki/rendezvous.lock` is held EXCLUSIVELY by an
//! external process, a `wiki search <query>` invocation still exits 0 — it
//! waits out the bounded budget (~10 s), then takes the floor: serve the
//! newest retained snapshot / proceed uncached after one diagnostic line.
//! The budget assertion is therefore generous-but-bounded: completing at
//! all proves no unbounded hang; staying under 15 s proves the wait is
//! bounded as designed.

mod common;

use std::time::Instant;

use assert_cmd::Command;
use wiki::cache::rendezvous;

#[test]
fn second_process_returns_without_waiting_on_rendezvous_lock() {
    let repo = common::make_parity_fixture();
    let common = common::git_common_dir(&repo.root);

    // Hold the rendezvous exclusively to simulate a long-running refresh
    // publication in a sibling process. Acquired through the production API
    // so the lock file and its private parent subtree are created exactly
    // as the open paths create them.
    let _held = rendezvous::try_acquire_exclusive(&common)
        .expect("rendezvous acquire")
        .expect("free store grants exclusive");

    let start = Instant::now();

    // Run a second `wiki` process. It must not hang: bounded wait, then the
    // serve-stale/uncached floor.
    let output = Command::cargo_bin("wiki")
        .expect("cargo_bin wiki")
        .current_dir(&repo.root)
        .args(["committed"])
        .output()
        .expect("wiki process");

    let elapsed = start.elapsed();

    // Bounded, not unbounded: under the 10 s acquisition budget plus normal
    // run overhead.
    assert!(
        elapsed.as_millis() < 15_000,
        "second process took {}ms — exceeded the bounded-wait envelope",
        elapsed.as_millis()
    );

    // Must exit cleanly regardless of which side of the floor it landed on.
    assert!(
        output.status.success(),
        "wiki exited with {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
}

// ── Adversarial repair wave: runtime-witnessed findings ─────────────────

use std::path::PathBuf;
use std::process::Output;

fn run_wiki(cwd: &std::path::Path, args: &[&str]) -> Output {
    Command::cargo_bin("wiki")
        .expect("cargo_bin wiki")
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("wiki process")
}

fn stdout_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn stderr_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

fn warning_lines(stderr: &str) -> usize {
    stderr.lines().filter(|l| l.starts_with("warning:")).count()
}

/// Two linked worktrees sharing one common dir, via the real git plumbing.
fn linked_worktrees(base: &common::FixtureRepo) -> PathBuf {
    let wt2 = base
        .dir
        .path()
        .parent()
        .unwrap()
        .join(format!("wt-linked-{}", std::process::id()));
    let added = std::process::Command::new("git")
        .current_dir(&base.root)
        .args(["worktree", "add"])
        .arg(&wt2)
        .arg("-b")
        .arg("wt-linked")
        .output()
        .expect("git worktree add");
    assert!(
        added.status.success(),
        "git worktree add failed: {}",
        String::from_utf8_lossy(&added.stderr)
    );
    wt2
}

/// F3 witness (a): a warm store whose newest generation carries ANOTHER
/// worktree's uncommitted corpus must not leak it to this worktree when the
/// refresh rendezvous times out. The contended run's output must equal this
/// worktree's uncontended reference — own rows only.
#[test]
fn contended_timeout_serves_own_worktree_corpus_not_foreign() {
    let repo = common::FixtureRepo::new();
    repo.write_wiki_md("committed.md", "Committed Page", "Shared committed page.", "body text");
    repo.git_add("committed.md");
    repo.git_commit("add shared page");

    let wt2 = linked_worktrees(&repo);

    // Worktree 1 gains an UNCOMMITTED extra page and primes the shared
    // store with its own corpus — the would-be phantom source.
    repo.write_wiki_md(
        "phantom_extra.md",
        "Phantom Title",
        "Only uncommitted in worktree one.",
        "phantom body",
    );
    let primed = run_wiki(&repo.root, &["--format", "json", "committed"]);
    assert!(primed.status.success());

    // Reference: worktree 2, uncontended — publishes and serves its OWN
    // state.
    let reference = run_wiki(&wt2, &["--format", "json", "committed"]);
    assert!(reference.status.success(), "reference run must succeed");
    let reference_out = stdout_of(&reference);
    assert!(reference_out.contains("committed.md"), "reference must match: {reference_out}");

    // Contended: hold the refresh-exclusive rendezvous; the same command
    // times out into the floor — which must answer with THIS worktree's
    // corpus, byte-identical to the reference.
    let common = common::git_common_dir(&wt2);
    let _held = rendezvous::try_acquire_exclusive(&common)
        .expect("rendezvous acquire")
        .expect("free store grants exclusive");

    let started = Instant::now();
    let contended = run_wiki(&wt2, &["--format", "json", "committed"]);
    let elapsed = started.elapsed();

    assert!(contended.status.success(), "contended run must still exit 0");
    assert!(
        elapsed.as_millis() < 15_000,
        "bounded wait exceeded: {elapsed:?}"
    );
    let out = stdout_of(&contended);
    assert_eq!(
        out, reference_out,
        "the timeout floor must serve this worktree's own corpus, identically"
    );
    assert!(
        !out.contains("Phantom Title"),
        "foreign uncommitted corpus must never leak through the floor"
    );
    assert!(
        warning_lines(&stderr_of(&contended)) <= 1,
        "at most one diagnostic line on degradation: {}",
        stderr_of(&contended)
    );
}

/// F3 witness (b): a COLD store under a held rendezvous must still answer a
/// matching query exactly like its uncached reference — the floor is an
/// ephemeral in-memory rebuild, not empty output.
#[test]
fn contended_cold_store_answers_uncached_reference() {
    let repo = common::FixtureRepo::new();
    repo.write_wiki_md("only.md", "Luminal Term", "The only page.", "distinctive body prose");
    repo.git_add("only.md");
    repo.git_commit("add only page");

    // Reference: cold but uncontended — builds the store and answers.
    let reference = run_wiki(&repo.root, &["--format", "json", "luminal"]);
    assert!(reference.status.success());
    let reference_out = stdout_of(&reference);
    assert!(!reference_out.trim().is_empty(), "matching query must have results");

    // Re-cold: remove every shared-store artifact.
    let common = common::git_common_dir(&repo.root);
    let wiki_dir = common.join("wiki");
    std::fs::remove_dir_all(&wiki_dir).expect("re-cold the store");

    // Contended against the cold store.
    let _held = rendezvous::try_acquire_exclusive(&common)
        .expect("rendezvous acquire")
        .expect("absent lock grants exclusive");

    let started = Instant::now();
    let contended = run_wiki(&repo.root, &["--format", "json", "luminal"]);
    let elapsed = started.elapsed();

    assert!(contended.status.success(), "cold+contended must exit 0");
    assert!(elapsed.as_millis() < 15_000, "bounded wait exceeded: {elapsed:?}");
    let out = stdout_of(&contended);
    assert!(
        !out.trim().is_empty(),
        "a matching query must never come back empty from the floor"
    );
    assert_eq!(
        out, reference_out,
        "the ephemeral uncached rebuild must equal the uncontended reference byte-for-byte"
    );
    assert!(
        warning_lines(&stderr_of(&contended)) <= 1,
        "at most one diagnostic line: {}",
        stderr_of(&contended)
    );
}

/// F6 witness (command level): a corrupt store encountered while another
/// process holds the init lock degrades to exit 0 with at most one warning
/// line — never NotADatabase as a hard failure.
#[test]
fn corrupt_store_under_held_init_lock_degrades_to_exit_0() {
    let repo = common::FixtureRepo::new();
    repo.write_wiki_md("page.md", "Steady Page", "A steady page.", "steady body");
    repo.git_add("page.md");
    repo.git_commit("add page");

    // Prime once so the wiki/ subtree exists, then corrupt the database.
    assert!(run_wiki(&repo.root, &["list"]).status.success());
    let common = common::git_common_dir(&repo.root);
    let db = common.join("wiki").join("store.sqlite");
    std::fs::write(&db, b"garbage bytes - not a database").expect("corrupt db");

    // Hold the init lock like a sibling mid-repair.
    let lock_path = common.join("wiki").join(wiki::cache::schema::INIT_LOCK_FILE_NAME);
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .open(&lock_path)
        .expect("open existing init lock");
    fs4::fs_std::FileExt::try_lock_exclusive(&lock).expect("hold init lock");

    let contended = run_wiki(&repo.root, &["--format", "json", "steady"]);
    assert!(
        contended.status.success(),
        "degraded run must exit 0, got {:?}: {}",
        contended.status.code(),
        stderr_of(&contended)
    );
    assert!(
        !stdout_of(&contended).trim().is_empty(),
        "the degraded ephemeral tier still answers matching queries"
    );
    assert!(
        warning_lines(&stderr_of(&contended)) <= 1,
        "at most one diagnostic line on degradation: {}",
        stderr_of(&contended)
    );
}

/// F9-support witness: a quarantine performed by the index open path must
/// surface its countable events through the run's JSON-lines channel — no
/// extra stderr noise beyond existing budgets.
#[test]
fn quarantine_surfaces_through_json_lines() {
    let repo = common::FixtureRepo::new();
    repo.write_wiki_md("page.md", "Quarantine Witness", "Witness page.", "witness body");
    repo.git_add("page.md");
    repo.git_commit("add page");

    // Prime so the wiki/ subtree and log path conventions exist.
    assert!(run_wiki(&repo.root, &["list"]).status.success());
    let common = common::git_common_dir(&repo.root);
    let log_path = common.join("wiki").join("wiki.log");
    let _ = std::fs::remove_file(&log_path);

    // Corrupt the store; the next open quarantines and rebuilds it.
    let db = common.join("wiki").join("store.sqlite");
    std::fs::write(&db, b"corrupt bytes for the witness").expect("corrupt db");

    let run = run_wiki(&repo.root, &["--format", "json", "list"]);
    assert!(run.status.success(), "quarantining run must succeed");
    let stderr = stderr_of(&run);
    assert!(
        warning_lines(&stderr) == 0,
        "quarantine is a counted event, not stderr noise: {stderr}"
    );

    let log = std::fs::read_to_string(&log_path).unwrap_or_default();
    let mut found = false;
    for line in log.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let Some(counts) = value.pointer("/meta/diagnostics") {
            if counts.get("quarantine_performed").and_then(|v| v.as_u64()) >= Some(1)
                && counts.get("rebuild_completed").and_then(|v| v.as_u64()) >= Some(1)
            {
                found = true;
                break;
            }
        }
    }
    assert!(
        found,
        "JSON-lines must carry quarantine/rebuild diagnostic counts; log:\n{log}"
    );
}

// ── Round-2 repair wave: store-fault fail-open (F-A) ────────────────────

/// Corrupt a byte range of the store file.
fn corrupt_store(common: &std::path::Path, offset: u64, len: usize) {
    let db = common.join("wiki").join("store.sqlite");
    let mut bytes = std::fs::read(&db).expect("read store");
    assert!(
        bytes.len() as u64 > offset + len as u64,
        "store too small to corrupt"
    );
    for b in &mut bytes[offset as usize..offset as usize + len] {
        *b = 0xFF;
    }
    std::fs::write(&db, bytes).expect("write corrupted store");
}

/// F-A witness (1): probe-passing header corruption (schema-format field,
/// bytes 44–47) must recover transparently — byte-identical output vs the
/// pre-corruption reference, at most one diagnostic line, and a healthy
/// store afterwards.
#[test]
fn header_corruption_recovers_byte_identical() {
    let repo = common::FixtureRepo::new();
    repo.write_wiki_md(
        "page.md",
        "Header Corruption Page",
        "Corruption witness page.",
        "witness body",
    );
    repo.git_add("page.md");
    repo.git_commit("add page");

    // Reference on the healthy warm store.
    let reference = run_wiki(&repo.root, &["--format", "json", "list"]);
    assert!(reference.status.success());
    let reference_out = stdout_of(&reference);

    // Overwrite the schema-format field.
    let common = common::git_common_dir(&repo.root);
    corrupt_store(&common, 44, 4);

    let recovered = run_wiki(&repo.root, &["--format", "json", "list"]);
    assert!(
        recovered.status.success(),
        "corrupted-header run must exit 0, got {:?}: {}",
        recovered.status.code(),
        stderr_of(&recovered)
    );
    assert_eq!(
        stdout_of(&recovered),
        reference_out,
        "output must be byte-identical to the pre-corruption reference"
    );
    assert!(
        warning_lines(&stderr_of(&recovered)) <= 1,
        "at most one diagnostic line: {}",
        stderr_of(&recovered)
    );

    // The store is healthy after the run: an uncontended follow-up answers
    // identically without any further fault path.
    let followup = run_wiki(&repo.root, &["--format", "json", "list"]);
    assert!(followup.status.success());
    assert_eq!(stdout_of(&followup), reference_out);
}

/// F-A witness (2): corruption that only bites at publish time (linked
/// worktree scenario class) degrades to exit 0 with uncached-reference
/// results — never `publish: ... database disk image is malformed` exit 2.
#[test]
fn publish_time_corruption_degrades_to_uncached_reference() {
    let repo = common::FixtureRepo::new();
    repo.write_wiki_md(
        "alpha.md",
        "Alpha Publish",
        "Publish-corruption witness.",
        "distinctive publish witness prose",
    );
    repo.git_add("alpha.md");
    repo.git_commit("add alpha");

    // Reference from a healthy store.
    let reference = run_wiki(&repo.root, &["--format", "json", "witness"]);
    assert!(reference.status.success());
    let reference_out = stdout_of(&reference);
    assert!(!reference_out.trim().is_empty());

    // Corrupt deep into the file — past the small hot tables' pages, where
    // publish-time writes (fts shadow tables, blob upserts) trip first.
    let common = common::git_common_dir(&repo.root);
    let db = common.join("wiki").join("store.sqlite");
    let size = std::fs::metadata(&db).expect("db metadata").len();
    let tail_offset = size.saturating_sub(256).max(100);
    corrupt_store(&common, tail_offset, 64);

    let degraded = run_wiki(&repo.root, &["--format", "json", "witness"]);
    assert!(
        degraded.status.success(),
        "publish-fault run must exit 0 via the uncached floor, got {:?}: {}",
        degraded.status.code(),
        stderr_of(&degraded)
    );
    assert_eq!(
        stdout_of(&degraded),
        reference_out,
        "uncached-floor answers must equal the healthy-store reference"
    );
    assert!(
        warning_lines(&stderr_of(&degraded)) <= 1,
        "at most one diagnostic line: {}",
        stderr_of(&degraded)
    );

    // The forced quarantine/rebuild means the NEXT run is healthy again.
    let healed = run_wiki(&repo.root, &["--format", "json", "witness"]);
    assert!(healed.status.success());
    assert_eq!(stdout_of(&healed), reference_out);
}

/// F-A witness (3): BUSY during index open (a long writer transaction on
/// the store from another process) maps exhausted-BUSY onto the contention
/// floor — exit 0, correct uncached answers, one line, bounded.
#[test]
fn busy_open_degrades_to_uncached_answers() {
    let repo = common::FixtureRepo::new();
    repo.write_wiki_md(
        "busy.md",
        "Busy Witness Page",
        "Witness for open-time contention.",
        "contention witness prose",
    );
    repo.git_add("busy.md");
    repo.git_commit("add busy");

    // Reference while uncontended.
    let reference = run_wiki(&repo.root, &["--format", "json", "contention"]);
    assert!(reference.status.success());
    let reference_out = stdout_of(&reference);

    // Hold a write transaction across the command's open window.
    let db = common::git_common_dir(&repo.root).join("wiki").join("store.sqlite");
    let holder = std::thread::spawn(move || {
        let conn = rusqlite::Connection::open(&db).expect("holder conn");
        conn.execute_batch("BEGIN IMMEDIATE; CREATE TABLE hold_open (x INTEGER);")
            .expect("take write lock");
        std::thread::sleep(std::time::Duration::from_millis(2500));
        let _ = conn.execute_batch("ROLLBACK;");
    });
    std::thread::sleep(std::time::Duration::from_millis(50));

    let started = Instant::now();
    let degraded = run_wiki(&repo.root, &["--format", "json", "contention"]);
    let elapsed = started.elapsed();

    holder.join().expect("holder thread");

    assert!(
        degraded.status.success(),
        "busy-open run must exit 0 via the contention floor, got {:?}: {}",
        degraded.status.code(),
        stderr_of(&degraded)
    );
    assert!(
        elapsed.as_millis() < 15_000,
        "bounded duration exceeded: {elapsed:?}"
    );
    assert_eq!(
        stdout_of(&degraded),
        reference_out,
        "the contention floor answers with correct uncached results"
    );
    assert!(
        warning_lines(&stderr_of(&degraded)) <= 1,
        "at most one diagnostic line: {}",
        stderr_of(&degraded)
    );
}
