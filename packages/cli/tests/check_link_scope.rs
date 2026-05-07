//! Integration tests for `wiki check` link-validation scope.
//!
//! Documented model (mirrors `multi_namespace_integration::f1`/`f2`):
//!   - A `*.wiki.md` file with `namespace: foo` belongs to peer `foo` only.
//!   - A `*.wiki.md` file with no `namespace:` frontmatter belongs to the
//!     default namespace only.
//!   - A regular page (`.md`) inside a wiki root belongs to that wiki.
//!
//! `Markdown Link To Wiki` and `Broken Wikilink` diagnostics must therefore
//! be evaluated only in the file's owning namespace. Today's implementation
//! evaluates `*.wiki.md` files under every configured namespace, producing
//! contradictory errors that block automated remediation.
//!
//! Tests marked "demonstrates current bug" assert the *intended* behavior
//! and are expected to fail until routing is fixed.

use std::fs;
use std::process::{Command, Output};

use tempfile::TempDir;

// ── TestRepo (mirrors check_namespace_rules.rs) ──────────────────────────────

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

    /// Run `wiki check --no-mesh --format json` from `cwd_rel`.
    fn check_json(&self, cwd_rel: &str) -> Output {
        self.check_json_with(cwd_rel, &[])
    }

    /// Run `wiki check ... --format json` with extra args (e.g., `["-n", "foo"]`
    /// before the subcommand, or a path glob after it).
    fn check_json_with(&self, cwd_rel: &str, extra: &[&str]) -> Output {
        let cwd = if cwd_rel.is_empty() {
            self.dir.path().to_path_buf()
        } else {
            self.dir.path().join(cwd_rel)
        };
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_wiki"));
        cmd.current_dir(&cwd).env("WIKI_BACKGROUND_FTS", "0");
        // -n flag, if any, must precede the subcommand.
        let mut idx = 0;
        while idx + 1 < extra.len() && extra[idx] == "-n" {
            cmd.args([extra[idx], extra[idx + 1]]);
            idx += 2;
        }
        cmd.args(["check", "--no-mesh", "--format", "json"]);
        for a in &extra[idx..] {
            cmd.arg(a);
        }
        cmd.output().expect("run wiki check")
    }
}

fn page(title: &str, body: &str) -> String {
    format!("---\ntitle: {title}\nsummary: A page about {title}.\n---\n{body}")
}

fn float(title: &str, namespace: Option<&str>, body: &str) -> String {
    match namespace {
        Some(ns) => format!(
            "---\ntitle: {title}\nsummary: A float about {title}.\nnamespace: {ns}\n---\n{body}"
        ),
        None => format!("---\ntitle: {title}\nsummary: A float about {title}.\n---\n{body}"),
    }
}

/// Count diagnostics in JSON output matching kind + namespace (any if None).
///
/// Tolerates the alternate `{"error": "..."}` envelope the CLI emits when a
/// per-namespace runtime failure short-circuits the run (returns 0 in that
/// case so callers can assert separately on the runtime failure).
fn count_errors(stdout: &str, kind: &str, namespace: Option<&str>) -> usize {
    let v: serde_json::Value = match serde_json::from_str(stdout) {
        Ok(v) => v,
        Err(_) if stdout.trim().is_empty() => return 0,
        Err(e) => panic!("parse json failed: {e}\nstdout: {stdout}"),
    };
    let arr = v
        .get("errors")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default();
    arr.iter()
        .filter(|e| e.get("kind").and_then(|x| x.as_str()) == Some(kind))
        .filter(|e| match namespace {
            Some(n) => e.get("namespace").and_then(|x| x.as_str()) == Some(n),
            None => true,
        })
        .count()
}

