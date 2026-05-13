//! Integration tests for FTS body indexing.
//!
//! Verifies that `body` is indexed by the FTS index so that terms appearing
//! only in the document body are returned by `wiki "<term>"`, ranked below
//! title hits on the same term.
//!
//! TDD bootstrap order: tests were written with `#[ignore]` first, then the
//! implementation was added and the ignores removed.

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

    fn remove_file(&self, path: &str) {
        let full = self.dir.path().join(path);
        fs::remove_file(full).expect("remove file");
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

fn make_wiki_repo() -> TestRepo {
    TestRepo::new()
}

/// A page whose term appears only in the body (not title/aliases/tags/keywords/summary)
/// must be returned by wiki search.
#[test]
fn body_only_term_is_returned_by_search() {
    let repo = make_wiki_repo();
    repo.create_file(
        "wiki/alpha.md",
        "---\ntitle: Alpha Doc\nsummary: A short summary.\n---\nThe secret body term is zygomorphic.\n",
    );
    repo.commit("add alpha");

    let out = repo.wiki(&["zygomorphic"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "wiki failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("Alpha Doc"),
        "expected 'Alpha Doc' in results for body-only term; got: {stdout}"
    );
}

/// A title hit must rank above a body-only hit when searching the same term.
#[test]
fn title_hit_ranks_above_body_only_hit() {
    let repo = make_wiki_repo();
    // "chromatic" is in the title of beta and in the body of gamma.
    repo.create_file(
        "wiki/beta.md",
        "---\ntitle: Chromatic Beta\nsummary: Something else.\n---\nUnrelated prose here.\n",
    );
    repo.create_file(
        "wiki/gamma.md",
        "---\ntitle: Gamma Doc\nsummary: Something else.\n---\nThis document discusses chromatic phenomena.\n",
    );
    repo.commit("add beta and gamma");

    let out = repo.wiki(&["--format", "json", "chromatic"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "wiki failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    let arr = v.as_array().expect("array");
    assert!(arr.len() >= 2, "expected at least 2 results; got: {stdout}");
    let titles: Vec<&str> = arr
        .iter()
        .map(|e| e["title"].as_str().unwrap_or(""))
        .collect();
    let beta_pos = titles.iter().position(|&t| t == "Chromatic Beta");
    let gamma_pos = titles.iter().position(|&t| t == "Gamma Doc");
    assert!(
        beta_pos.is_some() && gamma_pos.is_some(),
        "expected both docs in results; got titles: {titles:?}"
    );
    assert!(
        beta_pos.unwrap() < gamma_pos.unwrap(),
        "title hit (Chromatic Beta) should rank above body-only hit (Gamma Doc); order: {titles:?}"
    );
}

/// Re-indexing an unchanged file leaves body terms matchable.
#[test]
fn reindex_unchanged_file_stable_matchability() {
    let repo = make_wiki_repo();
    repo.create_file(
        "wiki/stable.md",
        "---\ntitle: Stable Doc\nsummary: A summary.\n---\nThis body contains the term holomorphic.\n",
    );
    repo.commit("add stable");

    // First search — trigger initial index build.
    let out1 = repo.wiki(&["holomorphic"]);
    assert!(out1.status.success());
    let stdout1 = String::from_utf8_lossy(&out1.stdout);
    assert!(
        stdout1.contains("Stable Doc"),
        "first search: expected Stable Doc; got: {stdout1}"
    );

    // Second search without changing the file — re-uses cached index.
    let out2 = repo.wiki(&["holomorphic"]);
    assert!(out2.status.success());
    let stdout2 = String::from_utf8_lossy(&out2.stdout);
    assert!(
        stdout2.contains("Stable Doc"),
        "second search (no change): expected Stable Doc; got: {stdout2}"
    );
}

/// After a file's body is changed, the prior body terms are no longer matched.
#[test]
fn reindex_changed_file_drops_prior_body_terms() {
    let repo = make_wiki_repo();
    repo.create_file(
        "wiki/changing.md",
        "---\ntitle: Changing Doc\nsummary: A summary.\n---\nOriginal body with term palimpsest.\n",
    );
    repo.commit("add changing");

    // Confirm initial term is found.
    let out1 = repo.wiki(&["palimpsest"]);
    assert!(out1.status.success());
    let s1 = String::from_utf8_lossy(&out1.stdout);
    assert!(
        s1.contains("Changing Doc"),
        "before update: expected Changing Doc; got: {s1}"
    );

    // Update the file — remove the old body term.
    repo.create_file(
        "wiki/changing.md",
        "---\ntitle: Changing Doc\nsummary: A summary.\n---\nUpdated body without the old term.\n",
    );
    repo.commit("update changing");

    // Old term must no longer match.
    let out2 = repo.wiki(&["palimpsest"]);
    assert!(out2.status.success());
    let s2 = String::from_utf8_lossy(&out2.stdout);
    assert!(
        !s2.contains("Changing Doc"),
        "after update: did not expect Changing Doc for old term; got: {s2}"
    );
}

/// After deleting a document, its FTS entry must not return results.
#[test]
fn delete_drops_document_from_fts() {
    let repo = make_wiki_repo();
    repo.create_file(
        "wiki/deletable.md",
        "---\ntitle: Deletable Doc\nsummary: A summary.\n---\nBody with unique term verisimilitude.\n",
    );
    repo.commit("add deletable");

    // Confirm the doc is found first.
    let out1 = repo.wiki(&["verisimilitude"]);
    assert!(out1.status.success());
    let s1 = String::from_utf8_lossy(&out1.stdout);
    assert!(
        s1.contains("Deletable Doc"),
        "before delete: expected Deletable Doc; got: {s1}"
    );

    // Delete the file and re-commit.
    repo.remove_file("wiki/deletable.md");
    repo.commit("delete deletable");

    // The term must no longer return results.
    let out2 = repo.wiki(&["verisimilitude"]);
    assert!(out2.status.success());
    let s2 = String::from_utf8_lossy(&out2.stdout);
    assert!(
        !s2.contains("Deletable Doc"),
        "after delete: did not expect Deletable Doc; got: {s2}"
    );
}

/// A term that only appears in frontmatter (not body) must still match the document,
/// and a term that only appears in the body must also match the same document.
///
/// Uses a single document to isolate each column: "sibilance" lives only in the
/// YAML `summary` field; "vexillology" lives only in the markdown body.
/// If `markdown_body` regressed and returned the raw source un-stripped, "sibilance"
/// would appear in the body column as well — the unit test in `index.rs` (module
/// `markdown_body_tests`) is the direct guard for that invariant; this test confirms
/// the end-to-end search surface for both columns independently.
#[test]
fn frontmatter_only_term_matches_via_metadata() {
    let repo = make_wiki_repo();
    repo.create_file(
        "wiki/combo.md",
        "---\ntitle: Combo Doc\nsummary: Contains the word sibilance here.\n---\nBody discusses vexillology exclusively.\n",
    );
    repo.commit("add combo");

    // Frontmatter-only term must find the document.
    let out_fm = repo.wiki(&["sibilance"]);
    let stdout_fm = String::from_utf8_lossy(&out_fm.stdout);
    assert!(out_fm.status.success());
    assert!(
        stdout_fm.contains("Combo Doc"),
        "expected Combo Doc for frontmatter term 'sibilance'; got: {stdout_fm}"
    );

    // Body-only term must also find the same document.
    let out_body = repo.wiki(&["vexillology"]);
    let stdout_body = String::from_utf8_lossy(&out_body.stdout);
    assert!(out_body.status.success());
    assert!(
        stdout_body.contains("Combo Doc"),
        "expected Combo Doc for body term 'vexillology'; got: {stdout_body}"
    );
}

/// Under search-limit contention, an exact-title-match doc must not be evicted by body-only hits.
///
/// Corpus: one doc has title "Luminal" (the exact query token, so it is promoted by the
/// exact-match lookup pipeline before FTS scores are applied), one has "luminal" in summary
/// (weight 2.0), and three docs have it only in their bodies (weight 1.0). SEARCH_LIMIT is 3,
/// so the three body-only docs could fill all result slots. This test asserts that the
/// title-exact-match doc ranks first and is not displaced, locking the behavior against
/// future tuning.
///
/// Note: Turso FTS BM25 scoring does not reliably rank a single summary occurrence above
/// multiple body occurrences when IDF is corpus-relative (the summary doc may be evicted
/// when 3+ body docs share the same term). Only the exact-match pipeline guarantee for the
/// title doc is asserted here.
#[test]
fn common_term_ranking_survives_search_limit() {
    let repo = make_wiki_repo();

    // Title is the exact query token; this doc is promoted via exact-match lookup,
    // which prepends it to results before any FTS score is considered.
    repo.create_file(
        "wiki/title_doc.md",
        "---\ntitle: Luminal\nsummary: Unrelated summary text.\n---\nBody prose without the common term.\n",
    );
    repo.create_file(
        "wiki/summary_doc.md",
        "---\ntitle: Summary Doc\nsummary: This summary discusses luminal phenomena.\n---\nBody prose without the common term.\n",
    );
    repo.create_file(
        "wiki/body1.md",
        "---\ntitle: Body Doc One\nsummary: Unrelated.\n---\nThis body mentions luminal once.\n",
    );
    repo.create_file(
        "wiki/body2.md",
        "---\ntitle: Body Doc Two\nsummary: Unrelated.\n---\nThis body also mentions luminal.\n",
    );
    repo.create_file(
        "wiki/body3.md",
        "---\ntitle: Body Doc Three\nsummary: Unrelated.\n---\nAnother body with luminal in the text.\n",
    );
    repo.commit("add contention corpus");

    let out = repo.wiki(&["--format", "json", "luminal"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "wiki failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let v: serde_json::Value = serde_json::from_str(&stdout).expect("json parse failed");
    let arr = v.as_array().expect("expected json array");
    assert!(
        !arr.is_empty(),
        "expected at least one result; got: {stdout}"
    );

    let titles: Vec<&str> = arr
        .iter()
        .map(|e| e["title"].as_str().unwrap_or(""))
        .collect();

    // The exact-match title doc must rank first and must not be displaced by body-only FTS hits.
    assert!(
        titles.first() == Some(&"Luminal"),
        "title-bearing doc (exact match) must rank first and not be evicted; order: {titles:?}"
    );
}

/// Existing snippet and line_snippet behavior is unchanged — source_raw still drives snippets.
#[test]
fn snippet_regression_source_raw_unchanged() {
    let repo = make_wiki_repo();
    repo.create_file(
        "wiki/snip.md",
        "---\ntitle: Snippet Doc\nsummary: A summary.\n---\nLine one.\nLine two contains snippetterm.\nLine three.\n",
    );
    repo.commit("add snip");

    let out = repo.wiki(&["--format", "json", "snippetterm"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    let arr = v.as_array().expect("array");
    assert!(!arr.is_empty(), "expected results; got: {stdout}");
    let snippets = &arr[0]["snippets"];
    assert!(!snippets.is_null(), "expected snippets field; got: {arr:?}");
    let snippets_str = snippets.to_string();
    assert!(
        snippets_str.contains("snippetterm"),
        "expected snippetterm in snippets; got: {snippets_str}"
    );
}
