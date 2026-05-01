//! Integration tests for `wiki namespaces` and `wiki init`.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

// ── TestRepo ──────────────────────────────────────────────────────────────────

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

    fn git(&self, args: &[&str]) {
        let output = Command::new("git")
            .current_dir(self.dir.path())
            .args(args)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .expect("spawn git");
        assert!(
            output.status.success(),
            "git {:?} failed:\n{}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// Run `wiki <args>` from `cwd_rel` (a path relative to the repo root).
    /// Returns the raw Output. Does NOT assert success.
    fn run_from(&self, cwd_rel: &str, args: &[&str]) -> Output {
        let cwd = if cwd_rel.is_empty() {
            self.dir.path().to_path_buf()
        } else {
            self.dir.path().join(cwd_rel)
        };
        Command::new(env!("CARGO_BIN_EXE_wiki"))
            .current_dir(&cwd)
            .args(args)
            .env("WIKI_BACKGROUND_FTS", "0")
            .output()
            .expect("run wiki")
    }
}

// ── `wiki namespaces` tests ───────────────────────────────────────────────────

/// Happy path: a single default-namespace wiki.
#[test]
fn namespaces_happy_path_single_default() {
    let repo = TestRepo::new();
    repo.create_file("wiki/wiki.toml", "");

    let out = repo.run_from("wiki", &["namespaces"]);
    assert!(
        out.status.success(),
        "expected exit 0, got {:?}\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("default\t"),
        "expected 'default' row, got: {stdout}"
    );
}

/// Happy path: a default wiki and a named wiki are both listed.
#[test]
fn namespaces_lists_multiple_wikis() {
    let repo = TestRepo::new();
    repo.create_file("wiki/wiki.toml", "");
    repo.create_file("foo-wiki/wiki.toml", "namespace = \"foo\"\n");

    let out = repo.run_from("wiki", &["namespaces"]);
    assert!(
        out.status.success(),
        "expected exit 0, got {:?}\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("default\t"), "expected default row, got: {stdout}");
    assert!(stdout.contains("foo\t"), "expected foo row, got: {stdout}");
}

/// Two wikis declaring the same namespace name: exit 1.
#[test]
fn namespaces_duplicate_namespace_exits_nonzero() {
    let repo = TestRepo::new();
    repo.create_file("a/wiki.toml", "namespace = \"shared\"\n");
    repo.create_file("b/wiki.toml", "namespace = \"shared\"\n");

    let out = repo.run_from("", &["namespaces"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "expected exit 1, got {:?}\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

/// A malformed wiki.toml hard-fails: exit 2, diagnostic on stderr naming the
/// broken file, and no row labelled `default` (or any other namespace) is
/// printed for the unparseable file.
#[test]
fn namespaces_malformed_wiki_toml_fails_closed() {
    let repo = TestRepo::new();
    repo.create_file("wiki/wiki.toml", "");
    repo.create_file("bad/wiki.toml", "this is = not = valid = toml\n");

    let out = repo.run_from("", &["namespaces"]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "expected exit 2 for malformed toml; stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("bad/wiki.toml") || stderr.contains("bad\\wiki.toml"),
        "expected stderr to name broken file path; stderr: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("default"),
        "broken file must not be rendered as a `default` row; stdout: {stdout}"
    );
}

/// `--format json` likewise hard-fails on a malformed wiki.toml.
#[test]
fn namespaces_json_malformed_wiki_toml_fails_closed() {
    let repo = TestRepo::new();
    repo.create_file("wiki/wiki.toml", "");
    repo.create_file("bad/wiki.toml", "this is = not = valid = toml\n");

    let out = repo.run_from("", &["namespaces", "--format", "json"]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "expected exit 2 for malformed toml; stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

/// `--format json` emits an array with namespace/path/status fields.
#[test]
fn namespaces_json_format() {
    let repo = TestRepo::new();
    repo.create_file("wiki/wiki.toml", "");
    repo.create_file("foo-wiki/wiki.toml", "namespace = \"foo\"\n");

    let out = repo.run_from("wiki", &["namespaces", "--format", "json"]);
    assert!(
        out.status.success(),
        "expected exit 0, got {:?}\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("valid JSON output");
    let arr = parsed.as_array().expect("JSON array");
    assert!(arr.len() >= 2, "expected at least 2 entries");

    for entry in arr {
        assert!(entry.get("path").is_some(), "missing 'path' in {entry}");
        assert!(entry.get("status").is_some(), "missing 'status' in {entry}");
    }

    let foo = arr
        .iter()
        .find(|e| e["namespace"] == "foo")
        .expect("expected foo entry");
    assert_eq!(foo["status"], "ok");
}

// ── `wiki init` tests ─────────────────────────────────────────────────────────

/// `wiki init` with no arg writes an empty wiki.toml.
#[test]
fn init_creates_empty_wiki_toml() {
    let repo = TestRepo::new();
    fs::create_dir_all(repo.path().join("new-wiki")).expect("mkdir");

    let out = repo.run_from("new-wiki", &["init"]);
    assert!(
        out.status.success(),
        "expected exit 0, got {:?}\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    let toml_path = repo.path().join("new-wiki/wiki.toml");
    assert!(toml_path.exists(), "wiki.toml must exist after init");
    let content = fs::read_to_string(&toml_path).expect("read wiki.toml");
    assert!(
        content.is_empty() || !content.contains("namespace"),
        "empty init must not write a namespace key; got: {content:?}"
    );
}

/// `wiki init <namespace>` writes `namespace = "<arg>"`.
#[test]
fn init_with_namespace_writes_namespace_field() {
    let repo = TestRepo::new();
    fs::create_dir_all(repo.path().join("ns-wiki")).expect("mkdir");

    let out = repo.run_from("ns-wiki", &["init", "myns"]);
    assert!(
        out.status.success(),
        "expected exit 0, got {:?}\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    let toml_path = repo.path().join("ns-wiki/wiki.toml");
    assert!(toml_path.exists(), "wiki.toml must exist after init");
    let content = fs::read_to_string(&toml_path).expect("read wiki.toml");
    assert!(
        content.contains("namespace = \"myns\""),
        "expected namespace field, got: {content:?}"
    );
}

/// `wiki init` fails closed when wiki.toml already exists.
#[test]
fn init_fails_if_wiki_toml_exists() {
    let repo = TestRepo::new();
    repo.create_file("existing-wiki/wiki.toml", "namespace = \"old\"\n");

    let out = repo.run_from("existing-wiki", &["init"]);
    assert!(
        !out.status.success(),
        "expected non-zero exit when wiki.toml exists, got success\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    // The existing file must not be overwritten.
    let content = fs::read_to_string(repo.path().join("existing-wiki/wiki.toml"))
        .expect("read wiki.toml");
    assert!(
        content.contains("namespace = \"old\""),
        "existing wiki.toml must not be overwritten, got: {content:?}"
    );
}

/// `wiki init` works in a directory with no ancestor wiki.toml (does not
/// require a loaded WikiConfig).
#[test]
fn init_works_without_any_wiki_toml_in_tree() {
    // Create a plain temp dir that is NOT a git repo — this ensures init
    // doesn't depend on WikiConfig::load or git::repo_root.
    // We need a git repo for the CLI to determine repo_root; use a fresh one.
    let repo = TestRepo::new();
    // No wiki.toml anywhere. Run init from the repo root.
    let out = repo.run_from("", &["init", "fresh"]);
    assert!(
        out.status.success(),
        "expected exit 0 when no wiki.toml exists anywhere, got {:?}\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let toml_path = repo.path().join("wiki.toml");
    assert!(toml_path.exists(), "wiki.toml must be created at cwd");
    let content = fs::read_to_string(&toml_path).expect("read wiki.toml");
    assert!(
        content.contains("namespace = \"fresh\""),
        "expected namespace = fresh, got: {content:?}"
    );
}

// ── F6: wiki init namespace validation ────────────────────────────────────────

/// `wiki init default` must be rejected (reserved literal).
#[test]
fn init_rejects_reserved_default_namespace() {
    let repo = TestRepo::new();
    fs::create_dir_all(repo.path().join("w")).unwrap();
    let out = repo.run_from("w", &["init", "default"]);
    assert!(
        !out.status.success(),
        "expected non-zero exit for reserved namespace 'default'\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    // No file should have been written.
    assert!(
        !repo.path().join("w/wiki.toml").exists(),
        "wiki.toml must not be created after rejection"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("reserved") || combined.contains("default"),
        "expected 'reserved' in output, got: {combined}"
    );
}

/// `wiki init` rejects namespaces with invalid characters (spaces, slashes, etc.)
#[test]
fn init_rejects_namespace_with_invalid_chars() {
    let repo = TestRepo::new();
    fs::create_dir_all(repo.path().join("w")).unwrap();
    // Space in namespace
    let out = repo.run_from("w", &["init", "foo bar"]);
    assert!(
        !out.status.success(),
        "expected non-zero exit for namespace with space\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        !repo.path().join("w/wiki.toml").exists(),
        "wiki.toml must not be created after rejection"
    );
}

/// `wiki init` accepts valid namespace characters (letters, digits, `-`, `_`).
#[test]
fn init_accepts_valid_namespace_charset() {
    let repo = TestRepo::new();
    fs::create_dir_all(repo.path().join("w")).unwrap();
    let out = repo.run_from("w", &["init", "my-wiki_1"]);
    assert!(
        out.status.success(),
        "expected exit 0 for valid namespace\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let content = fs::read_to_string(repo.path().join("w/wiki.toml")).unwrap();
    assert!(content.contains("namespace = \"my-wiki_1\""), "got: {content:?}");
}