/// Build the standard two-namespace fixture: default wiki at `wiki/` with peer
/// `foo` at `foo/`. Each has a single page named in args.
fn standard_fixture(repo: &TestRepo, default_page: Option<(&str, &str)>, foo_page: Option<(&str, &str)>) {
    repo.create_file("wiki/wiki.toml", "[peers]\nfoo = \"../foo\"\n");
    repo.create_file("foo/wiki.toml", "namespace = \"foo\"\n");
    if let Some((title, body)) = default_page {
        repo.create_file("wiki/default-page.md", &page(title, body));
    } else {
        // Ensure default has at least one page so it's a valid wiki.
        repo.create_file("wiki/index.md", &page("Default Index", "Hello."));
    }
    if let Some((title, body)) = foo_page {
        repo.create_file("foo/foo-page.md", &page(title, body));
    } else {
        repo.create_file("foo/index.md", &page("Foo Index", "Hello."));
    }
}

// ── Group L: float link checks honor owning namespace ────────────────────────

/// L1 (✅ expected to pass today): float `namespace: foo` with `[[Target]]`
/// resolving in `foo` — clean.
#[test]
fn l1_tagged_float_wikilink_resolving_in_owning_ns_is_clean() {
    let repo = TestRepo::new();
    standard_fixture(&repo, None, Some(("Target", "T.")));
    repo.create_file(
        "floats/notes.wiki.md",
        &float("Notes", Some("foo"), "See [[Target]].\n"),
    );
    repo.commit("init");

    let out = repo.check_json("wiki");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "expected clean check; stdout: {stdout}"
    );
    assert_eq!(count_errors(&stdout, "broken_wikilink", None), 0);
    assert_eq!(count_errors(&stdout, "markdown_link_to_wiki", None), 0);
}

/// L2 (❌ demonstrates bug): float `namespace: foo` with a path-style link to
/// the foo target produces exactly one `[foo] Markdown Link To Wiki` —
/// **not** duplicated under `default`.
#[test]
fn l2_tagged_float_path_link_emits_single_diagnostic_in_owning_ns() {
    let repo = TestRepo::new();
    standard_fixture(&repo, None, Some(("Target", "T.")));
    repo.create_file(
        "floats/notes.wiki.md",
        &float("Notes", Some("foo"), "See [Target](../foo/foo-page.md).\n"),
    );
    repo.commit("init");

    let out = repo.check_json("wiki");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let in_foo = count_errors(&stdout, "markdown_link_to_wiki", Some("foo"));
    let in_default = count_errors(&stdout, "markdown_link_to_wiki", Some("default"));
    assert_eq!(in_foo, 1, "expected one diagnostic in foo; stdout: {stdout}");
    assert_eq!(
        in_default, 0,
        "owning namespace is foo; default must not duplicate the diagnostic; stdout: {stdout}"
    );
}

/// L3 (❌ demonstrates bug): untagged float with `[[Target]]` where the target
/// only exists in peer `foo` — exactly one `[default] Broken Wikilink`,
/// **not** reported under `foo`.
#[test]
fn l3_untagged_float_wikilink_reported_only_in_default() {
    let repo = TestRepo::new();
    standard_fixture(&repo, None, Some(("Target", "T.")));
    repo.create_file(
        "floats/notes.wiki.md",
        &float("Notes", None, "See [[Target]].\n"),
    );
    repo.commit("init");

    let out = repo.check_json("wiki");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let in_default = count_errors(&stdout, "broken_wikilink", Some("default"));
    let in_foo = count_errors(&stdout, "broken_wikilink", Some("foo"));
    assert_eq!(
        in_default, 1,
        "untagged float belongs to default; expected one [default] broken_wikilink; stdout: {stdout}"
    );
    assert_eq!(
        in_foo, 0,
        "untagged float must not be evaluated under foo; stdout: {stdout}"
    );
}

