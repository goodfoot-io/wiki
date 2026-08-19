//! Git-history-derived fragment-link drift engine.
//!
//! Replaces the git-mesh coverage system: every line-range fragment link on a
//! wiki page is classified against that page's **anchor epoch** — the newest
//! commit at which its `links-reviewed:` frontmatter field value changed (or
//! the current, uncommitted state when a bump is pending). No anchor file
//! exists anywhere; the fingerprint of a link's target range at its anchor
//! commit is computed on demand from git history.
//!
//! The engine is the sole authority for line-range links: the generic
//! broken-link passes stop reporting them, and fix mode routes every outcome
//! through the classification here (see `check.rs` / `check_fix.rs`).
//!
//! Classification order per link (card main-3 flowchart, plan Decisions 4–5):
//! epoch resolution → locator presence at the anchor commit → target missing
//! (move scan: 1 → `Moved`, ≥2 → `Unknown`, 0 → `Broken`) / extent no longer
//! fitting → `Broken` → range-equal fingerprint compare (`Healthy` / `Drift` /
//! `Moved` / `Unknown`) → range-different (content equal → `Healthy`,
//! different → `Uncertified`).

use std::path::Path;

use thiserror::Error;

use crate::index::DocSource;

// ── Public types ──────────────────────────────────────────────────────────────

/// Per-link classification of a line-range fragment link against its page's
/// anchor epoch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriftOutcome {
    /// Target content unchanged since the anchor commit.
    Healthy,
    /// The link's locator was not present at the anchor commit — new since
    /// the last review. Remedy: bump `links-reviewed:`.
    Uncertified,
    /// The target path is missing from the current tree (or the recorded
    /// extent no longer fits the target's current line count). Remedy: fix
    /// the href.
    Broken,
    /// The target exists but its content changed since the anchor commit.
    /// Remedy: bump `links-reviewed:`.
    Drift,
    /// The certified content was found at exactly one new location —
    /// `--fix` rewrites the href (path and range) to follow it.
    Moved {
        new_path: String,
        new_start: u32,
        new_end: u32,
    },
    /// Could not verify (ambiguous move — the certified content occurs at
    /// ≥2 candidate locations). Fail-closed; never auto-fixed.
    Unknown,
}

/// The resolved anchor epoch for one page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkEpoch {
    /// The current-side field value differs from the newest committed value
    /// (a pending bump, a field added but not yet committed, or a field
    /// removed but not yet committed): the anchor epoch **is** the current
    /// state. Certification-based outcomes (`Uncertified`, `Drift`) are
    /// suppressed and `--fix` does no certification work; structural
    /// failures (`Broken`) still flag. `value` is the current-side value —
    /// `None` when the field is absent (removed) in the current state.
    Current { value: Option<String> },
    /// The anchor is the newest commit at which the field value changed (or
    /// the oldest walked commit when no pair differs — field introduction).
    /// `path_at_commit` is the page's path at that commit, so the
    /// anchor-side blob is read under the name in effect there.
    Commit {
        sha: String,
        path_at_commit: String,
        value: String,
    },
    /// The field is absent at the current side and was never walked —
    /// the page has no anchor epoch. Read-only modes hard-error
    /// (`anchor_epoch_missing`); `--fix` initializes the field.
    Missing,
}

/// Page-level failure of the drift engine — always fail-closed.
#[derive(Debug, Error)]
pub enum EpochError {
    #[error("git history is shallow; anchor-commit lookup requires full history (fetch-depth: 0)")]
    ShallowClone,
    #[error("page `{page}` unreadable at commit {commit}")]
    UnreadableBlob { page: String, commit: String },
    #[error("classify_page requires a resolved anchor epoch (Current or Commit)")]
    MissingEpoch,
    #[error("git failed: {0}")]
    GitFailed(String),
}

