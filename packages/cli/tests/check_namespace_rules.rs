//! Integration tests for `wiki check` namespace rules 5 and 6.
//!
//! Rule 5: `*.wiki.md` with `namespace: bogus` (not current, not a peer)
//!         → `namespace_undeclared` diagnostic; exit non-zero.
//!
//! Rule 6: `[[ns:Article]]` wikilinks where:
//!   - `ns` is not declared as a peer → `cross_namespace_wikilink_unresolved`
//!   - peer exists but `Article` is missing from it → `cross_namespace_wikilink_unresolved`
//!   - peer exists and `Article` exists → no diagnostic

use std::fs;
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

    fn create_file(&self, path: &str, content: &str) {
        let full = self.dir.path().join(path);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).expect("create_dir_all");
        }
        fs::write(full, content).expect("write file");
    }

    fn commit(&self, msg: &str) {
        self.git(&["add", "-A"]);
        self.git(&[
            "commit",
            "--allow-empty",
            "-m",
            msg,
        ]);
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

    /// Run `wiki check` from `cwd_rel` (relative to repo root). Does NOT assert success.
    fn run_check_from(&self, cwd_rel: &str) -> Output {
        let cwd = if cwd_rel.is_empty() {
            self.dir.path().to_path_buf()
        } else {
            self.dir.path().join(cwd_rel)
        };
        Command::new(env!("CARGO_BIN_EXE_wiki"))
            .current_dir(&cwd)
            .args(["check"])
            .env("WIKI_BACKGROUND_FTS", "0")
            .output()
            .expect("run wiki check")
    }
}

fn make_wiki_page(title: &str, body: &str) -> String {
    format!("---\ntitle: {title}\nsummary: A page about {title}.\n---\n{body}")
}

fn make_wiki_page_with_namespace(title: &str, namespace: &str) -> String {
    format!(
        "---\ntitle: {title}\nsummary: A page about {title}.\nnamespace: {namespace}\n---\n"
    )
}

// ── Rule 5 tests ──────────────────────────────────────────────────────────────

/// A `*.wiki.md` with `namespace: bogus` (not declared) → exit 1 with
/// `namespace_undeclared` diagnostic.
#[test]
fn rule5_undeclared_namespace_in_wiki_md_exits_1() {
    let repo = TestRepo::new();
    // Current wiki: default namespace, no peers.
    repo.create_file("wiki/wiki.toml", "");
    // A *.wiki.md that claims to belong to a namespace not declared anywhere.
    repo.create_file(
        "src/feature.wiki.md",
        &make_wiki_page_with_namespace("Feature", "bogus"),
    );
    repo.commit("init");

    let out = repo.run_check_from("wiki");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "expected exit 1; stdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("namespace_undeclared"),
        "expected namespace_undeclared diagnostic; stdout: {stdout}"
    );
}

/// A `*.wiki.md` with `namespace: foo` where `foo` is the current wiki's own
/// namespace → NOT a rule-5 violation.
#[test]
fn rule5_namespace_matches_current_wiki_is_valid() {
    let repo = TestRepo::new();
    // Current wiki with namespace = "foo"
    repo.create_file("wiki/wiki.toml", "namespace = \"foo\"\n");
    // A wiki page to avoid "no files" edge case
    repo.create_file("wiki/page.md", &make_wiki_page("Page", "Hello."));
    // A *.wiki.md that correctly claims namespace "foo"
    repo.create_file(
        "src/feature.wiki.md",
        &make_wiki_page_with_namespace("Feature", "foo"),
    );
    repo.commit("init");

    let out = repo.run_check_from("wiki");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "expected exit 0; stdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        !stdout.contains("namespace_undeclared"),
        "namespace matching current wiki must not produce namespace_undeclared; stdout: {stdout}"
    );
}

