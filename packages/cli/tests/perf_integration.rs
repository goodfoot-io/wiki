use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

struct TestRepo {
    dir: TempDir,
}

impl TestRepo {
    fn new() -> Self {
        let dir = TempDir::new().expect("tempdir");
        let repo = Self { dir };
        repo.git(&["init"]);
        repo.git(&["checkout", "-b", "main"]);
        repo
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn create_file(&self, path: &str, content: &str) {
        let full = self.dir.path().join(path);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).expect("create_dir_all");
        }
        fs::write(full, content).expect("write file");
    }

    fn log_path(&self) -> PathBuf {
        self.dir.path().join("wiki/wiki.log")
    }

    fn git(&self, args: &[&str]) {
        let output = Command::new("git")
            .current_dir(self.dir.path())
            .args(args)
            .env("GIT_AUTHOR_NAME", "Test Author")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test Committer")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .expect("spawn git");
        assert!(
            output.status.success(),
            "git {:?} failed:\n{}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn commit_all(&self, message: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-m", message]);
    }
}

fn run_wiki(repo: &TestRepo, args: &[&str]) -> Output {
    run_wiki_env(repo, args, &[])
}

fn run_wiki_env(repo: &TestRepo, args: &[&str], extra_env: &[(&str, &str)]) -> Output {
    let _ = fs::remove_file(repo.log_path());

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_wiki"));
    // Run from inside the wiki/ directory so `WikiConfig::load` walks up and
    // finds `wiki/wiki.toml`.
    cmd.current_dir(repo.path().join("wiki"))
        .args(args)
        .env("WIKI_BACKGROUND_FTS", "0")
        .env_remove("WIKI_PERF");
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    let output = cmd.output().expect("run wiki");

    assert!(
        output.status.success() || output.status.code() == Some(1),
        "status: {:?}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    output
}

fn log_events(repo: &TestRepo) -> Vec<Value> {
    let contents = fs::read_to_string(repo.log_path()).expect("read wiki log");
    contents
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("parse log event"))
        .collect()
}

fn has_event(events: &[Value], name: &str) -> bool {
    events
        .iter()
        .any(|event| event["event"].as_str() == Some(name))
}

fn seed_repo() -> TestRepo {
    let repo = TestRepo::new();
    repo.create_file("wiki/wiki.toml", "");
    repo.create_file(
        "wiki/example.md",
        "---\ntitle: Example\nsummary: Example summary.\n---\nBody.\n",
    );
    repo.commit_all("init");
    repo
}

#[test]
fn perf_flag_logs_timings_to_stderr() {
    let repo = seed_repo();
    let out = run_wiki(&repo, &["--perf", "list"]);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        stderr.contains("wiki perf: command.list"),
        "expected command_finish timing in stderr, got: {stderr}"
    );
}

#[test]
fn perf_env_logs_timings_to_stderr() {
    let repo = seed_repo();
    let out = run_wiki_env(&repo, &["list"], &[("WIKI_PERF", "1")]);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        stderr.contains("wiki perf: command.list"),
        "expected command_finish timing in stderr, got: {stderr}"
    );
}

#[test]
fn perf_off_emits_no_stderr_timings() {
    let repo = seed_repo();
    let out = run_wiki(&repo, &["list"]);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        !stderr.contains("wiki perf:"),
        "expected no perf timings without --perf, got: {stderr}"
    );
}

#[test]
fn warm_list_avoids_full_rescan_discovery_event() {
    let repo = TestRepo::new();
    repo.create_file("wiki/wiki.toml", "");
    repo.create_file(
        "wiki/example.md",
        "---\ntitle: Example\nsummary: Example summary.\n---\nBody.\n",
    );
    repo.commit_all("init");

    let first = run_wiki(&repo, &["list"]);
    assert!(first.status.success(), "bootstrap list should succeed");

    let second = run_wiki(&repo, &["list"]);
    assert!(second.status.success(), "warm list should succeed");

    let events = log_events(&repo);
    assert!(
        !has_event(&events, "index.discover_files"),
        "warm list should avoid full-rescan discovery, got events: {events:?}"
    );
}

#[test]
fn summary_uses_fts_suggestions_for_missing_pages() {
    let repo = TestRepo::new();
    repo.create_file("wiki/wiki.toml", "");
    repo.create_file(
        "wiki/example.md",
        "---\ntitle: Example\nsummary: Example summary.\n---\nRust indexing appears here.\n",
    );
    repo.commit_all("init");

    let bootstrap = run_wiki(&repo, &["rust"]);
    assert!(
        bootstrap.status.success(),
        "bootstrap search should succeed"
    );

    repo.create_file(
        "wiki/example.md",
        "---\ntitle: Example\nsummary: Example summary.\n---\nGraph traversal appears here.\n",
    );

    let existing = run_wiki(&repo, &["summary", "Example"]);
    assert!(existing.status.success(), "existing summary should succeed");
    // With native Tantivy FTS, no explicit FTS sync step is needed.
    let events = log_events(&repo);
    assert!(
        !has_event(&events, "index.sync_search"),
        "existing summary should not emit FTS sync event, got events: {events:?}"
    );

    let missing = run_wiki(&repo, &["summary", "Graph"]);
    assert_eq!(
        missing.status.code(),
        Some(1),
        "missing summary should exit 1"
    );
    // With native Tantivy FTS, suggestions work without an explicit sync step.
    let events = log_events(&repo);
    assert!(
        !has_event(&events, "index.sync_search"),
        "missing summary should not emit FTS sync event, got events: {events:?}"
    );
    assert!(
        has_event(&events, "index.search"),
        "missing summary should perform FTS search for suggestions, got events: {events:?}"
    );
}