/// L4 (❌ demonstrates bug): untagged float with path link to a foo target —
/// `[default] Markdown Link To Wiki`, **not** reported under `foo`.
#[test]
fn l4_untagged_float_path_link_reported_only_in_default() {
    let repo = TestRepo::new();
    standard_fixture(&repo, None, Some(("Target", "T.")));
    repo.create_file(
        "floats/notes.wiki.md",
        &float("Notes", None, "See [Target](../foo/foo-page.md).\n"),
    );
    repo.commit("init");

    let out = repo.check_json("wiki");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let in_default = count_errors(&stdout, "markdown_link_to_wiki", Some("default"));
    let in_foo = count_errors(&stdout, "markdown_link_to_wiki", Some("foo"));
    assert_eq!(in_default, 1, "expected one [default] diagnostic; stdout: {stdout}");
    assert_eq!(
        in_foo, 0,
        "untagged float must not be evaluated under foo; stdout: {stdout}"
    );
}

/// L5 (❌ demonstrates bug): float `namespace: foo` with `[[Target]]` where
/// `Target` only exists in default → `[foo] Broken Wikilink`, **not** in default.
#[test]
fn l5_tagged_float_wikilink_unresolved_in_owning_ns_only() {
    let repo = TestRepo::new();
    standard_fixture(&repo, Some(("Target", "T.")), None);
    repo.create_file(
        "floats/notes.wiki.md",
        &float("Notes", Some("foo"), "See [[Target]].\n"),
    );
    repo.commit("init");

    let out = repo.check_json("wiki");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let in_foo = count_errors(&stdout, "broken_wikilink", Some("foo"));
    let in_default = count_errors(&stdout, "broken_wikilink", Some("default"));
    assert_eq!(in_foo, 1, "expected one [foo] broken_wikilink; stdout: {stdout}");
    assert_eq!(
        in_default, 0,
        "owning namespace is foo; default must not be evaluated; stdout: {stdout}"
    );
}

// ── Group S: suggested fix is actually fixable (round-trip) ──────────────────

/// S1 (❌ demonstrates bug): apply the suggestion from `Markdown Link To Wiki`
/// on a tagged float and re-run check; expect it to be clean.
#[test]
fn s1_tagged_float_suggestion_round_trips_to_clean() {
    let repo = TestRepo::new();
    standard_fixture(&repo, None, Some(("Target", "T.")));
    let path = "floats/notes.wiki.md";
    repo.create_file(
        path,
        &float("Notes", Some("foo"), "See [Target](../foo/foo-page.md).\n"),
    );
    repo.commit("init");

    // Initial: at least one Markdown Link To Wiki diagnostic.
    let out = repo.check_json("wiki");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        count_errors(&stdout, "markdown_link_to_wiki", None) >= 1,
        "expected initial diagnostic; stdout: {stdout}"
    );

    // Apply the suggested fix verbatim.
    repo.create_file(path, &float("Notes", Some("foo"), "See [[Target]].\n"));
    repo.commit("apply suggestion");

    let out = repo.check_json("wiki");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "applying the CLI's own suggestion must yield a clean check; stdout: {stdout}"
    );
    assert_eq!(count_errors(&stdout, "broken_wikilink", None), 0);
    assert_eq!(count_errors(&stdout, "markdown_link_to_wiki", None), 0);
}

// ── Group N: namespaced source files (regular pages inside wiki roots) ──────

/// N4 (✅ expected to pass today): a page inside `foo/` with `[[Other]]`
/// where `Other` only exists in default → `[foo] Broken Wikilink`. Pins that
/// wikilinks do not silently cross namespaces from a namespaced source.
#[test]
fn n4_namespaced_source_wikilink_does_not_cross_namespaces() {
    let repo = TestRepo::new();
    standard_fixture(&repo, Some(("Other", "O.")), None);
    repo.create_file("foo/source.md", &page("Foo Source", "See [[Other]].\n"));
    repo.commit("init");

    let out = repo.check_json("wiki");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!out.status.success(), "expected error; stdout: {stdout}");
    assert_eq!(count_errors(&stdout, "broken_wikilink", Some("foo")), 1);
    assert_eq!(count_errors(&stdout, "broken_wikilink", Some("default")), 0);
}

// ── Group R: routing precedence and failure modes ───────────────────────────