/// One classified line-range link on a page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkClass {
    /// Resolved target path (repo-relative, no `#` fragment).
    pub target_path: String,
    /// First line of the referenced range.
    pub start_line: u32,
    /// Last line of the referenced range.
    pub end_line: u32,
    /// 1-based line number in the source wiki page.
    pub source_line: usize,
    /// Absolute byte offset in the page content where the href begins
    /// (the character after the opening `(`).
    pub href_byte_start: usize,
    /// Absolute byte offset in the page content where the href ends
    /// (the character before the closing `)`).
    pub href_byte_end: usize,
    /// The link text (the `[label]` part) — half of the locator identity.
    pub label: String,
    /// The original, unscrubbed href text.
    pub original_href: String,
    /// The classification.
    pub outcome: DriftOutcome,
}

// ── Engine ────────────────────────────────────────────────────────────────────

/// Extract the `links-reviewed:` frontmatter value from page content,
/// coerced to its string form. Returns `None` when the page has no wiki
/// frontmatter block or the field is absent. Change detection compares these
/// strings, so any later value change re-certifies the page.
pub fn extract_links_reviewed(content: &str) -> Option<String> {
    let _ = content;
    todo!("drift::extract_links_reviewed (Phase 1)")
}

/// Resolve the page's anchor epoch per plan Decision 2.
///
/// `current_value` is the field value at the current side (worktree fs,
/// `HEAD` blob, or index blob, per `--source`); `committed_value` is the
/// value at `HEAD` (the newest committed value). When the two differ the
/// anchor epoch IS the current state (pending-certification rule). When both
/// are `None` the page has no epoch (`Missing`). Only when both are `Some`
/// and equal does the engine walk full ancestry
/// (`git log --follow --name-status --format=%H -- <page>`, no commit cap, no
/// `--first-parent`) and anchor at the newer commit of the first adjacent
/// pair whose parsed values differ, or the oldest walked commit if no pair
/// differs. A shallow clone is detected via `git rev-parse
/// --is-shallow-repository` and fails closed with [`EpochError::ShallowClone`].
pub fn find_anchor_commit(
    repo_root: &Path,
    page_path: &str,
    current_value: Option<&str>,
    committed_value: Option<&str>,
) -> Result<LinkEpoch, EpochError> {
    let _ = (repo_root, page_path, current_value, committed_value);
    todo!("drift::find_anchor_commit (Phase 1)")
}

/// Classify every line-range fragment link on `page_content` against
/// `epoch` — the full per-link flowchart of plan Decision 5.
///
/// Reads target content at the current side through the source-aware reader
/// (`DocSource::WorkingTree` → fs, `Head` → `git show HEAD:path`, `Index` →
/// `git show :path`) and, for a [`LinkEpoch::Commit`] epoch, the anchor-side
/// page and target blobs via git history. Only links with an explicit line
/// range (`path#Lstart-Lend`) are classified; plain paths and heading-slug
/// fragments are outside this system's scope. The pending-bump override
/// ([`LinkEpoch::Current`]) suppresses certification outcomes but still flags
/// `Broken` structural failures.
pub fn classify_page(
    repo_root: &Path,
    source: DocSource,
    page_path: &str,
    page_content: &str,
    epoch: &LinkEpoch,
) -> Result<Vec<LinkClass>, EpochError> {
    let _ = (repo_root, source, page_path, page_content, epoch);
    todo!("drift::classify_page (Phase 1)")
}

/// The repo's first frontmatter writer: return `content` with
/// `links-reviewed: 1` appended as the last line of the existing YAML block,
/// just before the closing `---` fence, preserving the rest of the content
/// byte-for-byte. Returns `None` when the page has no wiki frontmatter block
/// (nothing to append into).
///
/// Pure: never writes to disk, and never rewrites an existing
/// `links-reviewed:` value — the caller only invokes it on pages the
/// classification proved field-less.
pub fn insert_links_reviewed(content: &str) -> Option<String> {
    let _ = content;
    todo!("drift::insert_links_reviewed (Phase 1)")
}

