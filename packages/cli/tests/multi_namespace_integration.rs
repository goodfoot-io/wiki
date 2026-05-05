//! Integration tests for `-n '*'` multi-namespace dispatch across the five
//! commands that support it: search (default query), links, summary, refs,
//! check.
//!
//! Layout used by every test:
//!
//!   <repo>/wiki/wiki.toml         current wiki, default namespace, peer "foo"
//!   <repo>/wiki/page.md           a page in the current wiki
//!   <repo>/foo/wiki.toml          peer wiki, namespace "foo"
//!   <repo>/foo/page.md            a page in the peer wiki
//!
//! Tests are run from <repo>/wiki so the walk-up finds the current wiki.

use std::fs;
use std::process::{Command, Output};

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

    fn create_file(&self, path: &str, content: &str) {
        let full = self.dir.path().join(path);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).expect("create_dir_all");
        }
        fs::write(full, content).expect("write file");
    }

    fn commit(&self, msg: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "--allow-empty", "-m", msg]);
    }

    fn git(&self, args: &[&str]) {
        let output = Command::new("git")
            .current_dir(self.dir.path())
            .args(args)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "t@t.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "t@t.com")
            .output()
            .expect("git");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn wiki(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_wiki"))
            .current_dir(self.dir.path().join("wiki"))
            .args(args)
            .env("WIKI_BACKGROUND_FTS", "0")
            .output()
            .expect("wiki")
    }
}

fn page(title: &str, body: &str) -> String {
    format!("---\ntitle: {title}\nsummary: {title} summary.\n---\n{body}\n")
}

fn make_basic_repo() -> TestRepo {
    let repo = TestRepo::new();
    // Current wiki — default namespace, peer "foo".
    repo.create_file("wiki/wiki.toml", "[peers]\nfoo = \"../foo\"\n");
    repo.create_file(
        "wiki/page.md",
        &page("Current Page", "Hello unique-current-token."),
    );
    // Peer foo
    repo.create_file("foo/wiki.toml", "namespace = \"foo\"\n");
    repo.create_file(
        "foo/page.md",
        &page("Foo Page", "Hello unique-foo-token."),
    );
    repo.commit("init");
    repo
}