/// A `*.wiki.md` with `namespace: foo` where `foo` is a declared peer alias
/// → NOT a rule-5 violation.
#[test]
fn rule5_namespace_matches_declared_peer_is_valid() {
    let repo = TestRepo::new();
    // Peer wiki at foo/wiki.toml with namespace = "foo"
    repo.create_file("foo/wiki.toml", "namespace = \"foo\"\n");
    repo.create_file("foo/page.md", &make_wiki_page("Foo Page", "In foo."));
    // Current wiki with peer "foo"
    repo.create_file("wiki/wiki.toml", "[peers]\nfoo = \"../foo\"\n");
    repo.create_file("wiki/page.md", &make_wiki_page("Page", "Hello."));
    // A *.wiki.md that belongs to the "foo" namespace
    repo.create_file(
        "src/feature.wiki.md",
        &make_wiki_page_with_namespace("Feature", "foo"),
    );
    repo.commit("init");

    let out = repo.run_check_from("wiki");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "expected exit 0 (namespace matches declared peer); stdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        !stdout.contains("namespace_undeclared"),
        "namespace matching declared peer must not produce namespace_undeclared; stdout: {stdout}"
    );
}

// ── Rule 6 tests ──────────────────────────────────────────────────────────────

/// `[[ghost:Article]]` where `ghost` is not declared as a peer → exit 1 with
/// `cross_namespace_wikilink_unresolved`.
#[test]
fn rule6_unknown_peer_namespace_in_wikilink_exits_1() {
    let repo = TestRepo::new();
    repo.create_file("wiki/wiki.toml", "");
    repo.create_file(
        "wiki/page.md",
        &make_wiki_page("Page", "See [[ghost:Article]]."),
    );
    repo.commit("init");

    let out = repo.run_check_from("wiki");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "expected exit 1; stdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("cross_namespace_wikilink_unresolved"),
        "expected cross_namespace_wikilink_unresolved; stdout: {stdout}"
    );
}

/// `[[foo:Missing]]` where peer `foo` exists but `Missing` is not in its
/// index → exit 1 with `cross_namespace_wikilink_unresolved`.
#[test]
fn rule6_peer_exists_but_article_missing_exits_1() {
    let repo = TestRepo::new();
    // Peer wiki at foo/
    repo.create_file("foo/wiki.toml", "namespace = \"foo\"\n");
    repo.create_file("foo/real.md", &make_wiki_page("Real Article", "Content."));
    // Current wiki linking to a non-existent article in foo
    repo.create_file("wiki/wiki.toml", "[peers]\nfoo = \"../foo\"\n");
    repo.create_file(
        "wiki/page.md",
        &make_wiki_page("Page", "See [[foo:Missing]]."),
    );
    repo.commit("init");

    let out = repo.run_check_from("wiki");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "expected exit 1; stdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("cross_namespace_wikilink_unresolved"),
        "expected cross_namespace_wikilink_unresolved; stdout: {stdout}"
    );
}

// ── F3: same-namespace qualified wikilink validation ─────────────────────────

/// `[[foo:Typo]]` where `foo` is the CURRENT wiki's namespace and `Typo` does
/// not exist → exit 1 with `broken_wikilink`.
#[test]
fn f3_same_ns_qualified_wikilink_missing_target_exits_1() {
    let repo = TestRepo::new();
    // Current wiki has namespace = "foo"
    repo.create_file("wiki/wiki.toml", "namespace = \"foo\"\n");
    repo.create_file("wiki/real.md", &make_wiki_page("Real Article", "Content."));
    // A page that uses [[foo:Typo]] where Typo does not exist
    repo.create_file(
        "wiki/page.md",
        &make_wiki_page("Page", "See [[foo:Typo]]."),
    );
    repo.commit("init");

    let out = repo.run_check_from("wiki");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "[[current_ns:Missing]] must produce an error; stdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("broken_wikilink"),
        "expected broken_wikilink diagnostic; stdout: {stdout}"
    );
}