// ── Group K: every file-level diagnostic kind honors the owning namespace ───
//
// Today, an untagged `*.wiki.md` float (owner = default) is evaluated under
// every namespace, so each diagnostic kind below is emitted twice — once in
// `default` (correct) and once in `foo` (spurious). Each test asserts the
// intended single-owner behavior and is expected to fail until fan-out is
// fixed.

/// K1 (❌): `frontmatter` diagnostic for an untagged float must be reported
/// only under `default`.
#[test]
fn k1_frontmatter_diagnostic_reported_only_in_owning_ns() {
    let repo = TestRepo::new();
    standard_fixture(&repo, None, None);
    // Missing `summary` field — triggers a frontmatter diagnostic.
    repo.create_file(
        "floats/notes.wiki.md",
        "---\ntitle: Notes\n---\nBody.\n",
    );
    repo.commit("init");

    let out = repo.check_json("wiki");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        count_errors(&stdout, "frontmatter", Some("default")),
        1,
        "expected one [default] frontmatter diagnostic; stdout: {stdout}"
    );
    assert_eq!(
        count_errors(&stdout, "frontmatter", Some("foo")),
        0,
        "untagged float must not be evaluated under foo; stdout: {stdout}"
    );
}

/// K2 (❌): `missing_file` (fragment link to a non-existent path) must be
/// reported only under the float's owning namespace.
#[test]
fn k2_missing_file_diagnostic_reported_only_in_owning_ns() {
    let repo = TestRepo::new();
    standard_fixture(&repo, None, None);
    repo.create_file(
        "floats/notes.wiki.md",
        &float("Notes", None, "See [bad](nonexistent.txt#L1-L2).\n"),
    );
    repo.commit("init");

    let out = repo.check_json("wiki");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(count_errors(&stdout, "missing_file", Some("default")), 1);
    assert_eq!(count_errors(&stdout, "missing_file", Some("foo")), 0);
}

/// K3 (❌): `line_range` (fragment link out of bounds) must be reported only
/// under the float's owning namespace.
#[test]
fn k3_line_range_diagnostic_reported_only_in_owning_ns() {
    let repo = TestRepo::new();
    standard_fixture(&repo, None, None);
    repo.create_file("floats/source.txt", "one line\n");
    repo.create_file(
        "floats/notes.wiki.md",
        &float("Notes", None, "See [bad](source.txt#L100-L200).\n"),
    );
    repo.commit("init");

    let out = repo.check_json("wiki");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(count_errors(&stdout, "line_range", Some("default")), 1);
    assert_eq!(count_errors(&stdout, "line_range", Some("foo")), 0);
}

/// K4 (❌): `missing_heading` (wikilink target found but heading anchor
/// missing). Today this kind also flips to `broken_wikilink` in the wrong-ns
/// scope because the target only exists in the other namespace's index — the
/// owning-ns rule cleans this up by evaluating once.
#[test]
fn k4_missing_heading_diagnostic_reported_only_in_owning_ns() {
    let repo = TestRepo::new();
    standard_fixture(&repo, Some(("Target", "# Real Heading\nBody.\n")), None);
    repo.create_file(
        "floats/notes.wiki.md",
        &float("Notes", None, "See [[Target#No Such Heading]].\n"),
    );
    repo.commit("init");

    let out = repo.check_json("wiki");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        count_errors(&stdout, "missing_heading", Some("default")),
        1,
        "expected one [default] missing_heading; stdout: {stdout}"
    );
    assert_eq!(count_errors(&stdout, "missing_heading", Some("foo")), 0);
    assert_eq!(
        count_errors(&stdout, "broken_wikilink", Some("foo")),
        0,
        "owning ns is default; foo must not be evaluated; stdout: {stdout}"
    );
}