#[test]
fn search_star_returns_results_from_all_namespaces() {
    let repo = make_basic_repo();
    let out = repo.wiki(&["-n", "*", "Page"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "stdout: {stdout}\nstderr: {}", String::from_utf8_lossy(&out.stderr));
    // Results from both namespaces, labeled.
    assert!(stdout.contains("[default]"), "expected [default] label; got: {stdout}");
    assert!(stdout.contains("[foo]"), "expected [foo] label; got: {stdout}");
    assert!(stdout.contains("Current Page"), "got: {stdout}");
    assert!(stdout.contains("Foo Page"), "got: {stdout}");
}

#[test]
fn search_star_json_includes_namespace_field() {
    let repo = make_basic_repo();
    let out = repo.wiki(&["--format", "json", "-n", "*", "Page"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "stdout: {stdout}");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    let arr = v.as_array().expect("array");
    let namespaces: Vec<String> = arr
        .iter()
        .map(|e| e["namespace"].as_str().unwrap_or("").to_string())
        .collect();
    assert!(namespaces.contains(&"default".to_string()), "got: {namespaces:?}");
    assert!(namespaces.contains(&"foo".to_string()), "got: {namespaces:?}");
}

#[test]
fn links_star_returns_results_from_all_namespaces() {
    let repo = TestRepo::new();
    repo.create_file("wiki/wiki.toml", "[peers]\nfoo = \"../foo\"\n");
    repo.create_file("wiki/target.md", &page("Target Page", "Body."));
    repo.create_file(
        "wiki/source.md",
        &page("Source Page", "See [[Target Page]]."),
    );
    repo.create_file("foo/wiki.toml", "namespace = \"foo\"\n");
    // Peer also has a Target Page and a page linking to it.
    repo.create_file("foo/target.md", &page("Target Page", "Foo body."));
    repo.create_file(
        "foo/linker.md",
        &page("Foo Linker", "Mentions [[Target Page]]."),
    );
    repo.commit("init");

    let out = repo.wiki(&["-n", "*", "links", "Target Page"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "stdout: {stdout}\nstderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("[default]"), "got: {stdout}");
    assert!(stdout.contains("[foo]"), "got: {stdout}");
    assert!(stdout.contains("Source Page"), "got: {stdout}");
    assert!(stdout.contains("Foo Linker"), "got: {stdout}");
}

#[test]
fn summary_star_emits_one_entry_per_namespace_when_title_resolves() {
    let repo = TestRepo::new();
    repo.create_file("wiki/wiki.toml", "[peers]\nfoo = \"../foo\"\n");
    repo.create_file("wiki/shared.md", &page("Shared", "default ver."));
    repo.create_file("foo/wiki.toml", "namespace = \"foo\"\n");
    repo.create_file("foo/shared.md", &page("Shared", "foo ver."));
    repo.commit("init");

    let out = repo.wiki(&["-n", "*", "summary", "Shared"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "stdout: {stdout}");
    assert!(stdout.contains("[default]"), "got: {stdout}");
    assert!(stdout.contains("[foo]"), "got: {stdout}");
}

#[test]
fn refs_star_resolves_references_per_namespace() {
    let repo = TestRepo::new();
    repo.create_file("wiki/wiki.toml", "[peers]\nfoo = \"../foo\"\n");
    repo.create_file(
        "wiki/source.md",
        &page("Source", "[[Default Target]]"),
    );
    repo.create_file("wiki/default-target.md", &page("Default Target", "x"));
    repo.create_file("foo/wiki.toml", "namespace = \"foo\"\n");
    // Peer also has a "Source" page that links to its own target.
    repo.create_file("foo/source.md", &page("Source", "[[Foo Target]]"));
    repo.create_file("foo/foo-target.md", &page("Foo Target", "y"));
    repo.commit("init");

    let out = repo.wiki(&["-n", "*", "refs", "Source"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "stdout: {stdout}\nstderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("[default]"), "got: {stdout}");
    assert!(stdout.contains("[foo]"), "got: {stdout}");
    assert!(stdout.contains("Default Target"), "got: {stdout}");
    assert!(stdout.contains("Foo Target"), "got: {stdout}");
}

#[test]
fn check_star_runs_rules_across_all_namespaces() {
    let repo = TestRepo::new();
    // Current wiki: clean; declares peer foo.
    repo.create_file("wiki/wiki.toml", "[peers]\nfoo = \"../foo\"\n");
    repo.create_file("wiki/clean.md", &page("Clean", "ok."));
    // Peer wiki: contains a broken wikilink — plain `wiki check` now
    // defaults to all wikis, so both paths surface the peer error.
    repo.create_file("foo/wiki.toml", "namespace = \"foo\"\n");
    repo.create_file(
        "foo/broken.md",
        &page("Broken Page", "[[Nonexistent Target]]"),
    );
    repo.commit("init");

    // Plain `check` — now defaults to all wikis, so it surfaces the peer's broken wikilink.
    let out_default = repo.wiki(&["check"]);
    let stdout_default = String::from_utf8_lossy(&out_default.stdout);
    assert!(
        !out_default.status.success(),
        "expected plain check to surface peer error; stdout: {stdout_default}"
    );
    assert!(stdout_default.contains("[foo]"), "expected [foo] label; got: {stdout_default}");
    assert!(
        stdout_default.contains("Broken Wikilink"),
        "expected Broken Wikilink diagnostic; got: {stdout_default}"
    );

    // `-n '*'` — explicit multi-namespace, same behavior.
    let out_star = repo.wiki(&["-n", "*", "check"]);
    let stdout_star = String::from_utf8_lossy(&out_star.stdout);
    assert!(
        !out_star.status.success(),
        "expected -n '*' check to surface peer error; stdout: {stdout_star}"
    );
    assert!(stdout_star.contains("[foo]"), "expected [foo] label; got: {stdout_star}");
    assert!(
        stdout_star.contains("Broken Wikilink"),
        "expected Broken Wikilink diagnostic; got: {stdout_star}"
    );
}

#[test]
fn star_on_unsupported_command_errors_clearly() {
    let repo = make_basic_repo();
    let out = repo.wiki(&["-n", "*", "extract"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "expected non-zero exit");
    assert!(
        stderr.contains("multi-namespace") || stderr.contains("does not support"),
        "expected clear error; stderr: {stderr}"
    );
}

// ── Cross-namespace inbound links: peer namespace links to default page ──────

/// `wiki links "Authentication" -n '*'` should include backlinks from peer
/// namespaces that contain `[[default:Authentication]]`, even though those
/// peer namespaces don't OWN an `Authentication` page.
#[test]
fn links_star_surfaces_cross_namespace_inbound_links() {
    let repo = TestRepo::new();
    // Default wiki — declares peer "scratch" at ../scratch.
    repo.create_file("wiki/wiki.toml", "[peers]\nscratch = \"../scratch\"\n");
    repo.create_file(
        "wiki/authentication.md",
        &page("Authentication", "Auth body."),
    );
    // Peer "scratch" — has its own wiki.toml declaring namespace, and a page
    // that links into the default namespace via `[[default:Authentication]]`.
    repo.create_file("scratch/wiki.toml", "namespace = \"scratch\"\n");
    repo.create_file(
        "scratch/notes/operator-notes.md",
        &page(
            "Operator Notes",
            "Refers to [[default:Authentication]] for login flow.",
        ),
    );
    repo.commit("init");

    let out = repo.wiki(&["-n", "*", "links", "Authentication"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "stdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("Operator Notes"),
        "expected cross-namespace inbound link from scratch:Operator Notes; stdout: {stdout}\nstderr: {stderr}"
    );
}

// ── F1: tagged float surfaces in matching peer's index ────────────────────────

/// A `*.wiki.md` file with `namespace: foo` in its frontmatter must appear in
/// the peer `foo` index (`wiki -n foo list`) but NOT in the default index.
#[test]
fn f1_tagged_float_surfaces_in_matching_peer_index() {
    let repo = TestRepo::new();
    repo.create_file("wiki/wiki.toml", "[peers]\nfoo = \"../foo\"\n");
    repo.create_file("wiki/page.md", &page("Default Page", "default content."));
    repo.create_file("foo/wiki.toml", "namespace = \"foo\"\n");
    repo.create_file("foo/page.md", &page("Foo Page", "foo content."));
    // Float: lives outside both wiki roots, tagged for peer "foo".
    repo.create_file(
        "floats/notes.wiki.md",
        "---\ntitle: Float Notes\nsummary: Notes float.\nnamespace: foo\n---\nFloat body.\n",
    );
    repo.commit("init");

    // Peer "foo" list should include the float.
    let out = repo.wiki(&["-n", "foo", "list"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "wiki -n foo list failed; stdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("Float Notes"),
        "tagged float must appear in peer index; stdout: {stdout}"
    );

    // Default list must NOT include the float.
    let out_default = repo.wiki(&["list"]);
    let stdout_default = String::from_utf8_lossy(&out_default.stdout);
    assert!(
        !stdout_default.contains("Float Notes"),
        "tagged float must NOT appear in default index; stdout: {stdout_default}"
    );
}

// ── F2: untagged float appears in default only ────────────────────────────────

/// A `*.wiki.md` with NO `namespace:` frontmatter must appear in the default
/// index but NOT in any peer's index.
#[test]
fn f2_untagged_float_in_default_only() {
    let repo = TestRepo::new();
    repo.create_file("wiki/wiki.toml", "[peers]\nfoo = \"../foo\"\n");
    repo.create_file("wiki/page.md", &page("Default Page", "default content."));
    repo.create_file("foo/wiki.toml", "namespace = \"foo\"\n");
    repo.create_file("foo/page.md", &page("Foo Page", "foo content."));
    // Untagged float — no namespace frontmatter.
    repo.create_file(
        "floats/untagged.wiki.md",
        "---\ntitle: Untagged Float\nsummary: Untagged float.\n---\nBody.\n",
    );
    repo.commit("init");

    // Default list should include the untagged float.
    let out = repo.wiki(&["list"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "wiki list failed; stdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("Untagged Float"),
        "untagged float must appear in default index; stdout: {stdout}"
    );

    // Peer "foo" list must NOT include the untagged float.
    let out_peer = repo.wiki(&["-n", "foo", "list"]);
    let stdout_peer = String::from_utf8_lossy(&out_peer.stdout);
    assert!(
        !stdout_peer.contains("Untagged Float"),
        "untagged float must NOT appear in peer index; stdout: {stdout_peer}"
    );
}

// ── F5: stale index entry removed on namespace frontmatter mutation ───────────

/// Changing a `*.wiki.md` file's `namespace:` frontmatter from a peer namespace
/// to untagged (or to a different namespace) must remove the stale row from the
/// prior namespace's index.
#[test]
fn f5_namespace_mutation_removes_stale_row_from_prior_index() {
    let repo = TestRepo::new();
    repo.create_file("wiki/wiki.toml", "[peers]\nfoo = \"../foo\"\n");
    repo.create_file("wiki/page.md", &page("Default Page", "default."));
    repo.create_file("foo/wiki.toml", "namespace = \"foo\"\n");
    repo.create_file("foo/page.md", &page("Foo Page", "foo."));
    // Float tagged for peer "foo".
    repo.create_file(
        "floats/mutable.wiki.md",
        "---\ntitle: Mutable Float\nsummary: Before mutation.\nnamespace: foo\n---\nBefore.\n",
    );
    repo.commit("initial with namespace: foo");

    // Confirm it appears in peer "foo" index.
    let out = repo.wiki(&["-n", "foo", "list"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Mutable Float"),
        "float must be in foo index before mutation; stdout: {stdout}"
    );

    // Mutate: remove namespace frontmatter (now untagged → default only).
    repo.create_file(
        "floats/mutable.wiki.md",
        "---\ntitle: Mutable Float\nsummary: After mutation.\n---\nAfter.\n",
    );
    repo.commit("remove namespace frontmatter");

    // After mutation: float must NOT appear in peer "foo" index.
    let out_peer = repo.wiki(&["-n", "foo", "list"]);
    let stdout_peer = String::from_utf8_lossy(&out_peer.stdout);
    assert!(
        !stdout_peer.contains("Mutable Float"),
        "stale row must be removed from foo index after namespace mutation; stdout: {stdout_peer}"
    );

    // And must appear in default index.
    let out_default = repo.wiki(&["list"]);
    let stdout_default = String::from_utf8_lossy(&out_default.stdout);
    assert!(
        stdout_default.contains("Mutable Float"),
        "float must appear in default index after removing namespace; stdout: {stdout_default}"
    );
}
