//! Regression tests for `wiki hook` (PostToolUse:Edit hook).
//!
//! The hook must silently return exit 0 when given a `.md` file that is
//! outside the wiki scope (i.e., not under `$WIKI_DIR` and not a
//! `*.wiki.md` file). Only wiki pages should be validated.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

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
}

/// A non-wiki `.md` file (not under `$WIKI_DIR`, not `*.wiki.md`) must be
/// silently skipped: `wiki hook` should produce no output and exit 0.
///
/// This test FAILS before the bug is fixed because `hook_check::run()` passes
/// the file path as an explicit glob to `check::collect`, bypassing the
/// wiki-scope filter in `discover_files`. As a result the non-wiki file is
/// validated and a `systemMessage` is printed to stdout.
#[test]
fn hook_check_skips_non_wiki_md_file() {
    let repo = TestRepo::new();

    // Create a plain README that is NOT a wiki page (no frontmatter, not
    // under wiki/ and not named *.wiki.md).
    let readme_path = repo.path().join("src/README.md");
    fs::create_dir_all(readme_path.parent().unwrap()).expect("mkdir src/");
    fs::write(
        &readme_path,
        "# README\n\nThis is not a wiki page — no frontmatter.\n",
    )
    .expect("write README");

    // The CLI walks up from cwd to find a wiki.toml. Provide one in the wiki/
    // directory so `WikiConfig::load` succeeds; the README at src/README.md
    // is outside the wiki and must be skipped.
    fs::create_dir_all(repo.path().join("wiki")).expect("mkdir wiki/");
    fs::write(repo.path().join("wiki/wiki.toml"), "").expect("write wiki.toml");

    // PostToolUse JSON payload that Claude Code sends to the hook.
    let payload = serde_json::json!({
        "tool_input": {
            "file_path": readme_path.to_str().expect("path utf8")
        }
    });

    let binary = env!("CARGO_BIN_EXE_wiki");
    let mut child = Command::new(binary)
        .arg("hook")
        .current_dir(repo.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn wiki binary");

    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.to_string().as_bytes())
        .expect("write stdin");

    let output = child.wait_with_output().expect("wait");

    assert!(
        output.status.success(),
        "exit {:?}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");

    // The hook must produce no output when given a non-wiki .md file.
    assert!(
        stdout.trim().is_empty(),
        "hook must produce no output for a non-wiki .md file, got: {stdout}"
    );
}

// ── Hook parity with `wiki check` owning-namespace routing ──────────────────
//
// Helpers and fixtures for asserting the hook respects the same owning-ns
// model that `wiki check` is required to honor.

fn run_hook(repo: &TestRepo, file_abs: &Path) -> (bool, String) {
    let payload = serde_json::json!({
        "tool_input": { "file_path": file_abs.to_str().unwrap() }
    });
    let mut child = Command::new(env!("CARGO_BIN_EXE_wiki"))
        .arg("hook")
        .current_dir(repo.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn wiki hook");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.to_string().as_bytes())
        .expect("write stdin");
    let output = child.wait_with_output().expect("wait");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    )
}

fn write(repo: &TestRepo, rel: &str, content: &str) {
    let p = repo.path().join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, content).unwrap();
}

/// H1 (❌ demonstrates bug): editing an untagged float (default-owned) that
/// contains a wikilink unresolvable in default must produce a single
/// systemMessage attributed to the default namespace. Today the hook silently
/// skips floats located outside every wiki root, so the file is never
/// validated even though `wiki check` would (and does) report it.
#[test]
fn h1_hook_validates_untagged_float_under_default() {
    let repo = TestRepo::new();
    write(&repo, "wiki/wiki.toml", "[peers]\nfoo = \"../foo\"\n");
    write(
        &repo,
        "wiki/index.md",
        "---\ntitle: Default Index\nsummary: D.\n---\nHi.\n",
    );
    write(&repo, "foo/wiki.toml", "namespace = \"foo\"\n");
    write(
        &repo,
        "foo/target.md",
        "---\ntitle: Target\nsummary: T.\n---\nT.\n",
    );
    let float_path = repo.path().join("floats/notes.wiki.md");
    write(
        &repo,
        "floats/notes.wiki.md",
        // Target only exists in foo; from default-owned float this is broken.
        "---\ntitle: Notes\nsummary: S.\n---\nSee [[Target]].\n",
    );

    let (success, stdout) = run_hook(&repo, &float_path);
    assert!(success, "hook must exit 0; stdout: {stdout}");
    // The hook output format is `  <file>:<line> [<kind>] <message>` — it
    // does not include a namespace label, so attribution is implicit (the
    // hook picks one target_wiki and only that wiki evaluates the file).
    // We assert exactly-once occurrence and that the float's path is
    // present, ensuring the file was actually validated rather than an
    // unrelated hit being matched.
    let occurrences = stdout.matches("broken_wikilink").count();
    assert_eq!(
        occurrences, 1,
        "hook must report the broken wikilink exactly once; stdout: {stdout}"
    );
    assert!(
        stdout.contains("floats/notes.wiki.md"),
        "diagnostic must reference the edited float; stdout: {stdout}"
    );
}

/// H2 (❌ demonstrates bug): editing a `*.wiki.md` file *inside* the foo peer
/// root with a path-style link to a foo target must produce a single hook
/// systemMessage with one `markdown_link_to_wiki` mention — not duplicated
/// across namespaces.
#[test]
fn h2_hook_reports_single_diagnostic_for_peer_root_float() {
    let repo = TestRepo::new();
    write(&repo, "wiki/wiki.toml", "[peers]\nfoo = \"../foo\"\n");
    write(
        &repo,
        "wiki/index.md",
        "---\ntitle: Default Index\nsummary: D.\n---\nHi.\n",
    );
    write(&repo, "foo/wiki.toml", "namespace = \"foo\"\n");
    write(
        &repo,
        "foo/target.md",
        "---\ntitle: Target\nsummary: T.\n---\nT.\n",
    );
    let float_path = repo.path().join("foo/notes/x.wiki.md");
    write(
        &repo,
        "foo/notes/x.wiki.md",
        "---\ntitle: Foo Notes\nsummary: S.\n---\nSee [Target](../target.md).\n",
    );

    let (success, stdout) = run_hook(&repo, &float_path);
    assert!(success, "hook must exit 0; stdout: {stdout}");
    let occurrences = stdout.matches("markdown_link_to_wiki").count();
    assert_eq!(
        occurrences, 1,
        "hook must report the path-link diagnostic exactly once; stdout: {stdout}"
    );
}
