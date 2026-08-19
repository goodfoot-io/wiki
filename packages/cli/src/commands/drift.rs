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