/// `[[foo:Real Article]]` where `foo` is the CURRENT wiki's namespace and
/// `Real Article` exists → exit 0, no diagnostic.
#[test]
fn f3_same_ns_qualified_wikilink_existing_target_is_valid() {
    let repo = TestRepo::new();
    repo.create_file("wiki/wiki.toml", "namespace = \"foo\"\n");
    repo.create_file("wiki/real.md", &make_wiki_page("Real Article", "Content."));
    repo.create_file(
        "wiki/page.md",
        &make_wiki_page("Page", "See [[foo:Real Article]]."),
    );
    repo.commit("init");

    let out = repo.run_check_from("wiki");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "[[current_ns:Existing]] must be valid; stdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        !stdout.contains("broken_wikilink"),
        "valid same-ns qualified link must not produce broken_wikilink; stdout: {stdout}"
    );
}

// ── F7: namespace_config_invalid under -n '*' ─────────────────────────────────

/// `wiki check -n '*'` where one peer namespace has a valid wiki.toml (so the
/// root `WikiConfig::load` succeeds) but that peer's own wiki.toml declares a
/// sub-peer with a missing `wiki.toml` (so `run_multi`'s per-namespace
/// `WikiConfig::load` fails).
///
/// F7: previously `.ok()` silently swallowed the per-namespace load error,
/// producing a green block even though rules 5/6 never ran for that namespace.
/// After the fix, a `namespace_config_invalid` diagnostic must appear.
#[test]
fn f7_broken_sub_peer_emits_namespace_config_invalid() {
    let repo = TestRepo::new();

    // "bad" wiki: syntactically valid (so root load parses it), but declares
    // a sub-peer "ghost" whose wiki.toml does not exist.
    repo.create_file(
        "bad/wiki.toml",
        "namespace = \"bad\"\n[peers]\nghost = \"../ghost\"\n",
    );
    repo.create_file("bad/page.md", &make_wiki_page("Bad Page", "Content."));

    // Root wiki lists "bad" as a peer — root load succeeds because bad/wiki.toml
    // is syntactically valid (root load only validates peer existence, not their
    // own sub-peers).
    repo.create_file(
        "wiki/wiki.toml",
        "[peers]\nbad = \"../bad\"\n",
    );
    repo.create_file("wiki/page.md", &make_wiki_page("Root Page", "Hello."));
    repo.commit("init");

    // Run with -n '*' from the wiki dir
    let cwd = repo.dir.path().join("wiki");
    let out = Command::new(env!("CARGO_BIN_EXE_wiki"))
        .current_dir(&cwd)
        .args(["check", "-n", "*"])
        .env("WIKI_BACKGROUND_FTS", "0")
        .output()
        .expect("run wiki check -n '*'");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        !out.status.success(),
        "broken sub-peer must cause non-zero exit; stdout: {stdout}\nstderr: {stderr}"
    );
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("namespace_config_invalid"),
        "expected namespace_config_invalid in combined output; stdout: {stdout}\nstderr: {stderr}"
    );
}

/// `[[foo:Real Article]]` where peer `foo` exists and `Real Article` is in its
/// wiki → exit 0, no diagnostic.
#[test]
fn rule6_peer_exists_and_article_exists_is_valid() {
    let repo = TestRepo::new();
    // Peer wiki at foo/
    repo.create_file("foo/wiki.toml", "namespace = \"foo\"\n");
    repo.create_file("foo/real.md", &make_wiki_page("Real Article", "Content."));
    // Current wiki linking correctly into foo
    repo.create_file("wiki/wiki.toml", "[peers]\nfoo = \"../foo\"\n");
    repo.create_file(
        "wiki/page.md",
        &make_wiki_page("Page", "See [[foo:Real Article]]."),
    );
    repo.commit("init");

    let out = repo.run_check_from("wiki");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "expected exit 0 for valid cross-namespace wikilink; stdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        !stdout.contains("cross_namespace_wikilink_unresolved"),
        "valid cross-namespace link must not produce diagnostic; stdout: {stdout}"
    );
}