/// K5 (❌): `collision` (title or alias clashes with an existing page) must
/// be evaluated against the owning namespace's index only. Untagged float
/// with a title that exists in `foo` should NOT collide with the foo index.
#[test]
fn k5_collision_diagnostic_evaluated_against_owning_ns_index() {
    let repo = TestRepo::new();
    standard_fixture(&repo, None, None);
    // foo/index.md uses title "Foo Index". Untagged float reuses that title.
    repo.create_file(
        "floats/notes.wiki.md",
        "---\ntitle: Foo Index\nsummary: S.\n---\nBody.\n",
    );
    repo.commit("init");

    let out = repo.check_json("wiki");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        count_errors(&stdout, "collision", Some("foo")),
        0,
        "untagged float must not collide with foo's index; stdout: {stdout}"
    );
    // It SHOULD be a clean run (no collision under default either since
    // default has no `Foo Index`).
    assert_eq!(count_errors(&stdout, "collision", Some("default")), 0);
}

/// K6 (❌): `alias_resolve` (warning that a wikilink resolved via alias) for a
/// tagged float must be evaluated against the owning namespace's aliases only.
/// A float `namespace: foo` linking `[[Alpha]]` where `Alpha` is an alias only
/// in `default` should produce `[foo] broken_wikilink` (alias doesn't exist
/// in foo) — and not `[default] alias_resolve`.
#[test]
fn k6_alias_resolve_diagnostic_evaluated_against_owning_ns_only() {
    let repo = TestRepo::new();
    standard_fixture(&repo, None, None);
    repo.create_file(
        "wiki/aliased.md",
        "---\ntitle: Aliased\naliases: [Alpha]\nsummary: A.\n---\nBody.\n",
    );
    repo.create_file(
        "floats/notes.wiki.md",
        &float("Notes", Some("foo"), "See [[Alpha]].\n"),
    );
    repo.commit("init");

    let out = repo.check_json("wiki");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        count_errors(&stdout, "alias_resolve", Some("default")),
        0,
        "owning ns is foo; default must not emit alias_resolve; stdout: {stdout}"
    );
    assert_eq!(
        count_errors(&stdout, "broken_wikilink", Some("foo")),
        1,
        "Alpha is not in foo's index; expected broken_wikilink under foo; stdout: {stdout}"
    );
}

/// R3 (❌ demonstrates bug): with no top-level `wiki.toml` (no default wiki),
/// only sibling peer roots, an untagged float outside every root is not a
/// member of any wiki and must produce a clear routing error — not be
/// silently evaluated under every peer.
///
/// This test asserts that no `broken_wikilink` is emitted under either peer
/// for an untagged float outside both roots. Today the float is checked
/// under every peer and produces `broken_wikilink` in each.
#[test]
fn r3_untagged_float_with_no_default_does_not_fan_out_diagnostics() {
    let repo = TestRepo::new();
    repo.create_file("nsA/wiki.toml", "namespace = \"nsA\"\n");
    repo.create_file("nsB/wiki.toml", "namespace = \"nsB\"\n");
    repo.create_file("nsA/target.md", &page("Target", "T."));
    repo.create_file("nsB/other.md", &page("Other", "O."));
    repo.create_file(
        "external/notes.wiki.md",
        &float("Notes", None, "See [[Target]].\n"),
    );
    repo.commit("init");

    let out = repo.check_json("nsA");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let in_nsa = count_errors(&stdout, "broken_wikilink", Some("nsA"));
    let in_nsb = count_errors(&stdout, "broken_wikilink", Some("nsB"));
    assert_eq!(
        in_nsa + in_nsb,
        0,
        "untagged float outside every root must not produce broken_wikilink \
         in unrelated namespaces; stdout: {stdout}"
    );
}

// ── Group G: alternate invocation surfaces (path filter, -n, location) ──────

