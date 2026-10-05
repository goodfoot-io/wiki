//! The binary's panic policy, driven through the real executable: a panic on
//! any thread exits 2 with one concise stderr report, in the
//! `{"error": ...}` shape under `--format json`.
//!
//! Panics are forced with the debug-only `WIKI_TEST_FAULT_PANIC=main|worker`
//! injection. Release builds compile the injection out, so under
//! `cargo test --release` the injection tests are absent and
//! `release_build_ignores_the_fault_variable` runs instead.

use std::path::Path;
use std::process::Command as StdCommand;

use assert_cmd::Command;

const FAULT_VAR: &str = "WIKI_TEST_FAULT_PANIC";

/// A fresh git repository, so a run that is not faulted completes cleanly
/// (no-argument `wiki` prints help and exits 0).
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let status = StdCommand::new("git")
        .args(["init", "-q"])
        .current_dir(dir.path())
        .status()
        .expect("spawn git init");
    assert!(status.success(), "git init failed");
    dir
}

/// `wiki <args>` from `cwd`, with `fault` (if any) as the injected panic.
fn wiki(cwd: &Path, fault: Option<&str>, args: &[&str]) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_wiki"));
    cmd.current_dir(cwd)
        .env_remove(FAULT_VAR)
        .env_remove("RUST_BACKTRACE")
        .env_remove("RUST_LIB_BACKTRACE")
        .args(args);
    if let Some(fault) = fault {
        cmd.env(FAULT_VAR, fault);
    }
    cmd.output().expect("run wiki")
}

#[cfg(debug_assertions)]
mod injected {
    use super::*;

    fn assert_text_report(fault: &str, thread_message: &str) {
        let dir = repo();
        let out = wiki(dir.path(), Some(fault), &[]);
        assert_eq!(out.status.code(), Some(2), "a panic exits 2: {out:?}");
        assert!(out.stdout.is_empty(), "nothing reaches stdout: {out:?}");
        let stderr = String::from_utf8(out.stderr).expect("utf-8 stderr");
        assert_eq!(stderr.lines().count(), 1, "one concise line: {stderr:?}");
        assert!(stderr.starts_with("internal error: "), "{stderr:?}");
        assert!(stderr.contains(thread_message), "{stderr:?}");
    }

    fn assert_json_report(fault: &str, thread_message: &str) {
        let dir = repo();
        let out = wiki(dir.path(), Some(fault), &["--format", "json"]);
        assert_eq!(out.status.code(), Some(2), "a panic exits 2: {out:?}");
        assert!(out.stdout.is_empty(), "nothing reaches stdout: {out:?}");
        let stderr = String::from_utf8(out.stderr).expect("utf-8 stderr");
        assert_eq!(stderr.lines().count(), 1, "one JSON line: {stderr:?}");
        let report: serde_json::Value = serde_json::from_str(&stderr).expect("stderr is JSON");
        let object = report.as_object().expect("JSON object");
        assert_eq!(object.len(), 1, "only the error key: {report}");
        let error = object["error"].as_str().expect("error is a string");
        assert!(error.starts_with("internal error: "), "{error:?}");
        assert!(error.contains(thread_message), "{error:?}");
    }

    #[test]
    fn main_thread_panic_exits_2_with_a_text_report() {
        assert_text_report("main", "injected test panic (main thread)");
    }

    #[test]
    fn worker_thread_panic_exits_2_with_a_text_report() {
        assert_text_report("worker", "injected test panic (worker thread)");
    }

    #[test]
    fn main_thread_panic_exits_2_with_a_json_report() {
        assert_json_report("main", "injected test panic (main thread)");
    }

    #[test]
    fn worker_thread_panic_exits_2_with_a_json_report() {
        assert_json_report("worker", "injected test panic (worker thread)");
    }

    #[test]
    fn unfaulted_run_is_clean() {
        let dir = repo();
        for fault in [None, Some("unrecognised")] {
            let out = wiki(dir.path(), fault, &[]);
            assert_eq!(out.status.code(), Some(0), "{fault:?}: {out:?}");
        }
    }
}

#[cfg(not(debug_assertions))]
#[test]
fn release_build_ignores_the_fault_variable() {
    let dir = repo();
    for fault in ["main", "worker"] {
        let out = wiki(dir.path(), Some(fault), &[]);
        assert_eq!(out.status.code(), Some(0), "{fault}: {out:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!stderr.contains("internal error"), "{fault}: {stderr:?}");
    }
}
