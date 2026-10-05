//! A stdout closed by its reader (`wiki … | head -0`) is an I/O condition,
//! not an internal error: every command that writes to stdout exits 2 with
//! nothing on stderr — no panic, no "internal error", no "Broken pipe".
//!
//! Each closed-pipe run gets a pipe whose reader is dropped BEFORE the child
//! is spawned, so the child's first stdout write deterministically fails
//! with `EPIPE` (Rust ignores `SIGPIPE`, so it surfaces as `BrokenPipe`).
//! The control tests pin each command's normal output with an open pipe, so
//! the fallible-write conversion is proven byte-for-byte unchanged.

use std::fs;
use std::io;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

/// A committed git repository holding a clean page (`Alpha`) and a page
/// with a broken link (`Beta`), so `check` has a finding to print.
fn repo() -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .current_dir(root)
            .args(args)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .expect("spawn git");
        assert!(out.status.success(), "git {args:?}: {out:?}");
    };
    git(&["init", "-q"]);
    git(&["checkout", "-q", "-b", "main"]);
    fs::create_dir_all(root.join("wiki")).expect("create wiki dir");
    fs::write(
        root.join("wiki/alpha.md"),
        "---\ntitle: Alpha\nsummary: The alpha page about widgets.\n---\nAlpha body about widgets.\n",
    )
    .expect("write alpha");
    fs::write(
        root.join("wiki/beta.md"),
        "---\ntitle: Beta\nsummary: The beta page.\n---\nSee [missing](missing.md).\n",
    )
    .expect("write beta");
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "init"]);
    dir
}

fn wiki(cwd: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_wiki"));
    cmd.current_dir(cwd)
        .args(args)
        .env("WIKI_BACKGROUND_FTS", "0")
        .env_remove("WIKI_PERF")
        .env_remove("WIKI_TEST_FAULT_PANIC")
        .stdin(Stdio::null());
    cmd
}

/// Run `wiki <args>` with stdout already closed by its reader.
fn run_with_closed_stdout(cwd: &Path, args: &[&str]) -> Output {
    let (reader, writer) = io::pipe().expect("create pipe");
    drop(reader);
    wiki(cwd, args)
        .stdout(writer)
        .stderr(Stdio::piped())
        .output()
        .expect("run wiki")
}

/// Every invocation here writes to stdout on its normal path.
const STDOUT_WRITERS: &[&[&str]] = &[
    &["widgets"],
    &["--format", "json", "widgets"],
    &["summary", "Alpha"],
    &["--format", "json", "summary", "Alpha"],
    &["check"],
    &["check", "--format", "json"],
    &["check", "--fix", "--fix-dry-run"],
    &["check", "--clear-cache"],
    &["list"],
    &["list", "--format", "json"],
    &["--version"],
    &["--help"],
    &[],
];

#[test]
fn closed_stdout_exits_2_silently_for_every_command() {
    let dir = repo();
    for args in STDOUT_WRITERS {
        let out = run_with_closed_stdout(dir.path(), args);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(
            out.status.code(),
            Some(2),
            "wiki {args:?}: a closed stdout exits 2 (stderr: {stderr:?})"
        );
        assert!(
            stderr.is_empty(),
            "wiki {args:?}: nothing on stderr — no panic, no internal error, \
             no broken-pipe report: {stderr:?}"
        );
    }
}

/// The normal run of each command, with `<ROOT>` standing in for the
/// temporary repository's path.
fn normal_run(dir: &TempDir, args: &[&str]) -> (Option<i32>, String, String) {
    let out = wiki(dir.path(), args).output().expect("run wiki");
    let root = dir.path().canonicalize().expect("canonical root");
    let scrub = |bytes: &[u8]| {
        String::from_utf8(bytes.to_vec())
            .expect("utf-8 output")
            .replace(&*root.to_string_lossy(), "<ROOT>")
            .replace(&*dir.path().to_string_lossy(), "<ROOT>")
    };
    (out.status.code(), scrub(&out.stdout), scrub(&out.stderr))
}

fn version() -> String {
    let package_json = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/package.json"))
        .expect("read package.json");
    let parsed: serde_json::Value =
        serde_json::from_str(&package_json).expect("parse package.json");
    parsed["version"]
        .as_str()
        .expect("version string")
        .to_owned()
}