/// G1 (❌): explicit path-filter invocation. Routing must hold even when the
/// user passes the float's path directly to `wiki check`. Today the run fans
/// out across namespaces and short-circuits with a `{"error": "[foo] no
/// wiki pages found"}` envelope because `foo`'s glob expansion finds no
/// files matching the path filter. Owning-ns routing should send the float
/// to `default` only, producing one `[default] broken_wikilink`.
#[test]
fn g1_path_filter_invocation_routes_to_owning_ns_only() {
    let repo = TestRepo::new();
    standard_fixture(&repo, None, Some(("Target", "T.")));
    repo.create_file(
        "floats/notes.wiki.md",
        &float("Notes", None, "See [[Target]].\n"),
    );
    repo.commit("init");

    let out = repo.check_json_with("wiki", &["../floats/notes.wiki.md"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    // Parse and assert structurally on the envelope shape: success runs emit
    // `{"errors": [...]}`; per-namespace runtime failures emit a top-level
    // `{"error": "..."}` (string), which must not appear under owning-ns
    // routing.
    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("parse json: {e}\nstdout: {stdout}\nstderr: {stderr}"));
    assert!(
        v.get("error").is_none(),
        "path-filter invocation must not produce a per-namespace runtime \
         error envelope; got top-level `error`: {v}\nstderr: {stderr}"
    );
    let in_default = count_errors(&stdout, "broken_wikilink", Some("default"));
    let in_foo = count_errors(&stdout, "broken_wikilink", Some("foo"));
    assert_eq!(in_default, 1, "expected one [default] diagnostic; stdout: {stdout}");
    assert_eq!(
        in_foo, 0,
        "explicit path filter must not bypass owning-ns routing; stdout: {stdout}"
    );
}

/// G2 (❌): `wiki check -n foo` must skip floats not owned by `foo`.
#[test]
fn g2_namespace_scope_skips_unrelated_float() {
    let repo = TestRepo::new();
    standard_fixture(&repo, None, Some(("Target", "T.")));
    repo.create_file(
        "floats/notes.wiki.md",
        &float("Notes", None, "See [[Target]].\n"),
    );
    repo.commit("init");

    let out = repo.check_json_with("wiki", &["-n", "foo"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        count_errors(&stdout, "broken_wikilink", None),
        0,
        "untagged (default-owned) float must not be evaluated under -n foo; stdout: {stdout}"
    );
    assert!(
        out.status.success(),
        "expected exit 0 when scoped to a namespace that owns no failing files; stdout: {stdout}"
    );
}

/// G3 (❌): a `*.wiki.md` file physically nested under a peer root
/// (`foo/notes/x.wiki.md`) with NO `namespace:` frontmatter must be owned by
/// that peer (location wins when frontmatter is silent). It must NOT be
/// re-evaluated under `default`.
#[test]
fn g3_float_nested_under_peer_root_is_owned_by_that_peer() {
    let repo = TestRepo::new();
    standard_fixture(&repo, Some(("Default Target", "D.")), None);
    repo.create_file(
        "foo/notes/x.wiki.md",
        // Wikilink to a target that exists ONLY in default — under owning-ns
        // routing this must produce a [foo] broken_wikilink, not a [default] one.
        &float("Nested Notes", None, "See [[Default Target]].\n"),
    );
    repo.commit("init");

    let out = repo.check_json("wiki");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        count_errors(&stdout, "broken_wikilink", Some("foo")),
        1,
        "nested *.wiki.md must be owned by enclosing peer; stdout: {stdout}"
    );
    assert_eq!(
        count_errors(&stdout, "broken_wikilink", Some("default")),
        0,
        "default must not re-evaluate a peer-owned float; stdout: {stdout}"
    );
}

/// G4 (✅ pin): an untagged float with cross-namespace wikilink syntax
/// `[[foo:Target]]` resolving in the declared peer must be clean.
#[test]
fn g4_cross_namespace_wikilink_from_float_resolves() {
    let repo = TestRepo::new();
    standard_fixture(&repo, None, Some(("Target", "T.")));
    repo.create_file(
        "floats/notes.wiki.md",
        &float("Notes", None, "See [[foo:Target]].\n"),
    );
    repo.commit("init");

    let out = repo.check_json("wiki");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "expected clean check for valid cross-ns wikilink; stdout: {stdout}"
    );
    assert_eq!(count_errors(&stdout, "broken_wikilink", None), 0);
    assert_eq!(
        count_errors(&stdout, "cross_namespace_wikilink_unresolved", None),
        0
    );
}

// ── Group P: scaling (3+ namespaces) ────────────────────────────────────────