// ── Tests ─────────────────────────────────────────────────────────────────────
//
// Phase 0 P2 (tdd-bootstrap): acceptance checks against the stubs, all
// pending. P3 unskips them one concern at a time. Every repo-backed test uses
// a real temp repository with real git history — no mocks.

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use tempfile::TempDir;

    use super::*;

    const BLOCK: &str = "block-line-1\nblock-line-2\nblock-line-3";

    fn make_wiki_page(title: &str, body: &str, links_reviewed: Option<&str>) -> String {
        let field = links_reviewed
            .map(|v| format!("links-reviewed: {v}\n"))
            .unwrap_or_default();
        format!("---\ntitle: {title}\nsummary: A page about {title}.\n{field}---\n{body}")
    }

    struct TestRepo {
        dir: TempDir,
    }

    impl TestRepo {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            let repo = TestRepo { dir };
            repo.git(&["init", "-q"]);
            // A committed identity independent of the invoking environment.
            repo.git(&["config", "user.email", "test@example.com"]);
            repo.git(&["config", "user.name", "Test Author"]);
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
            fs::write(&full, content).expect("write file");
        }

        fn remove_file(&self, path: &str) {
            fs::remove_file(self.dir.path().join(path)).expect("remove file");
        }

        fn read(&self, path: &str) -> String {
            fs::read_to_string(self.dir.path().join(path)).expect("read file")
        }

        fn commit(&self, message: &str) -> String {
            self.git(&["add", "-A"]);
            self.git(&["commit", "-q", "-m", message]);
            self.git(&["rev-parse", "HEAD"])
        }

        fn git(&self, args: &[&str]) -> String {
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
                "git {args:?} failed:\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }

        /// A shallow clone of this repo in a new temp dir — the real thing,
        /// `git clone --depth 1`.
        fn shallow_clone(&self) -> TempDir {
            let dst = tempfile::tempdir().expect("tempdir for clone");
            let output = Command::new("git")
                .args(["clone", "-q", "--depth", "1"])
                .arg(self.dir.path())
                .arg(dst.path())
                .output()
                .expect("spawn git clone");
            assert!(
                output.status.success(),
                "git clone failed:\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
            dst
        }
    }

    /// The shared fixture for classify tests: a wiki page with a certified
    /// link `[b](target.md#L2-L4)` whose range covers `BLOCK`.
    fn repo_with_certified_link() -> (TestRepo, String) {
        let repo = TestRepo::new();
        repo.create_file(
            "wiki/target.md",
            "T0\nblock-line-1\nblock-line-2\nblock-line-3\nT1\n",
        );
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "[b](target.md#L2-L4)\n", Some("1")),
        );
        let c1 = repo.commit("certified page and target");
        (repo, c1)
    }

    fn classify(
        repo: &TestRepo,
        epoch: &LinkEpoch,
        page: &str,
    ) -> Result<Vec<LinkClass>, EpochError> {
        classify_page(
            repo.path(),
            DocSource::WorkingTree,
            page,
            &repo.read(page),
            epoch,
        )
    }

    fn field_value(content: &str) -> Option<String> {
        extract_links_reviewed(content)
    }

    // ── extract_links_reviewed ──

    #[test]
    #[ignore = "Phase 0 P3: implement extract_links_reviewed"]
    fn extracts_scalar_values_to_their_string_form() {
        assert_eq!(
            field_value(&make_wiki_page("P", "body\n", Some("1"))),
            Some("1".into())
        );
        assert_eq!(
            field_value(&make_wiki_page("P", "body\n", Some("v2"))),
            Some("v2".into())
        );
        assert_eq!(
            field_value(&make_wiki_page("P", "body\n", Some("\"quoted value\""))),
            Some("quoted value".into()),
            "YAML string scalars unquote"
        );
        assert_eq!(
            field_value(&make_wiki_page("P", "body\n", Some("2"))),
            Some("2".into()),
            "numeric scalars coerce to their string form"
        );
    }

    #[test]
    #[ignore = "Phase 0 P3: implement extract_links_reviewed"]
    fn extracts_none_when_field_absent_or_unparseable() {
        assert_eq!(field_value(&make_wiki_page("P", "body\n", None)), None);
        assert_eq!(field_value("no frontmatter at all\n"), None);
        // A field-looking line in the BODY is not frontmatter.
        let body = "links-reviewed: 5\n";
        assert_eq!(field_value(&make_wiki_page("P", body, None)), None);
    }

    // ── insert_links_reviewed ──

    #[test]
    #[ignore = "Phase 0 P3: implement insert_links_reviewed"]
    fn appends_field_before_closing_fence_preserving_body() {
        let content = make_wiki_page("P", "# Heading\n\nSome body text.\n", None);
        let with_field = insert_links_reviewed(&content).expect("has frontmatter");
        assert_eq!(
            with_field,
            "---\ntitle: P\nsummary: A page about P.\nlinks-reviewed: 1\n---\n# Heading\n\nSome body text.\n"
        );
        // The body survives byte-for-byte.
        assert!(with_field.ends_with("---\n# Heading\n\nSome body text.\n"));
        // And the result is idempotent in the sense the caller expects: a
        // page now carrying the field is refused, never rewritten.
        assert_eq!(insert_links_reviewed(&with_field), None);
    }

    #[test]
    #[ignore = "Phase 0 P3: implement insert_links_reviewed"]
    fn preserves_crlf_and_missing_trailing_newline() {
        let crlf = "---\r\ntitle: P\r\nsummary: S\r\n---\r\nbody\r\n";
        let with_field = insert_links_reviewed(crlf).expect("has frontmatter");
        assert_eq!(
            with_field,
            "---\r\ntitle: P\r\nsummary: S\r\nlinks-reviewed: 1\r\n---\r\nbody\r\n",
            "the inserted line matches the file's EOL"
        );

        let no_nl = "---\ntitle: P\nsummary: S\n---\nbody";
        let with_field = insert_links_reviewed(no_nl).expect("has frontmatter");
        assert_eq!(with_field, "---\ntitle: P\nsummary: S\nlinks-reviewed: 1\n---\nbody");
    }

    #[test]
    #[ignore = "Phase 0 P3: implement insert_links_reviewed"]
    fn preserves_multiline_and_quoted_neighbor_values() {
        let content = "---\ntitle: P\nsummary: |\n  multi\n  line\nkeywords: [\"a\", \"b\"]\n---\nbody\n";
        let with_field = insert_links_reviewed(content).expect("has frontmatter");
        assert_eq!(
            with_field,
            "---\ntitle: P\nsummary: |\n  multi\n  line\nkeywords: [\"a\", \"b\"]\nlinks-reviewed: 1\n---\nbody\n"
        );
    }

    #[test]
    #[ignore = "Phase 0 P3: implement insert_links_reviewed"]
    fn refuses_pages_without_wiki_frontmatter() {
        assert_eq!(insert_links_reviewed("just text\n"), None);
        assert_eq!(insert_links_reviewed("---\nname: skill\n---\nbody\n"), None);
    }

    // ── find_anchor_commit: pending-certification rule ──

    #[test]
    #[ignore = "Phase 0 P3: implement find_anchor_commit"]
    fn pending_bump_makes_current_state_the_epoch() {
        let repo = TestRepo::new();
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("P", "body\n", Some("1")),
        );
        repo.commit("field=1");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("P", "body\n", Some("2")),
        );
        let epoch = find_anchor_commit(repo.path(), "wiki/page.md", Some("2"), Some("1"))
            .expect("resolves");
        assert_eq!(epoch, LinkEpoch::Current { value: Some("2".into()) });
    }

    #[test]
    #[ignore = "Phase 0 P3: implement find_anchor_commit"]
    fn field_added_but_uncommitted_is_pending() {
        let repo = TestRepo::new();
        repo.create_file("wiki/page.md", &make_wiki_page("P", "body\n", None));
        repo.commit("no field");
        repo.create_file("wiki/page.md", &make_wiki_page("P", "body\n", Some("1")));
        let epoch = find_anchor_commit(repo.path(), "wiki/page.md", Some("1"), None)
            .expect("resolves");
        assert_eq!(epoch, LinkEpoch::Current { value: Some("1".into()) });
    }

    #[test]
    #[ignore = "Phase 0 P3: implement find_anchor_commit"]
    fn field_removed_but_uncommitted_is_pending_with_none_value() {
        let repo = TestRepo::new();
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("P", "body\n", Some("1")),
        );
        repo.commit("field=1");
        repo.create_file("wiki/page.md", &make_wiki_page("P", "body\n", None));
        let epoch = find_anchor_commit(repo.path(), "wiki/page.md", None, Some("1"))
            .expect("resolves");
        assert_eq!(epoch, LinkEpoch::Current { value: None });
    }

    #[test]
    #[ignore = "Phase 0 P3: implement find_anchor_commit"]
    fn field_absent_everywhere_is_missing() {
        let repo = TestRepo::new();
        repo.create_file("wiki/page.md", &make_wiki_page("P", "body\n", None));
        repo.commit("no field");
        let epoch = find_anchor_commit(repo.path(), "wiki/page.md", None, None)
            .expect("resolves");
        assert_eq!(epoch, LinkEpoch::Missing);
    }

    // ── find_anchor_commit: the history walk ──

    #[test]
    #[ignore = "Phase 0 P3: implement find_anchor_commit"]
    fn anchors_at_the_newest_value_changing_commit() {
        let repo = TestRepo::new();
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("P", "body\n", Some("1")),
        );
        repo.commit("field=1");
        repo.create_file("wiki/page.md", &make_wiki_page("P", "edited\n", Some("1")));
        repo.commit("body edit");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("P", "edited\n", Some("2")),
        );
        let bump_sha = repo.commit("bump to 2");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("P", "edited again\n", Some("2")),
        );
        repo.commit("body edit after bump");

        let epoch = find_anchor_commit(repo.path(), "wiki/page.md", Some("2"), Some("2"))
            .expect("resolves");
        assert_eq!(
            epoch,
            LinkEpoch::Commit {
                sha: bump_sha,
                path_at_commit: "wiki/page.md".into(),
                value: "2".into(),
            }
        );
    }

    #[test]
    #[ignore = "Phase 0 P3: implement find_anchor_commit"]
    fn anchors_at_field_introduction_when_no_pair_differs() {
        let repo = TestRepo::new();
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("P", "body\n", Some("1")),
        );
        let intro_sha = repo.commit("field=1");
        repo.create_file("wiki/page.md", &make_wiki_page("P", "edited\n", Some("1")));
        repo.commit("body edit");

        let epoch = find_anchor_commit(repo.path(), "wiki/page.md", Some("1"), Some("1"))
            .expect("resolves");
        assert_eq!(
            epoch,
            LinkEpoch::Commit {
                sha: intro_sha,
                path_at_commit: "wiki/page.md".into(),
                value: "1".into(),
            }
        );
    }

    #[test]
    #[ignore = "Phase 0 P3: implement find_anchor_commit"]
    fn nonsquash_merge_preserves_feature_branch_certification() {
        let repo = TestRepo::new();
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("P", "body\n", Some("1")),
        );
        repo.commit("field=1");
        repo.git(&["checkout", "-q", "-b", "feature"]);
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("P", "body\n", Some("2")),
        );
        let bump_sha = repo.commit("bump on feature only");
        repo.git(&["checkout", "-q", "master"]);
        repo.create_file("wiki/other.md", &make_wiki_page("Other", "x\n", None));
        repo.commit("unrelated on master");
        repo.git(&["merge", "--no-ff", "-q", "-m", "merge feature", "feature"]);

        // HEAD is the merge commit; the certification exists only on the
        // feature branch — a --first-parent walk would never see it.
        let epoch = find_anchor_commit(repo.path(), "wiki/page.md", Some("2"), Some("2"))
            .expect("resolves");
        assert_eq!(
            epoch,
            LinkEpoch::Commit {
                sha: bump_sha,
                path_at_commit: "wiki/page.md".into(),
                value: "2".into(),
            }
        );
    }

    #[test]
    #[ignore = "Phase 0 P3: implement find_anchor_commit"]
    fn two_chained_renames_still_resolve_with_the_anchor_time_name() {
        let repo = TestRepo::new();
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("P", "body\n", Some("1")),
        );
        let intro_sha = repo.commit("field=1 at page.md");
        repo.git(&["mv", "wiki/page.md", "wiki/renamed.md"]);
        repo.commit("rename to renamed.md");
        repo.git(&["mv", "wiki/renamed.md", "wiki/final-name.md"]);
        repo.commit("rename to final-name.md");

        let epoch = find_anchor_commit(
            repo.path(),
            "wiki/final-name.md",
            Some("1"),
            Some("1"),
        )
        .expect("resolves");
        assert_eq!(
            epoch,
            LinkEpoch::Commit {
                sha: intro_sha,
                path_at_commit: "wiki/page.md".into(),
                value: "1".into(),
            },
            "the blob is read under the name in effect at the anchor commit"
        );
    }

    #[test]
    #[ignore = "Phase 0 P3: implement find_anchor_commit"]
    fn shallow_clone_fails_closed() {
        let repo = TestRepo::new();
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("P", "body\n", Some("1")),
        );
        repo.commit("field=1");
        repo.create_file("wiki/page.md", &make_wiki_page("P", "edited\n", Some("1")));
        repo.commit("body edit");

        let clone = repo.shallow_clone();
        let err = find_anchor_commit(clone.path(), "wiki/page.md", Some("1"), Some("1"))
            .expect_err("shallow history cannot resolve an anchor epoch");
        assert!(matches!(err, EpochError::ShallowClone), "got {err:?}");
    }

    // ── classify_page: one test per outcome ──

    #[test]
    #[ignore = "Phase 0 P3: implement classify_page"]
    fn healthy_when_target_unchanged() {
        let (repo, c1) = repo_with_certified_link();
        let epoch = LinkEpoch::Commit { sha: c1, path_at_commit: "wiki/page.md".into(), value: "1".into() };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(classes.len(), 1);
        assert_eq!(classes[0].outcome, DriftOutcome::Healthy);
    }

    #[test]
    #[ignore = "Phase 0 P3: implement classify_page"]
    fn drift_when_target_content_changed() {
        let (repo, c1) = repo_with_certified_link();
        repo.create_file("wiki/target.md", "T0\nblock-line-1\nblock-line-2\nCHANGED\nT1\n");
        repo.commit("target edited");
        let epoch = LinkEpoch::Commit { sha: c1, path_at_commit: "wiki/page.md".into(), value: "1".into() };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(classes[0].outcome, DriftOutcome::Drift);
    }

    #[test]
    #[ignore = "Phase 0 P3: implement classify_page"]
    fn uncertified_when_link_added_after_anchor() {
        let (repo, c1) = repo_with_certified_link();
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page(
                "Page",
                "[b](target.md#L2-L4)\n[c](target.md#L1-L1)\n",
                Some("1"),
            ),
        );
        repo.commit("new link, no bump");
        let epoch = LinkEpoch::Commit { sha: c1, path_at_commit: "wiki/page.md".into(), value: "1".into() };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(classes.len(), 2);
        assert_eq!(classes[0].outcome, DriftOutcome::Healthy);
        assert_eq!(classes[1].outcome, DriftOutcome::Uncertified);
    }

    #[test]
    #[ignore = "Phase 0 P3: implement classify_page"]
    fn broken_when_target_deleted() {
        let (repo, c1) = repo_with_certified_link();
        repo.remove_file("wiki/target.md");
        repo.commit("target deleted");
        let epoch = LinkEpoch::Commit { sha: c1, path_at_commit: "wiki/page.md".into(), value: "1".into() };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(classes[0].outcome, DriftOutcome::Broken);
    }

    #[test]
    #[ignore = "Phase 0 P3: implement classify_page"]
    fn broken_when_extent_overhangs_target() {
        let (repo, c1) = repo_with_certified_link();
        repo.create_file("wiki/target.md", "T0\n");
        repo.commit("target truncated");
        let epoch = LinkEpoch::Commit { sha: c1, path_at_commit: "wiki/page.md".into(), value: "1".into() };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(classes[0].outcome, DriftOutcome::Broken);
    }

    #[test]
    #[ignore = "Phase 0 P3: implement classify_page"]
    fn moved_when_content_shifted_within_target() {
        let (repo, c1) = repo_with_certified_link();
        repo.create_file(
            "wiki/target.md",
            "A1\nA2\nT0\nblock-line-1\nblock-line-2\nblock-line-3\nT1\n",
        );
        repo.commit("two lines inserted above the block");
        let epoch = LinkEpoch::Commit { sha: c1, path_at_commit: "wiki/page.md".into(), value: "1".into() };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(
            classes[0].outcome,
            DriftOutcome::Moved { new_path: "wiki/target.md".into(), new_start: 4, new_end: 6 }
        );
    }

    #[test]
    #[ignore = "Phase 0 P3: implement classify_page"]
    fn moved_cross_file_rewrites_path_and_range() {
        let (repo, c1) = repo_with_certified_link();
        repo.create_file("wiki/target.md", "T0\nreplacement\nT1\n");
        repo.create_file("wiki/other.md", &format!("H\n{BLOCK}\nF\n"));
        repo.commit("block moved to other.md");
        let epoch = LinkEpoch::Commit { sha: c1, path_at_commit: "wiki/page.md".into(), value: "1".into() };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(
            classes[0].outcome,
            DriftOutcome::Moved { new_path: "wiki/other.md".into(), new_start: 2, new_end: 4 }
        );
    }

    #[test]
    #[ignore = "Phase 0 P3: implement classify_page"]
    fn unknown_when_content_is_duplicated() {
        let (repo, c1) = repo_with_certified_link();
        repo.create_file(
            "wiki/target.md",
            &format!("T0\nchanged\nX\n{BLOCK}\nY\n{BLOCK}\nZ\n"),
        );
        repo.commit("certified block now occurs twice");
        let epoch = LinkEpoch::Commit { sha: c1, path_at_commit: "wiki/page.md".into(), value: "1".into() };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(classes[0].outcome, DriftOutcome::Unknown);
    }

    #[test]
    #[ignore = "Phase 0 P3: implement classify_page"]
    fn pending_bump_suppresses_certification_but_flags_broken() {
        let (repo, _c1) = repo_with_certified_link();
        repo.create_file(
            "wiki/other.md",
            &format!("H\n{BLOCK}\nF\n"),
        );
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page(
                "Page",
                "[b](target.md#L2-L4)\n[gone](gone.md#L1-L1)\n",
                Some("1"),
            ),
        );
        repo.commit("second link to a live target");
        // Worktree: target drifted, gone.md never exists — no commit, so the
        // field bump and the target edit are pending.
        repo.create_file("wiki/target.md", "T0\nblock-line-1\nblock-line-2\nCHANGED\nT1\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page(
                "Page",
                "[b](target.md#L2-L4)\n[gone](gone.md#L1-L1)\n",
                Some("2"),
            ),
        );
        let epoch = LinkEpoch::Current { value: Some("2".into()) };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(classes.len(), 2);
        assert_eq!(
            classes[0].outcome, DriftOutcome::Healthy,
            "certification outcomes are suppressed under a pending bump"
        );
        assert_eq!(
            classes[1].outcome, DriftOutcome::Broken,
            "structural failures still flag under a pending bump"
        );
    }

    #[test]
    #[ignore = "Phase 0 P3: implement classify_page"]
    fn missing_epoch_is_rejected_fail_closed() {
        let (repo, _c1) = repo_with_certified_link();
        let err = classify(&repo, &LinkEpoch::Missing, "wiki/page.md").expect_err("fails closed");
        assert!(matches!(err, EpochError::MissingEpoch), "got {err:?}");
    }

    #[test]
    #[ignore = "Phase 0 P3: implement classify_page"]
    fn href_edited_to_equal_content_stays_healthy() {
        let (repo, c1) = repo_with_certified_link();
        repo.create_file(
            "wiki/target.md",
            "A1\nA2\nT0\nblock-line-1\nblock-line-2\nblock-line-3\nT1\n",
        );
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "[b](target.md#L4-L6)\n", Some("1")),
        );
        repo.commit("href follows the shift, no field bump");
        let epoch = LinkEpoch::Commit { sha: c1, path_at_commit: "wiki/page.md".into(), value: "1".into() };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(classes[0].outcome, DriftOutcome::Healthy);
    }

    #[test]
    #[ignore = "Phase 0 P3: implement classify_page"]
    fn href_edited_to_different_content_is_uncertified() {
        let (repo, c1) = repo_with_certified_link();
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "[b](target.md#L1-L1)\n", Some("1")),
        );
        repo.commit("href now points at different content");
        let epoch = LinkEpoch::Commit { sha: c1, path_at_commit: "wiki/page.md".into(), value: "1".into() };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(classes[0].outcome, DriftOutcome::Uncertified);
    }

    #[test]
    #[ignore = "Phase 0 P3: implement classify_page"]
    fn plain_and_heading_links_are_out_of_scope() {
        let (repo, c1) = repo_with_certified_link();
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page(
                "Page",
                "[plain](target.md)\n[head](target.md#some-heading)\n[b](target.md#L2-L4)\n",
                Some("1"),
            ),
        );
        repo.commit("mixed links");
        let epoch = LinkEpoch::Commit { sha: c1, path_at_commit: "wiki/page.md".into(), value: "1".into() };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(classes.len(), 1, "only line-range links are classified");
        assert_eq!(classes[0].target_path, "wiki/target.md");
        assert_eq!(classes[0].start_line, 2);
        assert_eq!(classes[0].end_line, 4);
    }

    #[test]
    #[ignore = "Phase 0 P3: implement classify_page"]
    fn new_link_to_duplicated_content_stays_uncertified() {
        // Round-1 finding 4: content equality must never be the matcher. A
        // brand-new link to a verbatim copy of certified content, under a
        // different label, is NOT certified.
        let (repo, c1) = repo_with_certified_link();
        repo.create_file("wiki/target2.md", &format!("{BLOCK}\n"));
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page(
                "Page",
                "[b](target.md#L2-L4)\n[d](target2.md#L1-L3)\n",
                Some("1"),
            ),
        );
        repo.commit("new link to duplicated content");
        let epoch = LinkEpoch::Commit { sha: c1, path_at_commit: "wiki/page.md".into(), value: "1".into() };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(classes[0].outcome, DriftOutcome::Healthy);
        assert_eq!(classes[1].outcome, DriftOutcome::Uncertified);
    }

    #[test]
    #[ignore = "Phase 0 P3: implement classify_page"]
    fn cross_file_relocation_rerun_is_healthy_via_the_relocation_clause() {
        // The amendment scenario: --fix relocates a cross-file move (new path
        // AND range), and the next run must NOT flag its own rewrite.
        let (repo, c1) = repo_with_certified_link();
        repo.create_file("wiki/target.md", "T0\nreplacement\nT1\n");
        repo.create_file("wiki/other.md", &format!("H\n{BLOCK}\nF\n"));
        repo.commit("block moved to other.md");

        let epoch = LinkEpoch::Commit { sha: c1, path_at_commit: "wiki/page.md".into(), value: "1".into() };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(
            classes[0].outcome,
            DriftOutcome::Moved { new_path: "wiki/other.md".into(), new_start: 2, new_end: 4 }
        );

        // Apply the fix the way --fix would: rewrite the full href.
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "[b](other.md#L2-L4)\n", Some("1")),
        );
        repo.commit("fix applied, field untouched");
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(
            classes[0].outcome, DriftOutcome::Healthy,
            "the relocation clause keeps the tool's own rewrite certified"
        );
    }

    // ── Out-of-scope links are never classified; page-level identity ──

    #[test]
    #[ignore = "Phase 0 P3: implement classify_page"]
    fn classify_uses_label_and_range_identity_not_position() {
        // A page whose body gains text above the link (line shifts) must not
        // change the classification: identity is label/path/range-based, and
        // the anchor-side page is compared for identity, not line position.
        let (repo, c1) = repo_with_certified_link();
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "# New heading\n\n[b](target.md#L2-L4)\n", Some("1")),
        );
        repo.commit("prose added above the link");
        let epoch = LinkEpoch::Commit { sha: c1, path_at_commit: "wiki/page.md".into(), value: "1".into() };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(classes[0].outcome, DriftOutcome::Healthy);
        assert_eq!(classes[0].source_line, 5);
    }
}