#[test]
fn open_stdout_output_is_unchanged() {
    let dir = repo();
    let cases: &[(&[&str], i32, &str)] = &[
        (
            &["widgets"],
            0,
            "# Alpha\n## wiki/alpha.md\nThe alpha page about widgets.\n\nMatched snippets:\n- L0: Alpha body about widgets.\n\n",
        ),
        (
            &["--format", "json", "widgets"],
            0,
            "[\n  {\n    \"title\": \"Alpha\",\n    \"file\": \"<ROOT>/wiki/alpha.md\",\n    \"summary\": \"The alpha page about widgets.\",\n    \"snippets\": [\n      {\n        \"line\": 0,\n        \"text\": \"Alpha body about widgets.\\n\"\n      }\n    ]\n  }\n]\n",
        ),
        (
            &["summary", "Alpha"],
            0,
            "# Alpha\n## wiki/alpha.md\nThe alpha page about widgets.\n",
        ),
        (
            &["--format", "json", "summary", "Alpha"],
            0,
            "{\n  \"title\": \"Alpha\",\n  \"file\": \"<ROOT>/wiki/alpha.md\",\n  \"summary\": \"The alpha page about widgets.\"\n}\n",
        ),
        (
            &["check"],
            1,
            "Error: Broken Link\n- <ROOT>/wiki/beta.md:5\n- File `missing.md` not found.\n",
        ),
        (
            &["check", "--format", "json"],
            1,
            "{\n  \"errors\": [\n    {\n      \"file\": \"<ROOT>/wiki/beta.md\",\n      \"kind\": \"broken_link\",\n      \"line\": 5,\n      \"message\": \"File `missing.md` not found.\"\n    }\n  ]\n}\n",
        ),
        (
            &["list"],
            0,
            "**Alpha** — `wiki/alpha.md`\n\nThe alpha page about widgets.\n\n---\n\n**Beta** — `wiki/beta.md`\n\nThe beta page.\n\n---\n\n",
        ),
        (
            &["list", "--format", "json"],
            0,
            "[{\"title\":\"Alpha\",\"aliases\":[],\"tags\":[],\"summary\":\"The alpha page about widgets.\",\"file\":\"wiki/alpha.md\"},{\"title\":\"Beta\",\"aliases\":[],\"tags\":[],\"summary\":\"The beta page.\",\"file\":\"wiki/beta.md\"}]\n",
        ),
    ];
    for (args, code, stdout) in cases {
        let (got_code, got_stdout, got_stderr) = normal_run(&dir, args);
        assert_eq!(got_code, Some(*code), "wiki {args:?} exit code");
        assert_eq!(got_stdout, *stdout, "wiki {args:?} stdout");
        assert_eq!(got_stderr, "", "wiki {args:?} stderr");
    }

    let (code, stdout, stderr) = normal_run(&dir, &["--version"]);
    assert_eq!(code, Some(0));
    assert_eq!(stdout, format!("wiki {}\n", version()));
    assert_eq!(stderr, "");
}

#[test]
fn open_stdout_help_and_usage_error_are_unchanged() {
    let dir = repo();
    let version = version();

    let (code, stdout, stderr) = normal_run(&dir, &["--help"]);
    assert_eq!(code, Some(0), "--help exits 0");
    assert_eq!(stderr, "", "--help writes nothing to stderr");
    let long_help = format!(
        "wiki {version}\n\n\n\
wiki - Read and maintain wiki pages\n\n\
Pass a query to search wiki pages with weighted ranking:\n  wiki [query]\n\n\
With no arguments, wiki prints help and the wiki README when available.\n\n\
Stdin is read when no argument is given for commands that accept it:\n  echo wiki/page.md | wiki summary\n\n\
Command names (check, list, summary) are reserved and cannot be used as page titles.\n\n\
File selection follows the current working directory; links and anchors resolve against the git repository root.\n\n\
Usage: wiki [OPTIONS] [query] [COMMAND]\n\n\
Commands:\n\
\x20 check    Validate all links and frontmatter in wiki pages\n\
\x20 list     List all wiki pages with metadata (title, aliases, tags, file path)\n\
\x20 summary  Print the summary of a wiki page\n\n\
Arguments:\n\
\x20 [query]\n\
\x20         Search query for the default wiki lookup\n\n\
Options:\n\
\x20     --format <FORMAT>\n\
\x20         Output structured JSON instead of human-readable text\n\
\x20         \n\
\x20         [possible values: json]\n\n\
\x20 -v, --version\n\
\x20         Print the wiki CLI version\n\n\
\x20     --perf\n\
\x20         Emit per-event timings to stderr (also enabled by `WIKI_PERF=1`)\n\n\
\x20     --source <SOURCE>\n\
\x20         Document source: working tree (default), git index, or HEAD commit\n\
\x20         \n\
\x20         [default: worktree]\n\
\x20         [possible values: worktree, index, head]\n\n\
\x20 -l, --limit <LIMIT>\n\
\x20         Maximum number of search results to print\n\
\x20         \n\
\x20         [default: 3]\n\n\
\x20 -o, --offset <OFFSET>\n\
\x20         Skip the first N search results (for pagination)\n\
\x20         \n\
\x20         [default: 0]\n\n\
\x20 -h, --help\n\
\x20         Print help (see a summary with '-h')\n"
    );
    assert_eq!(stdout, long_help, "--help bytes");

    let (code, stdout, stderr) = normal_run(&dir, &["--bogus"]);
    assert_eq!(code, Some(2), "a usage error exits 2");
    assert_eq!(stdout, "", "a usage error writes nothing to stdout");
    assert_eq!(
        stderr,
        "error: unexpected argument '--bogus' found\n\n\
\x20 tip: to pass '--bogus' as a value, use '-- --bogus'\n\n\
Usage: wiki [OPTIONS] [query] [COMMAND]\n\n\
For more information, try '--help'.\n",
        "usage-error bytes"
    );
}