/// P1 (❌): with default + foo + bar peers, an untagged float linking to a
/// foo-only target must produce exactly one `[default] broken_wikilink` —
/// not one per peer.
#[test]
fn p1_three_peer_topology_does_not_multiply_diagnostics() {
    let repo = TestRepo::new();
    repo.create_file(
        "wiki/wiki.toml",
        "[peers]\nfoo = \"../foo\"\nbar = \"../bar\"\n",
    );
    repo.create_file("wiki/index.md", &page("Default Index", "D."));
    repo.create_file("foo/wiki.toml", "namespace = \"foo\"\n");
    repo.create_file("foo/target.md", &page("Target", "T."));
    repo.create_file("bar/wiki.toml", "namespace = \"bar\"\n");
    repo.create_file("bar/index.md", &page("Bar Index", "B."));
    repo.create_file(
        "floats/notes.wiki.md",
        &float("Notes", None, "See [[Target]].\n"),
    );
    repo.commit("init");

    let out = repo.check_json("wiki");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        count_errors(&stdout, "broken_wikilink", Some("default")),
        1,
        "expected one [default] diagnostic; stdout: {stdout}"
    );
    assert_eq!(count_errors(&stdout, "broken_wikilink", Some("foo")), 0);
    assert_eq!(count_errors(&stdout, "broken_wikilink", Some("bar")), 0);
}

// ── Group M: malformed routing inputs ───────────────────────────────────────

/// M1 (❌ demonstrates bug, dual purpose): `namespace: ""` is silently
/// accepted by the parser (today's behavior — pin it via the
/// no-`namespace_undeclared` assertion). Combined with owning-ns routing,
/// the float's link to a default-only page must resolve cleanly. Today the
/// fan-out evaluates the float under `foo` and emits a spurious
/// `[foo] broken_wikilink` for `[[Default Index]]`.
#[test]
fn m1_empty_string_namespace_is_treated_as_untagged() {
    let repo = TestRepo::new();
    standard_fixture(&repo, None, None);
    repo.create_file(
        "floats/notes.wiki.md",
        // Body links to default-owned page so we can confirm routing to default.
        "---\ntitle: Notes\nsummary: S.\nnamespace: \"\"\n---\nSee [[Default Index]].\n",
    );
    repo.commit("init");

    let out = repo.check_json("wiki");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "namespace=\"\" must currently be accepted as untagged; stdout: {stdout}"
    );
    assert_eq!(
        count_errors(&stdout, "namespace_undeclared", None),
        0,
        "empty-string namespace is not undeclared; stdout: {stdout}"
    );
    // With owning-ns routing, the float (untagged → default) links to a
    // default-only page and must be clean. Today the fan-out evaluates the
    // float under foo and emits a spurious [foo] broken_wikilink.
    assert_eq!(
        count_errors(&stdout, "broken_wikilink", None),
        0,
        "no broken_wikilink expected; stdout: {stdout}"
    );
}

// ── Group CC: positive collision check within owning namespace ──────────────

/// CC1 (❌): a tagged float `namespace: foo` whose title clashes with a page
/// already in `foo` must produce a `[foo] collision`. (Exercises the positive
/// side of the K5 negative — owning-ns evaluation must still fire when the
/// collision is real.)
#[test]
fn cc1_tagged_float_title_collides_with_owning_peer_index() {
    let repo = TestRepo::new();
    standard_fixture(&repo, None, None);
    // foo/index.md is titled "Foo Index"; tagged float reuses that title.
    repo.create_file(
        "floats/notes.wiki.md",
        &float("Foo Index", Some("foo"), "Body.\n"),
    );
    repo.commit("init");

    let out = repo.check_json("wiki");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        count_errors(&stdout, "collision", Some("foo")),
        1,
        "expected one [foo] collision for tagged float clashing with foo's index; stdout: {stdout}"
    );
    assert_eq!(
        count_errors(&stdout, "collision", Some("default")),
        0,
        "default must not see this collision; stdout: {stdout}"
    );
}
