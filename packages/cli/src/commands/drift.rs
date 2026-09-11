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

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;
use std::time::Instant;

use thiserror::Error;

use crate::frontmatter::{self, scalar_to_string};
use crate::index::DocSource;
use crate::rk64::{
    Extent, LineIndex, ScanIndex, cheap_fingerprint_with_extent, scan_indexed_rk64,
    scan_one_indexed,
};

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
    /// The target exists and the link's range fits it, but the range differs
    /// from the certified range and the move scan found no match — either the
    /// reviewer hand-edited the href (its range, or its path) or the certified
    /// block moved without a unique match. Fail-closed; never auto-fixed — the
    /// remedy is reviewing the link and bumping `links-reviewed:`.
    RangeDiffered,
    /// The certified content was found at exactly one new location —
    /// `--fix` rewrites the href (path and range) to follow it.
    /// `content_identical` is true for an exact-tier match (the destination
    /// is byte-identical to the certified block, so the link stays certified)
    /// and false for a fuzzy-tier match (the destination is a lightly-edited
    /// near-copy — the href follows the move, but the link needs
    /// re-certification; the fix must not claim "certified content moved").
    Moved {
        new_path: String,
        new_start: u32,
        new_end: u32,
        content_identical: bool,
    },
    /// Could not verify (ambiguous move — the certified content occurs at
    /// ≥2 candidate locations). Fail-closed; never auto-fixed.
    Unknown,
    /// A duplicate link with this display text was removed since the anchor
    /// epoch, so the surviving link cannot be matched to a reviewed record.
    /// Distinct from `Unknown`: the ambiguity there is *where* the certified
    /// content went, here it is *which* epoch record certified the survivor —
    /// the content itself may occur exactly once. Fail-closed; never
    /// auto-fixed.
    UnknownLabelDeleted,
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
    /// anchor-side blob is read under the name in effect there. `value` is
    /// `None` when the field is absent at the anchor commit (the anchor can
    /// be a field-less readable commit when unparseable commits above it
    /// were skipped).
    Commit {
        sha: String,
        path_at_commit: String,
        value: Option<String>,
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
    #[error("page `{page}` has unparseable YAML frontmatter — repair it before certification")]
    UnparseableYaml { page: String },
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

/// The outcome of reading a page's `links-reviewed:` frontmatter field,
/// coerced to its string form. `Unparseable` is a distinct state, not a
/// value and not an absence (finding yaml-breakage-rebaselines): a page
/// whose YAML block cannot be parsed must never participate in epoch
/// comparison — a repair commit would otherwise compare equal to the broken
/// commit, re-anchor the page, and silently certify links no human
/// reviewed. Change detection compares the readable strings, so any later
/// value change re-certifies the page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinksReviewedRead {
    /// The field is absent (no YAML block, field missing, or non-scalar
    /// value) or carries the coerced scalar string.
    Readable(Option<String>),
    /// The page has a YAML block that cannot be parsed.
    Unparseable,
}

pub fn read_links_reviewed(content: &str) -> LinksReviewedRead {
    let Some((yaml_start, yaml_end, _)) = frontmatter::yaml_block_bounds(content) else {
        return LinksReviewedRead::Readable(None);
    };
    let yaml = &content[yaml_start..yaml_end];
    let parsed: serde_yaml::Value = match serde_yaml::from_str(yaml) {
        Ok(p) => p,
        Err(_) => return LinksReviewedRead::Unparseable,
    };
    LinksReviewedRead::Readable(parsed.get("links-reviewed").and_then(scalar_to_string))
}

/// Resolve the page's anchor epoch per plan Decision 2.
///
/// `current_value` is the field read at the current side (worktree fs,
/// `HEAD` blob, or index blob, per `--source`); `committed_value` is the
/// read at `HEAD` (the newest committed value). An unparseable current side
/// fails closed with [`EpochError::UnparseableYaml`] — the page's YAML is
/// broken right now, so no classification can be trusted, and neither a
/// silent pass nor an auto-repair is acceptable. When both sides are
/// readable and differ, the anchor epoch IS the current state
/// (pending-certification rule). When both read `None` the page has no
/// epoch (`Missing`). Only when both are `Some` and equal does the engine
/// walk full ancestry (`git log --follow --name-status --format=%H --
/// <page>`, no commit cap, no `--first-parent`) and anchor at the newer
/// commit of the first adjacent pair whose parsed values differ, or the
/// oldest walked commit if no pair differs. An unparseable committed side
/// is not a value and not `Missing` — the walk resolves the epoch from the
/// readable history, skipping unparseable commits entirely. A shallow clone
/// is detected via `git rev-parse --is-shallow-repository` and fails closed
/// with [`EpochError::ShallowClone`].
///
/// `cache` is the anchor-cache seam: the walk leg below consults the
/// anchor tier (Phase 3). The `Current`/`Missing` early returns above
/// never reach the walk — and therefore never touch the cache and emit
/// no walk events.
pub fn find_anchor_commit(
    repo_root: &Path,
    reader: &crate::git::GitReader,
    cache: &dyn crate::cache::AnchorCache,
    run: &DriftRunCtx,
    page_path: &str,
    current_value: &LinksReviewedRead,
    committed_value: &LinksReviewedRead,
) -> Result<LinkEpoch, EpochError> {
    // Fail closed on an unparseable current side (lead tiebreak: an
    // explicit error, never a silent pass). `--fix` cannot repair broken
    // YAML; the page needs a human edit.
    let LinksReviewedRead::Readable(current_value) = current_value else {
        return Err(EpochError::UnparseableYaml {
            page: page_path.to_string(),
        });
    };
    match committed_value {
        LinksReviewedRead::Readable(committed_value) => {
            if current_value != committed_value {
                // Pending-certification rule (Decision 2): a field bump,
                // addition, or removal that is not yet committed makes the
                // anchor epoch the current state itself.
                return Ok(LinkEpoch::Current {
                    value: current_value.clone(),
                });
            }
            let Some(_committed) = committed_value else {
                // Both sides absent — the page has no anchor epoch.
                return Ok(LinkEpoch::Missing);
            };
            walk_anchor_epoch(repo_root, reader, cache, run, page_path)
        }
        // An unparseable HEAD blob is not a value: the pending rule cannot
        // compare it, and the field is not "missing" — the walk resolves
        // the epoch from the readable history, skipping the broken commit.
        LinksReviewedRead::Unparseable => {
            walk_anchor_epoch(repo_root, reader, cache, run, page_path)
        }
    }
}

/// Full-ancestry walk per plan Decision 3: `git log --follow --name-status
/// --format=%H -- <page>` — no commit cap, no `--first-parent`, so a
/// certification made only on a feature branch survives a non-squash merge.
/// The page's per-commit name is tracked through `R###` rename rows (any
/// similarity suffix is a rename). The anchor is the newer commit of the
/// first adjacent pair (newest→oldest) whose parsed field values differ;
/// when no pair differs the field was introduced at the oldest walked
/// commit, which is the anchor. Commits whose YAML cannot be parsed are
/// skipped entirely — they are not values and not epoch events, so a
/// repair commit compares against the next-older readable value and can
/// never re-anchor the page at itself.
///
/// The Phase 3 anchor-tier seam (plan decision 5): the disk tier wraps the
/// memoized leg — the per-commit blob-read + YAML-parse loop — and the
/// `git log` capture always runs for a page the run has not captured yet,
/// deliberately outside any span, so the span's presence on the miss path is
/// exactly the economy the cache buys. (`DriftRunCtx` memoizes the capture
/// per run — it is a pure function of HEAD and the page path — so the
/// capture a `--fix` run's second drift pass would repeat is not paid
/// again; the span accounting is unchanged either way, since both the
/// capture and the memoized leg sit outside it.) The shallow gate runs
/// first, before any cache consult: a shallow
/// repo emits `cache.walk.bypass` and fails closed unchanged, so warm rows
/// from a full-history past are never served. Read: derive the key from
/// the page path and the exact untrimmed `git log` output (the walk's
/// entire non-blob input — the commit sequence and rename rows are pinned
/// by hashing the output itself) → lookup → serve only a verified row; any
/// miss or lookup error is a miss, never a failure. Write: on a miss the
/// walk's epoch is upserted under the three-valued availability rule (plan
/// decision 1): a failed per-commit read is probed — present ⇒ no row
/// (fail open: object availability can change without the log changing, so
/// a row written while a blob was unreadable is never served), absent ⇒ a
/// genuine deletion/rename boundary, a defined walk input whose epoch is
/// safe to cache, unknown ⇒ no row. The all-unparseable fail-closed
/// ([`EpochError::UnparseableYaml`]) is never cached. The return value and
/// error propagation are byte-identical to the uncached walk in every
/// branch; a cache error is never a serve and never a failure.
fn walk_anchor_epoch(
    repo_root: &Path,
    reader: &crate::git::GitReader,
    cache: &dyn crate::cache::AnchorCache,
    run: &DriftRunCtx,
    page_path: &str,
) -> Result<LinkEpoch, EpochError> {
    // The repository state is the authority: a local-path `--depth 1` clone
    // can silently copy full history, so clone flags must not be trusted.
    // The gate runs FIRST, before any cache consult — the bypassed tier is
    // tallied for the run's aggregated `anchor_cache` event and the run
    // fails closed unchanged. The answer is a property of the object store,
    // so the run evaluates it once and every page reads the same verdict.
    if run.is_shallow(repo_root)? {
        crate::perf::anchor_cache_bypass();
        return Err(EpochError::ShallowClone);
    }

    // One capture per page per run: the fix phase and the post-fix re-check
    // walk the same history, and nothing in a run moves HEAD.
    let log = run.walk_log(repo_root, page_path)?;

    // Tier-A read. The key is the page path plus the exact untrimmed output
    // the walk parses below (`git_output` returns the lossy stdout verbatim
    // — its doc comment's "trimmed" claim is stale), so any history change
    // that moves the commit sequence or rename rows moves the key. Serve
    // only a verified row; any lookup error is a miss, never a failure.
    let cache_key = crate::cache::key::walk_key(page_path, &log);
    if let Some(row) = cache.lookup_walk(&cache_key, page_path, &log).unwrap_or(None) {
        crate::perf::anchor_cache_hit();
        return Ok(LinkEpoch::Commit {
            sha: row.anchor_sha,
            path_at_commit: row.path_at_commit,
            value: row.value,
        });
    }
    crate::perf::anchor_cache_miss();

    // The memoized leg: the existing per-commit blob-read + YAML-parse loop.
    // Failed reads are recorded — the walk still treats them as the
    // field-less state, exactly as uncached — and the availability probe
    // below decides whether the resulting epoch may be cached. Its duration
    // is tallied into the run's aggregated `anchor_cache` event (the walk
    // leg of the economy) — a served hit never reaches this timing, so a
    // warm run reports zero walk milliseconds.
    let mut failed_reads: Vec<(String, String)> = Vec::new();
    let walk_start = Instant::now();
    let epoch = (|| {
            // Walk newest→oldest: (sha, name in effect at that commit,
            // parsed value). Every pushed value is readable by construction;
            // absence at a commit is compared as the field-less state.
            //
            // Every history commit's blob read goes through the caller's gix
            // handle (P3) instead of spawning a `git show` subprocess per
            // commit — and instead of opening the repository here, which
            // re-reads every pack index on a run that walks many pages.
            let mut name = page_path.to_string();
            let mut walked: Vec<(String, String, LinksReviewedRead)> = Vec::new();
            for (sha, rows) in parse_name_status_log(&log) {
                match blob_links_reviewed(reader, &sha, &name)? {
                    Some(LinksReviewedRead::Unparseable) => {} // skipped entirely
                    Some(LinksReviewedRead::Readable(v)) => {
                        walked.push((sha.clone(), name.clone(), LinksReviewedRead::Readable(v)));
                    }
                    None => {
                        failed_reads.push((sha.clone(), name.clone()));
                        walked.push((sha.clone(), name.clone(), LinksReviewedRead::Readable(None)));
                    }
                }
                // The pre-commit name for the next (older) commit comes from
                // the rename row whose new path is the name in effect at
                // this commit.
                if let Some(row) = rows.iter().find(|r| r.is_rename_to(&name)) {
                    name = row.old_path.clone();
                }
            }

            for pair in walked.windows(2) {
                let (newer, older) = (&pair[0], &pair[1]);
                if newer.2 != older.2 {
                    // Invariant: unparseable commits were skipped, so both
                    // sides of every pair are readable — the newer side
                    // anchors, whether it carries the field or the field is
                    // absent there.
                    return Ok(LinkEpoch::Commit {
                        sha: newer.0.clone(),
                        path_at_commit: newer.1.clone(),
                        value: match &newer.2 {
                            LinksReviewedRead::Readable(v) => v.clone(),
                            LinksReviewedRead::Unparseable => {
                                unreachable!("unparseable commits are skipped")
                            }
                        },
                    });
                }
            }
            let Some(anchor) = walked.last() else {
                // Every walked commit carried unparseable YAML: no readable
                // value exists to anchor on. Fail closed rather than
                // anchoring at a broken commit.
                return Err(EpochError::UnparseableYaml {
                    page: page_path.to_string(),
                });
            };
            Ok(LinkEpoch::Commit {
                sha: anchor.0.clone(),
                path_at_commit: anchor.1.clone(),
                value: match &anchor.2 {
                    LinksReviewedRead::Readable(v) => v.clone(),
                    LinksReviewedRead::Unparseable => {
                        unreachable!("unparseable commits are skipped")
                    }
                },
            })
    })();
    crate::perf::anchor_cache_add_leg("walk", walk_start.elapsed().as_nanos() as u64);
    let epoch = epoch?;

    // The write: the walk's epoch is cached only when no failed read probed
    // present or unknown — an unreadable blob is not a defined walk input,
    // and its availability can change without the log changing, so the
    // epoch it produced must never serve. A probed-absent read is a genuine
    // deletion/rename boundary: the field-less state is the true value at
    // that commit, and the epoch is a defined computation.
    let mut cacheable = true;
    for (sha, name) in &failed_reads {
        if availability_probe(repo_root, sha, name) != Availability::Absent {
            cacheable = false;
            break;
        }
    }
    if cacheable {
        let LinkEpoch::Commit {
            sha,
            path_at_commit,
            value,
        } = &epoch
        else {
            unreachable!("the walk resolves to a commit epoch or fails closed")
        };
        let _ = cache.upsert_walk(
            &cache_key,
            page_path,
            &crate::cache::key::sha256_hex(log.as_bytes()),
            sha,
            path_at_commit,
            value.as_deref(),
        );
    }
    Ok(epoch)
}

/// One `--name-status` row: the status token (letter plus informational
/// similarity suffix) and the path(s). Renames and copies carry two paths.
#[derive(Debug)]
pub(crate) struct NameStatusRow {
    status: String,
    /// The pre-image path (for non-rename rows, the single affected path).
    pub(crate) old_path: String,
    /// The post-image path (equal to `old_path` unless the row is a rename).
    pub(crate) new_path: String,
}

impl NameStatusRow {
    /// Any `R###` row is a rename — the similarity suffix is informational.
    pub(crate) fn is_rename(&self) -> bool {
        self.status.starts_with('R')
    }

    fn is_rename_to(&self, name: &str) -> bool {
        self.is_rename() && self.new_path == name
    }
}

/// Parse `git log --name-status --format=%H` output into `(sha, rows)` pairs,
/// newest commit first. A blank line separates commit records. Shared with
/// the fix phase's deleted-path rename lookup (`check_fix.rs`), which parses
/// the same output shape.
pub(crate) fn parse_name_status_log(log: &str) -> Vec<(String, Vec<NameStatusRow>)> {
    let mut out: Vec<(String, Vec<NameStatusRow>)> = Vec::new();
    for line in log.lines() {
        if line.is_empty() {
            continue;
        }
        if line.len() == 40 && line.bytes().all(|b| b.is_ascii_hexdigit()) {
            out.push((line.to_string(), Vec::new()));
            continue;
        }
        let Some((_, rows)) = out.last_mut() else {
            continue; // a row before the first commit record — not produced by git
        };
        let mut parts = line.split('\t');
        let status = parts.next().unwrap_or_default().to_string();
        let first = parts.next().unwrap_or_default().to_string();
        let second = parts.next().unwrap_or_default();
        rows.push(NameStatusRow {
            status,
            old_path: first.clone(),
            new_path: if second.is_empty() {
                first
            } else {
                second.to_string()
            },
        });
    }
    out
}

/// The page's parsed field read at `commit`, under the name in effect there.
/// `Ok(None)` when the path is absent at that commit — a walk boundary, not
/// an error. A present-but-unparseable page reports `Unparseable` so the
/// walk can skip the commit instead of treating it as a value.
///
/// Reads through the caller-held [`crate::git::GitReader`] so a walk over
/// many commits opens the repository once.
fn blob_links_reviewed(
    reader: &crate::git::GitReader,
    commit: &str,
    path: &str,
) -> Result<Option<LinksReviewedRead>, EpochError> {
    match reader.read_blob_at_commit(commit, path).map_err(git_failed)? {
        None => Ok(None),
        Some(bytes) => Ok(Some(read_links_reviewed(&String::from_utf8_lossy(&bytes)))),
    }
}

/// Map an in-process git failure (gix open/object/tree error) onto the
/// fail-closed [`EpochError::GitFailed`], preserving the full cause chain.
fn git_failed(e: miette::Report) -> EpochError {
    EpochError::GitFailed(format!("{e:?}"))
}

/// Read a blob at `<commit>:<path>` — `git show <commit>:<path>` semantics.
///
/// A full 40-hex commit SHA is served in-process through the caller's
/// [`crate::git::GitReader`], so a run that reads many anchor blobs pays the
/// repository open once. Any other rev spec keeps the subprocess:
/// `HEAD:path`, and the index form `:path` when `commit` is empty.
/// `Ok(None)` when the path is absent there; any other git failure fails
/// closed.
pub(crate) fn read_blob_at(
    reader: &crate::git::GitReader,
    repo_root: &Path,
    commit: &str,
    path: &str,
) -> Result<Option<Vec<u8>>, EpochError> {
    if commit.len() == 40 && commit.bytes().all(|b| b.is_ascii_hexdigit()) {
        return reader.read_blob_at_commit(commit, path).map_err(git_failed);
    }
    let spec = if commit.is_empty() {
        format!(":{path}")
    } else {
        format!("{commit}:{path}")
    };
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["show", &spec])
        .output()
        .map_err(|e| EpochError::GitFailed(e.to_string()))?;
    if output.status.success() {
        return Ok(Some(output.stdout));
    }
    if output.status.code() == Some(128) {
        return Ok(None); // absent at this commit
    }
    Err(EpochError::GitFailed(
        String::from_utf8_lossy(&output.stderr).trim().to_string(),
    ))
}

/// The three-valued outcome of the availability probe (plan decision 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Availability {
    /// The target's tree entry exists at the commit — the failed read was an
    /// availability failure, so the computed value must not be cached.
    Present,
    /// A completed tree read with no entry — genuine absence at the commit;
    /// the computed value (fp 0) is a defined input and may be cached.
    Absent,
    /// The tree itself could not be read, or git failed — no write.
    Unknown,
}

/// Classify a completed `git ls-tree <sha> -- :(literal)<path>` output into
/// the three-valued availability (plan decision 1): exit 0 with a non-empty
/// listing = present; exit 0 with an empty listing = absent; anything else
/// — including git's `not a tree object` exit-128 family — = unknown. A
/// two-valued exit-code reading is unsound: git's `does not exist in
/// '<sha>'` message also fires when the tree cannot be read, so exit 128 is
/// not absence.
fn classify_ls_tree_output(output: &std::process::Output) -> Availability {
    if output.status.success() {
        if output.stdout.is_empty() {
            Availability::Absent
        } else {
            Availability::Present
        }
    } else {
        Availability::Unknown
    }
}

/// The three-valued availability probe (plan decision 1): `git ls-tree
/// <anchor_sha> -- :(literal)<target_path>` — the verbatim path the failed
/// read used (`:(literal)` defeats pathspec glob/magic; a listing works
/// even when blobs are missing, and partial-clone trees are present when
/// blobs are not — the `blob:none` case). Runs only after a failed blob
/// read, on the write path; a spawn failure is unknown — fail open toward
/// no write, never a failure of the run.
fn availability_probe(repo_root: &Path, anchor_sha: &str, target_path: &str) -> Availability {
    let spec = format!(":(literal){target_path}");
    match Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["ls-tree", anchor_sha, "--", &spec])
        .output()
    {
        Ok(output) => classify_ls_tree_output(&output),
        Err(_) => Availability::Unknown,
    }
}

/// Run git in `repo_root`, returning stdout trimmed of the trailing newline;
/// any failure is [`EpochError::GitFailed`].
fn git_output(repo_root: &Path, args: &[&str]) -> Result<String, EpochError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(args)
        .output()
        .map_err(|e| EpochError::GitFailed(e.to_string()))?;
    if !output.status.success() {
        return Err(EpochError::GitFailed(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Repository facts one `wiki check` invocation needs from the drift
/// machinery but that cannot change while it runs — so they are established
/// once per run instead of once per page or per pass.
///
/// The three facts:
///
/// * **The shallow gate.** `git rev-parse --is-shallow-repository` guards
///   every walk entry and is what makes a shallow clone fail closed. `--fix`
///   runs the drift pass twice (the fix phase and the post-fix re-check), so
///   the per-page gate cost the run one `git` spawn per page per pass.
/// * **The per-page history capture** — `git log --follow --name-status
///   --format=%H -- <page>`, the walk's entire non-blob input and its
///   dominant cost: a full rename-following history walk per page (~200 ms
///   on a large repo, where the anchor cache's own legs are milliseconds).
///   It is a pure function of HEAD and the page path, and `wiki check`
///   never commits, so one capture per page serves both passes.
/// * **The per-destination rename history** — `git log --follow
///   --name-status --format=%H -- <destination>`, the committed name chain a
///   cross-file relocation consults for identity evidence. One walk costs as
///   much as a page capture, a move scan asks it of every file whose content
///   matches, and the same destination answers the same question in both
///   passes: keyed by path on committed history, which the run never moves,
///   so the fix phase's walks serve the post-fix re-check too. (The
///   uncommitted rename rows stay per pass — they read the worktree, which
///   the fix phase rewrites — but they are two `git diff` spawns per pass,
///   not one walk per matching file.)
///
/// Memoizing any of them changes no decision, only the number of
/// subprocesses spent reaching it: the same gate result, the same log bytes
/// and the same name chain feed the same walk. A lookup that fails is never
/// memoized, so the walk's error semantics are exactly those of the
/// per-page capture or per-destination walk it replaces.
#[derive(Default)]
pub struct DriftRunCtx {
    shallow: Mutex<Option<bool>>,
    walk_logs: Mutex<HashMap<String, String>>,
    dest_histories: Mutex<HashMap<String, Vec<String>>>,
}

impl DriftRunCtx {
    pub fn new() -> Self {
        Self::default()
    }

    /// `true` when the repository is a shallow clone. Evaluated once: the
    /// answer is a property of the repository's object store, which the run
    /// never mutates. A git failure is returned to the caller and never
    /// memoized, so every page sees the failure the uncached gate would
    /// have produced.
    fn is_shallow(&self, repo_root: &Path) -> Result<bool, EpochError> {
        if let Some(shallow) = *lock(&self.shallow) {
            return Ok(shallow);
        }
        let shallow =
            git_output(repo_root, &["rev-parse", "--is-shallow-repository"])?.trim() == "true";
        *lock(&self.shallow) = Some(shallow);
        Ok(shallow)
    }

    /// The page's history capture, memoized for the run.
    fn walk_log(&self, repo_root: &Path, page_path: &str) -> Result<String, EpochError> {
        if let Some(log) = lock(&self.walk_logs).get(page_path) {
            return Ok(log.clone());
        }
        let log = capture_walk_log(repo_root, page_path)?;
        lock(&self.walk_logs).insert(page_path.to_string(), log.clone());
        Ok(log)
    }

    /// Whether `dest`'s committed rename history contains `name` — the
    /// identity-evidence question, memoized for the run. A walk that fails is
    /// returned to the caller and never memoized, so every match sees the
    /// failure its own walk would have produced.
    fn dest_history_contains(
        &self,
        repo_root: &Path,
        dest: &str,
        name: &str,
    ) -> Result<bool, EpochError> {
        {
            let histories = lock(&self.dest_histories);
            if let Some(names) = histories.get(dest) {
                return Ok(names.iter().any(|n| n == name));
            }
        }
        let names = renamed_from_history(repo_root, dest)?;
        let contains = names.iter().any(|n| n == name);
        lock(&self.dest_histories).insert(dest.to_string(), names);
        Ok(contains)
    }

    /// Warm the per-destination committed-name histories a scan is about to
    /// ask for, on the worker pool. The walk is a pure function of the
    /// repository's immutable history — the same question of the same bytes
    /// no matter which link asks — so the answers may be computed in any
    /// order and any concurrency, and each one spares the evidence loop a
    /// serial `git log --follow` spawn.
    fn prewarm_dest_histories<'a>(
        &self,
        repo_root: &Path,
        dests: impl IntoIterator<Item = &'a str>,
    ) -> Result<(), EpochError> {
        let pending: Vec<&str> = {
            let histories = lock(&self.dest_histories);
            dests
                .into_iter()
                .filter(|dest| !histories.contains_key(*dest))
                .collect()
        };
        if pending.is_empty() {
            return Ok(());
        }
        let walked = parallel_map(&pending, |dest| {
            renamed_from_history(repo_root, dest).map(|names| ((*dest).to_string(), names))
        });
        for result in walked {
            let (dest, names) = result?;
            lock(&self.dest_histories).entry(dest).or_insert(names);
        }
        Ok(())
    }

    /// Capture the history logs of `page_paths` in parallel, so the serial
    /// classification loop that follows does not pay one `git log` spawn per
    /// page. Purely a schedule change: each worker runs the same command the
    /// lazy [`Self::walk_log`] would, a page whose capture fails stays
    /// uncached for the serial path to retry and report, and an already
    /// memoized page is skipped — so a second drift pass re-pays nothing.
    ///
    /// The scheduling itself is [`parallel_map`]'s; this adds only the
    /// memoizing insert, and the failure policy the cache requires — a capture
    /// that fails is left for the serial path rather than failing the run.
    pub fn precapture(&self, repo_root: &Path, page_paths: &[String]) {
        let pending: Vec<&String> = {
            let logs = lock(&self.walk_logs);
            page_paths
                .iter()
                .filter(|p| !logs.contains_key(*p))
                .collect()
        };
        if pending.is_empty() {
            return;
        }

        let captured = parallel_map(&pending, |page| {
            capture_walk_log(repo_root, page.as_str()).map(|log| ((*page).clone(), log))
        });
        let mut logs = lock(&self.walk_logs);
        for (page, log) in captured.into_iter().flatten() {
            logs.entry(page).or_insert(log);
        }
    }
}

/// Lock a run-context mutex, recovering the data if a worker panicked while
/// holding it: the guarded values are plain caches of read-only repository
/// facts, so a poisoned lock leaves them consistent — and a cache may never
/// be the reason a run fails.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// Apply `map` to every item across the run's bounded worker count (the
/// [`MoveScanCtx::precapture`] idiom), collecting the results in item order so
/// a parallel pass is indistinguishable from the serial loop it replaces.
///
/// A worker that panics is re-raised here rather than dropped. Every caller
/// treats its result as exhaustive — the complete match set, the complete
/// history table — and a silently missing chunk is silently missing work.
fn parallel_map<T, R, F>(items: &[T], map: F) -> Vec<R>
where
    T: Sync,
    R: Send,
    F: Fn(&T) -> R + Sync,
{
    if items.is_empty() {
        return Vec::new();
    }
    // Bounded worker count, matching the content cache's warm-up: at most one
    // thread per item, capped at 8, never more than the machine's parallelism.
    let parallelism = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let worker_count = parallelism.min(8).min(items.len()).max(1);
    let chunk_size = items.len().div_ceil(worker_count);

    let mut collected: Vec<Vec<R>> = Vec::with_capacity(worker_count);
    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(worker_count);
        for chunk in items.chunks(chunk_size) {
            let map = &map;
            handles.push(scope.spawn(move || {
                chunk.iter().map(map).collect::<Vec<R>>()
            }));
        }
        for handle in handles {
            match handle.join() {
                Ok(out) => collected.push(out),
                Err(panic) => std::panic::resume_unwind(panic),
            }
        }
    });
    collected.into_iter().flatten().collect()
}

/// [`parallel_map`] for scans that accumulate into a hit list: each worker
/// keeps one buffer for the whole chunk.
fn parallel_scan<T, F>(items: &[T], scan: F) -> Vec<crate::rk64::Location>
where
    T: Sync,
    F: Fn(&T, &mut Vec<crate::rk64::Location>) + Sync,
{
    parallel_map(items, |item| {
        let mut hits = Vec::new();
        scan(item, &mut hits);
        hits
    })
    .into_iter()
    .flatten()
    .collect()
}

/// The walk's non-blob input for one page: the full rename-following history
/// of the page, newest first, as `git log --follow --name-status
/// --format=%H -- <page>` prints it (the walk parses and hashes exactly
/// these bytes).
fn capture_walk_log(repo_root: &Path, page_path: &str) -> Result<String, EpochError> {
    git_output(
        repo_root,
        &[
            "log",
            "--follow",
            "--name-status",
            "--format=%H",
            "--",
            page_path,
        ],
    )
}

/// Per-pass move-scan context. The candidate inventory — `git ls-files`, a
/// full read of every tracked candidate file, and each file's exact-tier
/// [`ScanIndex`] (see [`CandidateFile`]) — is expensive, and every link's
/// cross-file move scan needs it. Build it lazily on the first scan that
/// needs it and reuse it for the rest of the pass; runs with no move scans
/// pay nothing.
///
/// The inventory and the decisions taken against it are per pass, not per
/// run: the fix phase rewrites the pages whose bytes those decisions were
/// computed against, so the post-fix re-check must read the rewritten tree
/// and judge it afresh. What the run's own repository facts hold — the
/// per-destination rename histories the evidence loop consults — is borrowed
/// from [`DriftRunCtx`], which is what lets the second pass ask the same
/// question without paying for the walk twice.
pub struct MoveScanCtx<'a> {
    run: &'a DriftRunCtx,
    candidates: Option<Vec<CandidateFile>>,
    /// Rename rows from the uncommitted diff layers (worktree↔index and
    /// index↔HEAD) — cross-file relocation evidence (amendment Change 2).
    uncommitted_renames: Option<Vec<(String, String)>>,
    /// Move-scan decisions for the pass, keyed by the certified block that
    /// was scanned for. A page routinely cites the same (target, range) more
    /// than once — the auth protocol page repeats one link fourteen times —
    /// and every citation of a block asks the identical question of the
    /// identical corpus: whether the certified content occurs somewhere it
    /// may be relocated to. Answering it twice is pure waste, so the verdict
    /// of the whole scan is memoized here and replayed.
    decisions: HashMap<MoveScanKey, MoveScanDecision>,
}

/// The identity of one move-scan question: everything the verdict is a
/// function of, and nothing else. The anchor epoch selects the certified
/// content; the target path and range select the block within it; the page is
/// part of the question because the candidate inventory excludes the page
/// itself (a page quoting the range must not relocate the link into itself),
/// so two pages citing the same block are two different questions.
#[derive(PartialEq, Eq, Hash)]
struct MoveScanKey {
    anchor_sha: String,
    page_path: String,
    target_path: String,
    start: u32,
    end: u32,
}

/// A move scan's verdict — both tiers' whole decision, before the caller's
/// own `zero_matches` outcome is substituted for "the content is nowhere at
/// all". `Ambiguous` is the ≥2-qualifying-match rule in both tiers, which
/// reports `Unknown` regardless of how many further matches exist.
#[derive(Clone)]
enum MoveScanDecision {
    Moved {
        location: crate::rk64::Location,
        content_identical: bool,
    },
    Ambiguous,
    Unmatched,
}

impl MoveScanDecision {
    /// The verdict as the caller's outcome, substituting `zero_matches` — the
    /// link's own terminal outcome when the certified content is nowhere.
    fn into_outcome(self, zero_matches: DriftOutcome) -> DriftOutcome {
        match self {
            Self::Moved {
                location,
                content_identical,
            } => DriftOutcome::Moved {
                new_path: location.path,
                new_start: location.start_line,
                new_end: location.end_line,
                content_identical,
            },
            Self::Ambiguous => DriftOutcome::Unknown,
            Self::Unmatched => zero_matches,
        }
    }
}

impl<'a> MoveScanCtx<'a> {
    pub fn new(run: &'a DriftRunCtx) -> Self {
        Self {
            run,
            candidates: None,
            uncommitted_renames: None,
            decisions: HashMap::new(),
        }
    }

    fn candidates(
        &mut self,
        reader: &crate::git::GitReader,
        repo_root: &Path,
        source: DocSource,
    ) -> Result<&[CandidateFile], EpochError> {
        if self.candidates.is_none() {
            self.candidates = Some(candidate_files(reader, repo_root, source)?);
        }
        Ok(self.candidates.as_deref().expect("just populated"))
    }

    fn uncommitted_renames(
        &mut self,
        repo_root: &Path,
    ) -> Result<&[(String, String)], EpochError> {
        if self.uncommitted_renames.is_none() {
            self.uncommitted_renames = Some(uncommitted_rename_rows(repo_root)?);
        }
        Ok(self.uncommitted_renames.as_deref().expect("just populated"))
    }
}

/// Rename rows from the uncommitted layers: `git diff --diff-filter=R
/// --name-status` (worktree↔index) and `git diff --cached --diff-filter=R
/// --name-status` (index↔HEAD). A staged `git mv` appears in the cached
/// layer; a plain unstaged `mv` is invisible to git until staged and is
/// fail-closed (no evidence — a content-only match in an unrelated file is
/// never relocated).
fn uncommitted_rename_rows(repo_root: &Path) -> Result<Vec<(String, String)>, EpochError> {
    let mut rows = Vec::new();
    for cached in [false, true] {
        let mut args = vec!["diff"];
        if cached {
            args.push("--cached");
        }
        args.extend(["--diff-filter=R", "--name-status"]);
        let output = Command::new("git")
            .arg("-C")
            .arg(repo_root)
            .args(&args)
            .output()
            .map_err(|e| EpochError::GitFailed(e.to_string()))?;
        if !output.status.success() {
            // A repo with no HEAD makes the cached layer fail; treat it as
            // empty — the committed history layer still decides.
            continue;
        }
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            if !line.starts_with('R') {
                continue;
            }
            let mut parts = line.splitn(3, '\t');
            let _status = parts.next();
            let old = parts.next().unwrap_or_default().to_string();
            let new = parts.next().unwrap_or_default().to_string();
            if !old.is_empty() && !new.is_empty() {
                rows.push((old, new));
            }
        }
    }
    Ok(rows)
}

/// The paths a destination file has been renamed from, most recent first:
/// `git log --follow --name-status --format=%H -- <dest>` with the name
/// tracked backwards through `R###` rows — the anchor-epoch walk technique
/// applied to an existing target.
fn renamed_from_history(repo_root: &Path, dest_path: &str) -> Result<Vec<String>, EpochError> {
    let log = git_output(
        repo_root,
        &[
            "log",
            "--follow",
            "--name-status",
            "--format=%H",
            "--",
            dest_path,
        ],
    )?;
    let mut name = dest_path.to_string();
    let mut old_names = Vec::new();
    for (_, rows) in parse_name_status_log(&log) {
        if let Some(row) = rows.iter().find(|r| r.is_rename_to(&name)) {
            name = row.old_path.clone();
            old_names.push(name.clone());
        }
    }
    Ok(old_names)
}

/// Amendment Change 2: a cross-file match may relocate only when the
/// destination file carries identity evidence connecting it to the source.
/// The destination is evidenced when it is the certified path itself (the
/// content moved within its own file), when an uncommitted diff-layer rename
/// row pairs source → destination, or when the source path appears in the
/// destination's committed rename history — i.e. the destination is the
/// source's committed successor. A content-only match in an unrelated file
/// (a quote, a copy) is never a move.
fn has_identity_evidence(
    ctx: &mut MoveScanCtx<'_>,
    repo_root: &Path,
    cert_path: &str,
    dest_path: &str,
) -> Result<bool, EpochError> {
    if dest_path == cert_path {
        return Ok(true);
    }
    if ctx
        .uncommitted_renames(repo_root)?
        .iter()
        .any(|(old, new)| old == cert_path && new == dest_path)
    {
        return Ok(true);
    }
    ctx.run.dest_history_contains(repo_root, dest_path, cert_path)
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
///
/// `cache` is the anchor-cache seam: threaded into the per-link
/// classification and its move scans so a page's committed history is walked
/// once and reused across passes.
#[allow(clippy::too_many_arguments)]
pub fn classify_page(
    repo_root: &Path,
    reader: &crate::git::GitReader,
    cache: &dyn crate::cache::AnchorCache,
    source: DocSource,
    page_path: &str,
    page_content: &str,
    epoch: &LinkEpoch,
    ctx: &mut MoveScanCtx<'_>,
) -> Result<Vec<LinkClass>, EpochError> {
    let anchor = match epoch {
        LinkEpoch::Missing => return Err(EpochError::MissingEpoch),
        LinkEpoch::Current { .. } => None,
        LinkEpoch::Commit {
            sha,
            path_at_commit,
            ..
        } => Some((sha.as_str(), path_at_commit.as_str())),
    };

    // The certified locator set: the anchor-commit page's own line-range
    // links, target paths resolved relative to the page's directory as it
    // was at that commit.
    let mut certified: Vec<CertifiedLink> = Vec::new();
    if let Some((sha, path_at_commit)) = anchor {
        let Some(anchor_page) = read_blob_at(reader, repo_root, sha, path_at_commit)? else {
            // The page must exist at its own anchor commit.
            return Err(EpochError::UnreadableBlob {
                page: path_at_commit.to_string(),
                commit: sha.to_string(),
            });
        };
        let anchor_page = String::from_utf8_lossy(&anchor_page);
        for link in parse_line_range_links(&anchor_page) {
            certified.push(CertifiedLink {
                target_path: resolve_target_path(repo_root, path_at_commit, &link.target_path_raw),
                start: link.start,
                end: link.end,
                label: link.label,
                fragment: link.fragment,
                label_ordinal: link.label_ordinal,
                label_count: link.label_count,
            });
        }
    }

    let mut classes = Vec::new();
    for link in parse_line_range_links(page_content) {
        // The effective path is the resolved path after suffix salvage, so
        // `target_path` reports where the link was actually judged against.
        let (outcome, target_path) = classify_link(
            repo_root,
            reader,
            cache,
            source,
            page_path,
            &link,
            &certified,
            anchor.map(|(sha, _)| sha),
            ctx,
        )?;
        classes.push(LinkClass {
            target_path,
            start_line: link.start,
            end_line: link.end,
            source_line: link.source_line,
            href_byte_start: link.href_byte_start,
            href_byte_end: link.href_byte_end,
            label: link.label,
            original_href: link.original_href,
            outcome,
        });
    }
    Ok(classes)
}

/// One link parsed from a page — the current side's or the anchor side's.
#[derive(Debug, Clone)]
struct ParsedLink {
    /// The href path part as written (`""` for a same-page link).
    target_path_raw: String,
    /// The fragment as written, e.g. `L2-L4`.
    fragment: String,
    /// The parsed 1-based range; `(0, 0)` for a range-shaped but invalid
    /// fragment (`L0`, `L3-L2`) — reported `Broken`, never skipped.
    start: u32,
    end: u32,
    label: String,
    source_line: usize,
    href_byte_start: usize,
    href_byte_end: usize,
    original_href: String,
    /// 1-based occurrence ordinal of this display text among links with
    /// byte-identical labels in the page (document order). Half of the
    /// epoch-record identity: a relocation rewrites path+range only, never
    /// the display text, so (label, ordinal) is the stable key.
    label_ordinal: usize,
    /// How many links in the page share this display text. The ordinal
    /// pairing against the epoch is only reliable when the two pages agree
    /// on this count.
    label_count: usize,
}

/// A line-range link on the anchor-commit page — one certified locator.
#[derive(Debug, Clone)]
struct CertifiedLink {
    /// Resolved repo-relative target path at the anchor commit.
    target_path: String,
    start: u32,
    end: u32,
    label: String,
    /// The fragment as written at the anchor commit.
    fragment: String,
    /// The link's occurrence ordinal and total count for its display text on
    /// the anchor page — the epoch side of the (label, ordinal) identity.
    label_ordinal: usize,
    label_count: usize,
}

/// True when `content` contains at least one line-range fragment link — the
/// population the whole-page certification covers. Pages without one need no
/// `links-reviewed:` field: there is nothing to vouch for.
pub fn has_line_range_links(content: &str) -> bool {
    !parse_line_range_links(content).is_empty()
}

/// Parse every line-range fragment link in `content`, in document order.
/// Plain paths and heading-slug fragments are outside this system's scope.
fn parse_line_range_links(content: &str) -> Vec<ParsedLink> {
    // Blank code blocks, inline code, and HTML comments with spaces of equal
    // length before scanning: the drift pass must parse the same link
    // population the main parser sees, and a placeholder example inside a
    // code span is not a link. Equal-length blanking keeps byte offsets and
    // line numbers aligned with the original content.
    let scrubbed = crate::parser::scrub_non_content(content);
    let bytes = scrubbed.as_bytes();
    let line_starts = line_start_offsets(bytes);
    let mut links = Vec::new();
    let mut i = 0usize;
    while i + 1 < bytes.len() {
        if bytes[i] != b']' || bytes[i + 1] != b'(' {
            i += 1;
            continue;
        }
        let href_start = i + 2;
        let Some(href_len) = bytes[href_start..].iter().position(|&b| b == b')') else {
            i += 1;
            continue;
        };
        let href_end = href_start + href_len;
        let href = &scrubbed[href_start..href_end];
        let Some((path_part, fragment)) = href.split_once('#') else {
            i = href_end + 1;
            continue;
        };
        let Some((start, end)) = parse_fragment(fragment) else {
            i = href_end + 1;
            continue;
        };
        let Some(label_start) = find_label_start(bytes, i) else {
            i += 1;
            continue;
        };
        links.push(ParsedLink {
            target_path_raw: path_part.to_string(),
            fragment: fragment.to_string(),
            start,
            end,
            label: scrubbed[label_start + 1..i].to_string(),
            source_line: line_of(&line_starts, i),
            href_byte_start: href_start,
            href_byte_end: href_end,
            original_href: href.to_string(),
            label_ordinal: 0,
            label_count: 0,
        });
        i = href_end + 1;
    }

    // Epoch-record identity: the display text plus its occurrence ordinal
    // among identical display texts (document order). Both the anchor page
    // and the current page run this same parser, so the ordinals compare
    // one-to-one — and a relocation rewrites only path+range, never the
    // display text, so (label, ordinal) survives relocation.
    let mut counts: HashMap<String, usize> = HashMap::new();
    for l in &links {
        *counts.entry(l.label.clone()).or_default() += 1;
    }
    let mut seen: HashMap<String, usize> = HashMap::new();
    for l in &mut links {
        let ordinal = seen.entry(l.label.clone()).or_default();
        *ordinal += 1;
        l.label_ordinal = *ordinal;
        l.label_count = counts[&l.label];
    }
    links
}

/// Parse a fragment as a line range: `L2-L4` or the single-line shorthand
/// `L5`. `None` for fragments that are not ranges (heading slugs, plain
/// anchors). A range-shaped but invalid fragment (`L0`, `L3-L2`) yields
/// `Some((0, 0))` — a degenerate extent the classifier reports `Broken`
/// rather than silently letting it escape the check.
fn parse_fragment(fragment: &str) -> Option<(u32, u32)> {
    let rest = fragment.strip_prefix('L')?;
    if rest.is_empty() {
        return None;
    }
    let digits = |s: &str| s.parse::<u32>().ok();
    if let Some((a, b)) = rest.split_once('-') {
        if b.contains('-') {
            return None; // "L2-L4-x" is not a range
        }
        // "L2-L4" strips the outer `L` once; the second number carries its
        // own `L` prefix.
        let b = b.strip_prefix('L').unwrap_or(b);
        match (digits(a), digits(b)) {
            (Some(a), Some(b)) => {
                if a > 0 && b >= a {
                    Some((a, b))
                } else {
                    Some((0, 0))
                }
            }
            _ => None, // "L2-" or "Lx-Ly" — not range-shaped
        }
    } else {
        match digits(rest) {
            Some(n) if n > 0 => Some((n, n)),
            Some(_) => Some((0, 0)), // "L0"
            None => None,            // "Lx" — not a range
        }
    }
}

/// The `[` that opens the label ending at `close_bracket`, balancing nested
/// brackets so `[[a]](href)` parses as label `[a]`.
fn find_label_start(bytes: &[u8], close_bracket: usize) -> Option<usize> {
    let mut depth = 0usize;
    for j in (0..close_bracket).rev() {
        match bytes[j] {
            b']' => depth += 1,
            b'[' if depth > 0 => depth -= 1,
            b'[' => return Some(j),
            _ => {}
        }
    }
    None
}

/// Offsets of every line start, for 1-based line lookup.
fn line_start_offsets(bytes: &[u8]) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    starts
}

/// 1-based line number containing `byte`.
fn line_of(starts: &[usize], byte: usize) -> usize {
    starts.partition_point(|&s| s <= byte)
}

/// Resolve a link's path part to a repo-relative path per
/// `resolve_link_path`'s rules (leading `/` is repo-root-absolute, anything
/// else resolves relative to the page's directory).
fn resolve_target_path(repo_root: &Path, page_path: &str, path_part: &str) -> String {
    super::resolve_link_path(path_part, &repo_root.join(page_path), repo_root)
        .to_string_lossy()
        .into_owned()
}

/// Classify one link per the Decision 5 flowchart, returning the outcome
/// together with the effective target path — the resolved path after suffix
/// salvage — so the caller reports where the link was actually judged
/// against, not where the stale href happened to point.
#[allow(clippy::too_many_arguments)]
fn classify_link(
    repo_root: &Path,
    reader: &crate::git::GitReader,
    cache: &dyn crate::cache::AnchorCache,
    source: DocSource,
    page_path: &str,
    link: &ParsedLink,
    certified: &[CertifiedLink],
    anchor_sha: Option<&str>,
    ctx: &mut MoveScanCtx<'_>,
) -> Result<(DriftOutcome, String), EpochError> {
    // Decision 5 target resolution: resolve the href path part, then salvage
    // the longest existing repo-relative suffix when the direct read misses.
    let (target_path, target_bytes) =
        read_target(reader, repo_root, source, page_path, &link.target_path_raw)?;

    let Some(anchor_sha) = anchor_sha else {
        // Pending-bump override (Decision 2): the anchor epoch IS the current
        // state, so certification outcomes are suppressed and only structural
        // failure still flags.
        let outcome = match &target_bytes {
            None => DriftOutcome::Broken,
            Some(b) if !extent_fits(b, link.start, link.end) => DriftOutcome::Broken,
            Some(_) => DriftOutcome::Healthy,
        };
        return Ok((outcome, target_path));
    };

    // Decision 4 presence: identity-based — same resolved target path AND
    // (same label OR same href-range string). The label identity is the
    // display text plus its occurrence ordinal among identical display texts
    // (the epoch-record key): a relocation rewrites path+range only, never
    // the display text, so (label, ordinal) is stable across relocations.
    // The ordinal pairing is only reliable when the epoch page and the
    // current page agree on how many links share the display text — a
    // duplicate deleted between epoch and now leaves the survivor unpaired
    // (Unknown, never a guess at which record certified it), while a
    // duplicate added now pairs by ordinal and the extras have no record
    // (Uncertified).
    let label_match = |c: &CertifiedLink| {
        c.label == link.label
            && c.label_ordinal == link.label_ordinal
            && link.label_count >= c.label_count
    };
    let label_deleted = certified
        .iter()
        .any(|c| c.label == link.label && c.label_count > link.label_count);
    let mut matched: Vec<&CertifiedLink> = Vec::new();
    let mut memo: HashMap<(String, u32, u32), u64> = HashMap::new();
    let current_fp = current_content_fp(target_bytes.as_deref(), link.start, link.end);
    for c in certified {
        if c.target_path == target_path && (label_match(c) || c.fragment == link.fragment) {
            matched.push(c);
        }
    }

    if matched.is_empty() {
        // The locator is not present at the anchor epoch (amendment Change 1):
        // after a committed relocation the stored coordinates never existed
        // at the epoch, so the same-label record supplies the certified
        // content. The record's OWN path+range at the anchor commit is what
        // every comparison and the move scan key on — never the current
        // href's. A label with no record at all (edited display text, or an
        // ordinal beyond the epoch's count) has no certified content to scan
        // for: Uncertified — unless a duplicate was deleted between epoch and
        // now, which leaves the pairing ambiguous.
        let label_records: Vec<&CertifiedLink> =
            certified.iter().filter(|c| label_match(c)).collect();
        if label_records.is_empty() {
            if label_deleted {
                // Pairing ambiguity: a duplicate with the same display text
                // was deleted since the epoch, so no record pairs with the
                // survivor by ordinal. Content identity resolves the pairing
                // first — the relocated-but-healthy carve-out (Change 1)
                // generalized to this case: if the current locator's content
                // equals ANY same-label candidate's certified block, the
                // survivor verifiably cites that block (unique content →
                // unique record; byte-identical candidates → outcome-
                // invariant), so the link is Healthy. Structural failures
                // still apply first; the move scan does not run here — the
                // fragment-match path already handles stale hrefs. With no
                // candidate match the pairing is genuinely unresolvable:
                // fail closed, reported distinctly from the multi-location
                // `Unknown` so the diagnostic can name the real problem.
                let Some(bytes) = target_bytes.as_deref() else {
                    return Ok((DriftOutcome::Broken, target_path));
                };
                if !extent_fits(bytes, link.start, link.end) {
                    return Ok((DriftOutcome::Broken, target_path));
                }
                for cand in certified.iter().filter(|c| c.label == link.label) {
                    if certified_content_fp(reader, repo_root, cache, page_path, anchor_sha, cand, &mut memo)?
                        == current_fp
                    {
                        return Ok((DriftOutcome::Healthy, target_path));
                    }
                }
                return Ok((DriftOutcome::UnknownLabelDeleted, target_path));
            }
            return Ok((DriftOutcome::Uncertified, target_path));
        }
        let cert = primary_cert(&label_records, &target_path, link.start, link.end);
        // Relocated-but-healthy carve-out: the content at the current locator
        // equals the certified content.
        if let Some(bytes) = target_bytes.as_deref()
            && extent_fits(bytes, link.start, link.end)
            && certified_content_fp(reader, repo_root, cache, page_path, anchor_sha, cert, &mut memo)?
                == current_fp
        {
            return Ok((DriftOutcome::Healthy, target_path));
        }
        // Move scan for the certified content. No match → Broken or
        // RangeDiffered per the rules below: a missing or overhanging target
        // is Broken; a present target whose locator was not at the epoch
        // (hand-edited href) is RangeDiffered.
        let zero = match &target_bytes {
            None => DriftOutcome::Broken,
            Some(b) if !extent_fits(b, link.start, link.end) => DriftOutcome::Broken,
            Some(_) => DriftOutcome::RangeDiffered,
        };
        let outcome = move_scan_outcome(
            repo_root,
            reader,
            cache,
            source,
            page_path,
            &target_path,
            target_bytes.as_deref(),
            cert,
            anchor_sha,
            &mut memo,
            zero,
            ctx,
        )?;
        return Ok((outcome, target_path));
    }

    let cert = primary_cert(&matched, &target_path, link.start, link.end);

    // Step 3: target present at the current side, extent still fitting.
    let Some(bytes) = target_bytes.as_deref() else {
        let outcome = move_scan_outcome(
            repo_root,
            reader,
            cache,
            source,
            page_path,
            &target_path,
            None,
            cert,
            anchor_sha,
            &mut memo,
            DriftOutcome::Broken,
            ctx,
        )?;
        return Ok((outcome, target_path));
    };
    if !extent_fits(bytes, link.start, link.end) {
        // Any part of the recorded range beyond the current line count is
        // Broken (round-1 finding 5): the content comparison is uncomputable.
        return Ok((DriftOutcome::Broken, target_path));
    }

    // Step 4: the href's range equals a certified range — the fingerprint
    // comparison decides.
    if matched
        .iter()
        .any(|c| c.start == link.start && c.end == link.end)
    {
        let cert_fp = certified_content_fp(reader, repo_root, cache, page_path, anchor_sha, cert, &mut memo)?;
        let cur_fp = cheap_fingerprint_with_extent(
            bytes,
            &Extent::LineRange {
                start: link.start,
                end: link.end,
            },
        );
        if cur_fp == cert_fp {
            return Ok((DriftOutcome::Healthy, target_path));
        }
        let outcome = move_scan_outcome(
            repo_root,
            reader,
            cache,
            source,
            page_path,
            &target_path,
            Some(bytes),
            cert,
            anchor_sha,
            &mut memo,
            DriftOutcome::Drift,
            ctx,
        )?;
        return Ok((outcome, target_path));
    }

    // Step 5: the href's range differs from every certified range (the href
    // was edited). Content equal to the certified content → Healthy (already
    // relocated); otherwise the move scan decides — a genuine relocation
    // wins, and everything else is RangeDiffered (bump), never Uncertified:
    // the link WAS reviewed at the epoch, and the as-written locator is an
    // edit the reviewer must ratify.
    let cur_fp = cheap_fingerprint_with_extent(
        bytes,
        &Extent::LineRange {
            start: link.start,
            end: link.end,
        },
    );
    for c in &matched {
        if certified_content_fp(reader, repo_root, cache, page_path, anchor_sha, c, &mut memo)? == cur_fp {
            return Ok((DriftOutcome::Healthy, target_path));
        }
    }
    let outcome = move_scan_outcome(
        repo_root,
        reader,
        cache,
        source,
        page_path,
        &target_path,
        Some(bytes),
        cert,
        anchor_sha,
        &mut memo,
        DriftOutcome::RangeDiffered,
        ctx,
    )?;
    Ok((outcome, target_path))
}

/// The matched certified link that anchors the comparison: prefer the one
/// whose path and range equal the current link's, then the one whose range
/// equals it, then document order.
fn primary_cert<'a>(
    matched: &[&'a CertifiedLink],
    target_path: &str,
    start: u32,
    end: u32,
) -> &'a CertifiedLink {
    matched
        .iter()
        .find(|c| c.target_path == target_path && c.start == start && c.end == end)
        .or_else(|| matched.iter().find(|c| c.start == start && c.end == end))
        .copied()
        .unwrap_or(matched[0])
}

/// The canonical rk64 fingerprint of a certified link's target range at the
/// anchor commit, memoized per (path, range).
///
/// The Phase 2 fingerprint-tier seam (plan decision 5): the disk tier wraps
/// this function itself, so all five call sites (four in `classify_link`,
/// one in `move_scan_outcome`) engage it identically. Read: derive the key
/// from the full queried tuple → lookup → serve only a verified row (the
/// store re-derives the tuple and `row_digest` itself; any miss or error is
/// a miss). The in-memory memo sits under the tier — checked only on a disk
/// miss — so cross-link dedup on a page exists only via the disk tier.
/// Write: compute strictly outside any cache transaction, then upsert on
/// the miss path; a failed anchor read (`None` — absent path, unreadable
/// blob, or unreadable tree, all exit 128) is probed per the three-valued
/// availability rule (plan decision 1): present ⇒ no write (fail open — a
/// row written while the blob was unreadable is never served), absent ⇒
/// `fp = "0000000000000000"` is cached (an absent target at the anchor is a
/// defined input), unknown ⇒ no write. The return value and error
/// propagation are byte-identical to the uncached computation in every
/// branch; a cache error is never a serve and never a failure.
fn certified_content_fp(
    reader: &crate::git::GitReader,
    repo_root: &Path,
    cache: &dyn crate::cache::AnchorCache,
    page_path: &str,
    anchor_sha: &str,
    cert: &CertifiedLink,
    memo: &mut HashMap<(String, u32, u32), u64>,
) -> Result<u64, EpochError> {
    let key = (cert.target_path.clone(), cert.start, cert.end);
    let cache_key = crate::cache::key::fingerprint_key(
        page_path,
        anchor_sha,
        &cert.target_path,
        cert.start,
        cert.end,
    );
    if let Some(fp_hex) = cache
        .lookup_fingerprint(
            &cache_key,
            page_path,
            anchor_sha,
            &cert.target_path,
            cert.start,
            cert.end,
        )
        .unwrap_or(None)
        && let Some(fp) = crate::rk64::rk64_from_hex(&fp_hex)
    {
        crate::perf::anchor_cache_hit();
        return Ok(fp);
    }
    crate::perf::anchor_cache_miss();
    if let Some(&fp) = memo.get(&key) {
        return Ok(fp);
    }

    // The tier's git leg — it runs only here, never on a served hit, and its
    // duration is tallied into the run's aggregated `anchor_cache` event, so
    // a warm run reports zero fingerprint milliseconds.
    let fp_start = Instant::now();
    let fp = match read_blob_at(reader, repo_root, anchor_sha, &cert.target_path)? {
        None => {
            // The read failed; the probe decides the write.
            if availability_probe(repo_root, anchor_sha, &cert.target_path) == Availability::Absent {
                let _ = cache.upsert_fingerprint(
                    &cache_key,
                    page_path,
                    anchor_sha,
                    &cert.target_path,
                    cert.start,
                    cert.end,
                    "0000000000000000",
                );
            }
            0 // no content at all
        }
        Some(bytes) => {
            let fp = cheap_fingerprint_with_extent(
                &bytes,
                &Extent::LineRange {
                    start: cert.start,
                    end: cert.end,
                },
            );
            let _ = cache.upsert_fingerprint(
                &cache_key,
                page_path,
                anchor_sha,
                &cert.target_path,
                cert.start,
                cert.end,
                &crate::rk64::rk64_to_hex(fp),
            );
            fp
        }
    };
    crate::perf::anchor_cache_add_leg("fingerprint", fp_start.elapsed().as_nanos() as u64);
    memo.insert(key, fp);
    Ok(fp)
}

/// The canonical rk64 fingerprint of the current link's target range from
/// bytes already read at the current side; `0` when the target is absent
/// (no content at all).
fn current_content_fp(bytes: Option<&[u8]>, start: u32, end: u32) -> u64 {
    match bytes {
        None => 0,
        Some(bytes) => cheap_fingerprint_with_extent(bytes, &Extent::LineRange { start, end }),
    }
}

/// The move scan, both tiers. Exact first: find the certified content as a
/// contiguous window, same file first, then every other candidate file in the
/// repo — one match → `Moved`; ≥2 → `Unknown` (the card's multi-match rule is
/// unconditional — never first-hit-wins). Zero exact matches falls to the
/// fuzzy Jaccard tier (Decision 5 step 4): one at-threshold window → `Moved`,
/// ≥2 → `Unknown`, none → `zero_matches` (the caller's own terminal outcome,
/// `Broken` for a missing target, `Drift` for range-equal content drift).
#[allow(clippy::too_many_arguments)]
fn move_scan_outcome(
    repo_root: &Path,
    reader: &crate::git::GitReader,
    cache: &dyn crate::cache::AnchorCache,
    source: DocSource,
    page_path: &str,
    target_path: &str,
    target_bytes: Option<&[u8]>,
    cert: &CertifiedLink,
    anchor_sha: &str,
    memo: &mut HashMap<(String, u32, u32), u64>,
    zero_matches: DriftOutcome,
    ctx: &mut MoveScanCtx<'_>,
) -> Result<DriftOutcome, EpochError> {
    let key = MoveScanKey {
        anchor_sha: anchor_sha.to_string(),
        page_path: page_path.to_string(),
        target_path: cert.target_path.clone(),
        start: cert.start,
        end: cert.end,
    };
    if let Some(decision) = ctx.decisions.get(&key) {
        return Ok(decision.clone().into_outcome(zero_matches));
    }
    let decision = compute_move_scan(
        repo_root,
        reader,
        cache,
        source,
        page_path,
        target_path,
        target_bytes,
        cert,
        anchor_sha,
        memo,
        ctx,
    )?;
    ctx.decisions.insert(key, decision.clone());
    Ok(decision.into_outcome(zero_matches))
}

/// The uncached move scan behind [`move_scan_outcome`]: run both tiers and
/// return the verdict the caller replays for every citation of this block.
#[allow(clippy::too_many_arguments)]
fn compute_move_scan(
    repo_root: &Path,
    reader: &crate::git::GitReader,
    cache: &dyn crate::cache::AnchorCache,
    source: DocSource,
    page_path: &str,
    target_path: &str,
    target_bytes: Option<&[u8]>,
    cert: &CertifiedLink,
    anchor_sha: &str,
    memo: &mut HashMap<(String, u32, u32), u64>,
    ctx: &mut MoveScanCtx<'_>,
) -> Result<MoveScanDecision, EpochError> {
    let span = line_range_span(cert.start, cert.end);
    if span == 0 {
        // Degenerate certified content never matches a window.
        return Ok(MoveScanDecision::Unmatched);
    }
    let cert_fp =
        certified_content_fp(reader, repo_root, cache, page_path, anchor_sha, cert, memo)?;
    let extent = Extent::LineRange {
        start: 1,
        end: span as u32,
    };

    // The certified window — the certified content at its own epoch
    // coordinates — is never a relocation target in any tier: a match there
    // means the content drifted in place (or a hand-edited href points
    // elsewhere while the certified block never moved), and "relocating" to
    // it rewrites the href to itself on every run. The current link's own
    // coordinates are NOT excluded: the exact tiers cannot match them (the
    // scan only runs when the content there differs from the certified
    // block), and the fuzzy tier must be able to match them — the edited
    // relocation's match lives at the current coordinates.
    let not_certified_window = |l: &crate::rk64::Location| {
        !(l.path == cert.target_path && l.start_line == cert.start && l.end_line == cert.end)
    };

    // Same-file tier: the link's own target.
    let same_file = match target_bytes {
        Some(bytes) => {
            let idx = LineIndex::build(bytes);
            let matches: Vec<crate::rk64::Location> =
                scan_indexed_rk64(&[(target_path.to_string(), idx)], cert_fp, extent, None)
                    .into_iter()
                    .filter(not_certified_window)
                    .collect();
            match matches.len() {
                1 => Some(moved_to(&matches[0], true)),
                n if n >= 2 => Some(MoveScanDecision::Ambiguous),
                _ => None,
            }
        }
        None => None,
    };
    if let Some(decision) = same_file {
        return Ok(decision);
    }

    // Cross-file tier: every other candidate file in the repo. The page
    // itself is excluded — a body quoting the range must not relocate the
    // link into itself — as is the target (already scanned above). Only
    // identity-evidenced destinations count (amendment Change 2): a
    // content-only match in an unrelated file is a quote or a copy, never a
    // move, and is dropped rather than counted toward Unknown.
    let others: Vec<&CandidateFile> = ctx
        .candidates(reader, repo_root, source)?
        .iter()
        .filter(|c| c.path != target_path && c.path != page_path)
        .collect();
    // The exact tier: the certified fingerprint, byte for byte, in every
    // candidate file.
    let scanned: Vec<crate::rk64::Location> = parallel_scan(&others, |candidate, out| {
        scan_one_indexed(&candidate.path, &candidate.index, cert_fp, extent, out);
    })
    .into_iter()
    .filter(not_certified_window)
    .collect();
    // The identity-evidence pass (dest-history walks) over the exact tier's
    // raw matches, warmed on the pool before the serial loop consumes it.
    ctx.run
        .prewarm_dest_histories(repo_root, scanned.iter().map(|l| l.path.as_str()))?;
    let mut matches: Vec<crate::rk64::Location> = Vec::new();
    for l in scanned {
        if has_identity_evidence(ctx, repo_root, &cert.target_path, &l.path)? {
            matches.push(l);
        }
    }
    match matches.len() {
        1 => return Ok(moved_to(&matches[0], true)),
        n if n >= 2 => return Ok(MoveScanDecision::Ambiguous),
        _ => {}
    }

    // Fuzzy tier: the certified content is absent everywhere in exact form.
    // Look for a lightly-edited near-copy (Decision 5 step 4): exactly one
    // at-threshold window → Moved; ≥2 → Unknown; none → `zero_matches`.
    // Same-file windows need no identity evidence; cross-file windows do.
    let mut fuzzy: Vec<crate::rk64::Location> = Vec::new();
    let fuzzy_scanned = fuzzy_locations(
        repo_root,
        reader,
        source,
        page_path,
        target_path,
        target_bytes,
        cert,
        anchor_sha,
        ctx,
    )?;
    let fuzzy_scanned: Vec<crate::rk64::Location> = fuzzy_scanned
        .into_iter()
        .filter(not_certified_window)
        .collect();
    // Same-file windows are evidence-free, so only the cross-file ones ask
    // about a destination's history.
    ctx.run.prewarm_dest_histories(
        repo_root,
        fuzzy_scanned
            .iter()
            .filter(|l| l.path != target_path)
            .map(|l| l.path.as_str()),
    )?;
    for l in fuzzy_scanned {
        if l.path != target_path
            && !has_identity_evidence(ctx, repo_root, &cert.target_path, &l.path)?
        {
            continue;
        }
        fuzzy.push(l);
    }
    match fuzzy.len() {
        1 => Ok(moved_to(&fuzzy[0], false)),
        n if n >= 2 => Ok(MoveScanDecision::Ambiguous),
        _ => Ok(MoveScanDecision::Unmatched),
    }
}

/// The fuzzy tier's at-threshold for multiset-Jaccard window similarity.
/// Tuned against real wiki markdown and code excerpts in the P2 acceptance
/// test `fuzzy_threshold_separates_real_content`, not inherited from
/// git-span's 0.95/0.50 pair: a window reaches the tier when its
/// [`fuzzy_window_score`] with the certified content is at least this value.
const FUZZY_JACCARD_THRESHOLD: f64 = 0.7;

/// The per-line match threshold inside [`fuzzy_window_score`]'s containment
/// weighting: a window line counts as matched when its token multiset
/// Jaccard with at least one certified line reaches this value. Low enough
/// that a moved block's edited line still counts as the block's own (a
/// replaced line typically shares ~0.5–0.6 of its tokens), high enough that
/// filler lines sharing a couple of tokens do not.
const FUZZY_LINE_MATCH_THRESHOLD: f64 = 0.5;

/// Multiset Jaccard similarity between two line groups: tokenize every line
/// on whitespace, count duplicate tokens on both sides, and divide the
/// intersection size by the union size. `0.0` for two empty groups.
///
/// The decoded reference form of what the collector computes over byte keys
/// and index facts ([`CertBlock::window_token_jaccard`]), kept as the oracle
/// the collector's differential tests are written against.
#[cfg(test)]
fn window_jaccard(a_lines: &[&str], b_lines: &[&str]) -> f64 {
    let a = token_counts(a_lines);
    let b = token_counts(b_lines);
    let a_total: u64 = a.values().map(|&c| u64::from(c)).sum();
    let b_total: u64 = b.values().map(|&c| u64::from(c)).sum();
    if a_total == 0 && b_total == 0 {
        return 0.0;
    }
    let inter: u64 = a
        .iter()
        .map(|(t, &ca)| {
            b.get(t)
                .map(|&cb| u64::from(ca.min(cb)))
                .unwrap_or(0)
        })
        .sum();
    let union = a_total + b_total - inter;
    inter as f64 / union as f64
}

/// Token multisets of a line group, keyed by whitespace token.
fn token_counts<'a>(lines: &[&'a str]) -> HashMap<&'a str, u32> {
    let mut counts: HashMap<&str, u32> = HashMap::new();
    for line in lines {
        for token in line.split_whitespace() {
            *counts.entry(token).or_insert(0) += 1;
        }
    }
    counts
}

/// The fuzzy-tier window score: token multiset Jaccard
/// ([`window_jaccard`]) weighted by line containment — the line-level
/// Jaccard `matched / (|a| + |b| − matched)`, where a window line counts as
/// matched when its token multiset Jaccard with at least one certified line
/// reaches [`FUZZY_LINE_MATCH_THRESHOLD`] (blank lines match blanks). The
/// weighting keeps sliding windows that merely *overlap* a moved block (high
/// token similarity plus foreign filler lines) below the tier threshold:
/// only a window made of the moved block's own lines scores at or above it.
///
/// `collect_fuzzy_hits` scores windows through the same formula with a
/// prefilter, so this direct form is now the test-side oracle — the
/// brute-force reference the optimized collector must agree with.
#[cfg(test)]
fn fuzzy_window_score(a_lines: &[&str], b_lines: &[&str]) -> f64 {
    let token = window_jaccard(a_lines, b_lines);
    if token == 0.0 {
        return 0.0;
    }
    let mut matched = 0usize;
    for b_line in b_lines {
        let b_blank = b_line.split_whitespace().next().is_none();
        let hit = a_lines.iter().any(|a_line| {
            let a_blank = a_line.split_whitespace().next().is_none();
            (a_blank && b_blank)
                || (!a_blank
                    && !b_blank
                    && window_jaccard(&[a_line], &[b_line]) >= FUZZY_LINE_MATCH_THRESHOLD)
        });
        if hit {
            matched += 1;
        }
    }
    if matched == 0 {
        return 0.0;
    }
    let union = a_lines.len() + b_lines.len() - matched;
    token * matched as f64 / union as f64
}

/// The fuzzy-tier move scan: every window of the certified span's height in
/// every candidate file (the link's own target first, then the cross-file
/// candidate set, page excluded) whose [`fuzzy_window_score`] with the
/// certified content reaches [`FUZZY_JACCARD_THRESHOLD`]. Callers turn the
/// list into Moved / Unknown / zero-matches per Decision 5 step 4.
#[allow(clippy::too_many_arguments)]
fn fuzzy_locations(
    repo_root: &Path,
    reader: &crate::git::GitReader,
    source: DocSource,
    page_path: &str,
    target_path: &str,
    target_bytes: Option<&[u8]>,
    cert: &CertifiedLink,
    anchor_sha: &str,
    ctx: &mut MoveScanCtx<'_>,
) -> Result<Vec<crate::rk64::Location>, EpochError> {
    let span = line_range_span(cert.start, cert.end);
    if span == 0 {
        return Ok(Vec::new());
    }
    // The certified lines at the anchor commit, clamped like the kernel's
    // canonical content. The range was valid when certified, so the clamp
    // only fires on a corrupt or hand-edited anchor file.
    let Some(blob) = read_blob_at(reader, repo_root, anchor_sha, &cert.target_path)? else {
        return Ok(Vec::new());
    };
    let text = String::from_utf8_lossy(&blob);
    let lines: Vec<&str> = text.lines().collect();
    let start = cert.start as usize;
    if start == 0 || start > lines.len() {
        return Ok(Vec::new());
    }
    let end = (cert.end as usize).min(lines.len());
    if end < start {
        return Ok(Vec::new());
    }
    let cert_lines = &lines[start - 1..end];

    // Certified-line facts, computed once and shared by every candidate file.
    let block = CertBlock::of(cert_lines);

    let mut hits = Vec::new();
    // Same-file tier: the link's own target.
    if let Some(bytes) = target_bytes {
        collect_fuzzy_hits(
            bytes,
            &FuzzyLines::build(bytes),
            target_path,
            &block,
            span,
            &mut hits,
        );
    }
    // Cross-file tier: every candidate minus the target and the page itself.
    let others: Vec<&CandidateFile> = ctx
        .candidates(reader, repo_root, source)?
        .iter()
        .filter(|c| c.path != target_path && c.path != page_path)
        .collect();
    hits.extend(parallel_scan(&others, |candidate, out| {
        collect_fuzzy_hits(
            &candidate.bytes,
            &candidate.fuzzy,
            &candidate.path,
            &block,
            span,
            out,
        );
    }));
    Ok(hits)
}

/// One candidate file's fuzzy-tier line index: every line's raw byte span, its
/// tokens' byte ranges, and its token count — built once per inventory
/// ([`CandidateFile`]) and reused by every link's scan.
///
/// The collector needs a candidate line's token count *before* it needs the
/// line's text: token-Jaccard never exceeds `min(|a|,|b|)/max(|a|,|b|)`, so a
/// line can only match a certified line whose token count lies in the window
/// `[T·a, a/T]`, and a line outside every certified line's window is rejected
/// without decoding it or splitting it into tokens. On a corpus whose bulk is
/// media that is nearly every line of nearly every file, and decoding a line
/// of binary — three bytes of replacement character per invalid byte — is what
/// the tier cannot afford to do per link.
///
/// The tokens' ranges are indexed here for the same reason: tokenizing a line
/// is a property of the file's bytes, but the tier's scans re-ask it of every
/// line of every file for every link that drifts, and the asking is a
/// separator test per *byte* — on a repo-sized corpus, gigabytes of byte
/// tests per run, to re-derive what one pass over the file already knew.
///
/// Both facts are raw-byte facts. The lossy decode copies valid runs through
/// unchanged and inserts replacement characters for invalid ones, and a
/// replacement character is not whitespace; so the decoded text's whitespace
/// characters are exactly the raw bytes' separators — the ASCII whitespace
/// bytes plus the UTF-8 encodings of the non-ASCII `White_Space` characters —
/// and a token's bytes are a maximal run between them. Counting those runs
/// therefore counts the decoded line's tokens, and slicing between them
/// yields the line the decoder would have produced.
#[derive(Default)]
struct FuzzyLines {
    /// Raw `[start, end)` of each line's content, `\n` and a preceding `\r`
    /// excluded — the ranges [`str::lines`] yields.
    spans: Vec<(u32, u32)>,
    /// Token count of each line, per [`str::split_whitespace`].
    tokens: Vec<u32>,
    /// Every line's tokens, concatenated: line `i`'s are
    /// `runs[starts[i]..starts[i + 1]]`, as `[start, end)` ranges into the
    /// line's own bytes ([`Self::line`]).
    runs: Vec<(u32, u32)>,
    /// Where each line's tokens begin in `runs`; `line_count() + 1` entries,
    /// so the last line's slice has an end.
    starts: Vec<u32>,
}

impl FuzzyLines {
    /// Index `bytes` with one forward scan, recording each line's tokens as it
    /// closes.
    fn build(bytes: &[u8]) -> FuzzyLines {
        // The same contract as the exact tier's index: u32 spans mean a 4 GiB
        // buffer is refused, not silently truncated.
        assert!(
            bytes.len() <= u32::MAX as usize,
            "fuzzy: buffer of {} bytes exceeds the supported size of {} bytes \
             (line spans are stored as u32); files of 4 GiB or larger are not indexable",
            bytes.len(),
            u32::MAX,
        );
        let mut lines = FuzzyLines::default();
        let mut start = 0usize;
        for (i, &b) in bytes.iter().enumerate() {
            if b == b'\n' {
                lines.push_line(bytes, start, i);
                start = i + 1;
            }
        }
        // A trailing segment with no terminating newline is the final,
        // unterminated line; a buffer ending in `\n` has none (matching
        // `str::lines`).
        if start < bytes.len() {
            lines.push_line(bytes, start, bytes.len());
        }
        lines.starts.push(lines.runs.len() as u32);
        lines
    }

    fn push_line(&mut self, bytes: &[u8], start: usize, end: usize) {
        let end = if end > start && bytes[end - 1] == b'\r' {
            end - 1
        } else {
            end
        };
        self.spans.push((start as u32, end as u32));
        self.starts.push(self.runs.len() as u32);
        for (from, to) in Runs::new(&bytes[start..end]) {
            self.runs.push((from as u32, to as u32));
        }
        self.tokens.push(self.runs.len() as u32 - self.starts[self.starts.len() - 1]);
    }

    /// Number of lines, per [`str::lines`] counting.
    fn line_count(&self) -> usize {
        self.spans.len()
    }

    /// Line `i`'s raw content bytes.
    fn line<'a>(&self, bytes: &'a [u8], i: usize) -> &'a [u8] {
        let (start, end) = self.spans[i];
        &bytes[start as usize..end as usize]
    }

    /// Line `i`'s tokens, as `[start, end)` ranges into the indexed bytes.
    fn line_runs(&self, i: usize) -> &[(u32, u32)] {
        &self.runs[self.starts[i] as usize..self.starts[i + 1] as usize]
    }
}

/// The decoded line's token count, taken from the raw bytes: the number of
/// maximal runs between separators ([`Runs`]).
#[cfg(test)]
fn token_count(bytes: &[u8]) -> u32 {
    Runs::new(bytes).count() as u32
}

/// The maximal non-separator runs of a byte string, as offsets — one line's
/// tokens, without decoding it. One tokenizer for the whole tier: the line
/// index counts runs with it, the matcher compares them.
struct Runs<'a> {
    bytes: &'a [u8],
    i: usize,
}

impl<'a> Runs<'a> {
    fn new(bytes: &'a [u8]) -> Runs<'a> {
        Runs { bytes, i: 0 }
    }
}

impl Iterator for Runs<'_> {
    type Item = (usize, usize);

    fn next(&mut self) -> Option<(usize, usize)> {
        let bytes = self.bytes;
        while self.i < bytes.len() {
            let separator = separator_len(bytes, self.i);
            if separator == 0 {
                break;
            }
            self.i += separator;
        }
        if self.i == bytes.len() {
            return None;
        }
        let start = self.i;
        while self.i < bytes.len() && separator_len(bytes, self.i) == 0 {
            self.i += 1;
        }
        Some((start, self.i))
    }
}

/// The length of the whitespace sequence at `bytes[i]`, or `0` when the byte
/// there is not whitespace — [`str::split_whitespace`]'s definition, read at
/// byte level.
///
/// Only the ASCII whitespace bytes and the UTF-8 encodings of the non-ASCII
/// `White_Space` characters separate tokens, and the raw scan and the decoded
/// text agree on every one of them. Error recovery never rewrites a whitespace
/// character: a whitespace encoding always begins with `C2`, `E1`, `E2` or
/// `E3`, and a byte above `0xBF` is never a continuation byte, so such a byte
/// is always the start of the sequence the decoder reads there; the decoder
/// resumes exactly at a byte the raw scan would test; and where the raw scan
/// reads a whitespace sequence the decoder reads that same character, since
/// the encodings are the canonical ones.
fn separator_len(bytes: &[u8], i: usize) -> usize {
    let b = bytes[i];
    if b < 0x80 {
        return usize::from(b.is_ascii_whitespace());
    }
    let rest = &bytes[i..];
    if rest.starts_with("\u{85}".as_bytes()) || rest.starts_with("\u{a0}".as_bytes()) {
        return 2;
    }
    if rest.starts_with("\u{1680}".as_bytes())
        || rest.starts_with("\u{202f}".as_bytes())
        || rest.starts_with("\u{205f}".as_bytes())
        || rest.starts_with("\u{3000}".as_bytes())
        || (rest.starts_with("\u{2000}".as_bytes()[..2].as_ref())
            && rest
                .get(2)
                .is_some_and(|&third| matches!(third, 0x80..=0x8a | 0xa8 | 0xa9)))
    {
        return 3;
    }
    0
}

/// The replacement character's UTF-8 encoding: what the decoder writes for an
/// invalid sequence.
const FFFD: &[u8] = "\u{fffd}".as_bytes();

/// The certified block's fuzzy-tier facts, resolved once per scan and shared
/// by every candidate file: its lines as the anchor-commit decoder produced
/// them, their token multisets, the token → line index, the token filter, the
/// count bounds a candidate line must land in, and the block's whole token
/// multiset with its total.
///
/// One bundle because every candidate line's judgment reads all of it: the
/// count bounds decide whether the line is read at all, the filter and index
/// decide which of its tokens matter, and the multisets decide the match —
/// the per-line ones for the containment flags, the whole-block one for the
/// window score, which would otherwise rebuild the same fixed side once per
/// scored window.
struct CertBlock<'a> {
    data: Vec<CertLine<'a>>,
    index: HashMap<&'a [u8], Vec<u32>>,
    fps: CertFingerprints,
    counts: CertCounts,
    /// Every token of every line, counted — the fixed side of the window
    /// score's token-Jaccard, keyed by bytes like the candidate side.
    multiset: HashMap<&'a [u8], u32>,
    /// Total tokens over the block's lines, the fixed side of the score's
    /// union and of the token bound a window is screened against.
    token_total: u64,
}

impl<'a> CertBlock<'a> {
    fn of(lines: &[&'a str]) -> CertBlock<'a> {
        let data: Vec<CertLine<'a>> = lines.iter().map(|l| CertLine::new(l)).collect();
        let index = cert_token_index(&data);
        let mut multiset: HashMap<&[u8], u32> = HashMap::new();
        for cert in &data {
            for (token, &count) in &cert.counts {
                *multiset.entry(token).or_insert(0) += count;
            }
        }
        let token_total = data.iter().map(|cert| u64::from(cert.total)).sum();
        CertBlock {
            counts: CertCounts::of(&data),
            fps: CertFingerprints::of(&index),
            index,
            multiset,
            token_total,
            data,
        }
    }

    /// The fuzzy-tier score of a window of candidate lines, given the window's
    /// tokens as a multiset keyed by bytes and its token total: token
    /// multiset Jaccard ([`window_jaccard`]'s formula, over the byte keys both
    /// sides share) times the caller's line containment.
    ///
    /// The block's side of the Jaccard is [`Self::multiset`] and
    /// [`Self::token_total`], computed once per scan; the candidate side is
    /// built from the line index's token ranges, so no line is ever decoded.
    /// The comparison never leaves integers: the intersection is a sum of
    /// `min` counts and the totals are the index's own, so the quotient is the
    /// same float the decoded reference formula produces.
    fn window_token_jaccard(&self, window: &HashMap<&[u8], u32>, window_total: u64) -> f64 {
        if self.token_total == 0 && window_total == 0 {
            return 0.0;
        }
        let inter: u64 = self
            .multiset
            .iter()
            .map(|(token, &a)| {
                window
                    .get(*token)
                    .map_or(0, |&b| u64::from(a.min(b)))
            })
            .sum();
        let union = self.token_total + window_total - inter;
        inter as f64 / union as f64
    }
}

/// The certified block's token counts, as the bounds a candidate line's count
/// must reach to match one of its lines: `min` and `max` over the non-blank
/// lines, whether the block has a blank line at all (a blank candidate line
/// matches only a blank certified line), and whether any of its tokens carries
/// a replacement character.
///
/// That last fact is what decides whether the pass may compare candidate runs
/// to certified tokens by their bytes: a token the decoder produced out of
/// invalid bytes has no byte-for-byte counterpart in the candidate, so a
/// block that has one is read through a decode (see [`collect_fuzzy_hits`]).
struct CertCounts {
    min: u32,
    max: u32,
    any_blank: bool,
    any_fffd: bool,
}

impl CertCounts {
    fn of(cert_data: &[CertLine]) -> CertCounts {
        let mut counts = CertCounts {
            min: u32::MAX,
            max: 0,
            any_blank: false,
            any_fffd: false,
        };
        for cert in cert_data {
            counts.any_fffd |= cert
                .counts
                .keys()
                .any(|token| token.windows(FFFD.len()).any(|w| w == FFFD));
            if cert.blank {
                counts.any_blank = true;
                continue;
            }
            counts.min = counts.min.min(cert.total);
            counts.max = counts.max.max(cert.total);
        }
        counts
    }

    /// Whether a candidate line of `tokens` tokens can match any non-blank
    /// certified line. Necessary, not sufficient: token-Jaccard never exceeds
    /// `min/max`, so a match requires the certified count and the candidate
    /// count to lie within a factor of `1/T` of one another. One token of
    /// slack on each side — over-admitting only costs the decode this exists
    /// to save, while under-admitting would lose a match.
    fn could_match(&self, tokens: u32) -> bool {
        if self.max == 0 {
            return false;
        }
        let count = f64::from(tokens);
        let threshold = FUZZY_LINE_MATCH_THRESHOLD;
        count * threshold <= f64::from(self.max) + 1.0
            && count / threshold + 1.0 >= f64::from(self.min)
    }
}

/// One certified line's fuzzy-tier facts: its token multiset, token count,
/// and whether it is blank.
///
/// Tokens are keyed by their bytes, not by `&str`: the candidate side compares
/// the bytes of a run (see [`collect_fuzzy_hits`]), and a token's bytes *are*
/// its identity in the decoded text, so one map serves both sides.
struct CertLine<'a> {
    counts: HashMap<&'a [u8], u32>,
    total: u32,
    blank: bool,
}

impl<'a> CertLine<'a> {
    fn new(line: &'a str) -> Self {
        let counts = token_counts(&[line])
            .into_iter()
            .map(|(token, count)| (token.as_bytes(), count))
            .collect();
        let total = line.split_whitespace().count() as u32;
        let blank = line.split_whitespace().next().is_none();
        Self { counts, total, blank }
    }
}

/// Token → indices of the certified lines containing it, for the containment
/// prefilter: a candidate line can only match certified lines that share a
/// token with it.
fn cert_token_index<'a>(cert_data: &[CertLine<'a>]) -> HashMap<&'a [u8], Vec<u32>> {
    let mut index: HashMap<&[u8], Vec<u32>> = HashMap::new();
    for (i, cert) in cert_data.iter().enumerate() {
        for token in cert.counts.keys() {
            index.entry(token).or_default().push(i as u32);
        }
    }
    index
}

/// A 1024-bit filter over the certified block's token fingerprints: the step
/// that keeps the matcher's byte-keyed lookups — and the whole multiset
/// intersection behind them — to the candidate lines that can possibly share
/// a token with the block at all.
///
/// It is a filter, never an authority: a token is looked up in
/// [`cert_token_index`] only when its fingerprint is present here, and every
/// certified token's fingerprint is present by construction, so a line is
/// never rejected on a token the block does contain. Fingerprint collisions
/// only cost the lookup they fail to save, which is why the fingerprint may
/// be sampled from a token's length and a few of its bytes rather than hashed
/// over all of them: on a corpus whose bulk is media, a line's tokens are
/// long binary runs, and hashing every byte of them to answer "could this
/// line share a token with the block?" is the cost this exists to avoid.
struct CertFingerprints {
    bits: [u64; Self::WORDS],
}

impl CertFingerprints {
    const BITS: u32 = 1024;
    const WORDS: usize = (Self::BITS / 64) as usize;

    fn of(index: &HashMap<&[u8], Vec<u32>>) -> Self {
        let mut fps = CertFingerprints {
            bits: [0; Self::WORDS],
        };
        for token in index.keys() {
            let fp = token_fingerprint(token) % Self::BITS;
            fps.bits[(fp / 64) as usize] |= 1u64 << (fp % 64);
        }
        fps
    }

    fn may_contain(&self, token: &[u8]) -> bool {
        let fp = token_fingerprint(token) % Self::BITS;
        self.bits[(fp / 64) as usize] & (1u64 << (fp % 64)) != 0
    }
}

/// A token's fingerprint, for [`CertFingerprints`]: its length and three
/// sampled bytes, mixed. Sampling is enough because a collision only sends
/// the token to the authoritative lookup, which decides on its bytes.
fn token_fingerprint(token: &[u8]) -> u32 {
    let last = token.len() - 1;
    let mut h = (token.len() as u32).wrapping_mul(0x9e37_79b1);
    h ^= u32::from(token[0]).wrapping_mul(0x85eb_ca6b);
    h ^= u32::from(token[last]).wrapping_mul(0xc2b2_ae35);
    if last > 0 {
        h ^= u32::from(token[last / 2]).wrapping_mul(0x27d4_eb2f);
    }
    h
}

/// Slide a window of the certified span's height over one candidate file and
/// collect every at-threshold location.
///
/// Naively, every window position re-tokenizes the whole window and compares
/// every window line against every certified line — O(lines × span²) work per
/// candidate, which hangs a real repo the moment one link drifts (the
/// corpus's first genuine Drift link scans ~220k candidate lines). The
/// observations that make the same judgment cheap are layered, each letting
/// the next run on fewer lines:
///
/// 1. Line containment (does this line match *any* certified line?) is a
///    per-line fact, so a window's matched count is a sliding sum over
///    per-file flags, not a per-window recomputation.
/// 2. A line can only match a certified line whose token count lies within a
///    factor of `T` of its own (token-Jaccard never exceeds `min/max`), and
///    the counts are in the index, so most lines are rejected before their
///    bytes are read.
/// 3. A surviving line is compared as runs of its bytes against certified
///    tokens keyed by bytes, so it is never decoded ([`line_matches_any_cert`]),
///    and only the runs the certified block could contain are looked up at all
///    ([`CertFingerprints`]) — on a corpus of media and prose, none of them.
///
/// And the score is token-Jaccard × containment, where containment ≥ T is a
/// *necessary* condition for the score to reach FUZZY_JACCARD_THRESHOLD
/// (token-Jaccard never exceeds 1). The matched-count floor is derived from
/// the same threshold constant, so no window the old formula could admit is
/// ever skipped — only windows below the floor, which the old formula
/// provably scores below the threshold too. The same argument caps the score
/// by `min/max` over the two token totals — the second number both sides of a
/// Jaccard admit, and both totals are index facts — so a second screen rejects
/// windows whose similarity *cannot* reach the threshold before any token of
/// them is read.
///
/// Only the rare windows that clear both screens pay for a multiset, and none
/// of them is decoded: the score's tokens are the line index's byte ranges,
/// compared against the block's own byte-keyed multiset ([`CertBlock`]). The
/// one exception is a block whose own tokens carry replacement characters,
/// which no candidate's raw bytes can match and which is therefore compared
/// through a decode.
fn collect_fuzzy_hits(
    bytes: &[u8],
    lines: &FuzzyLines,
    path: &str,
    block: &CertBlock<'_>,
    span: usize,
    hits: &mut Vec<crate::rk64::Location>,
) {
    if lines.line_count() < span {
        return;
    }

    // Per-line containment flags, computed once per file, with one scratch
    // that borrows nothing ([`Runs`]) so no line has to become a `String` for
    // the comparison.
    let mut runs: Vec<(u32, u32, u32)> = Vec::new();
    let mut flags: Vec<bool> = Vec::with_capacity(lines.line_count());
    for i in 0..lines.line_count() {
        flags.push(line_flag(bytes, lines, i, block, &mut runs));
    }

    // Matched-count floor: containment ≥ T requires matched ≥ 2·span·T/(1+T).
    // Windows below the floor cannot reach FUZZY_JACCARD_THRESHOLD no matter
    // their token similarity.
    let floor = ((2.0 * span as f64 * FUZZY_JACCARD_THRESHOLD)
        / (1.0 + FUZZY_JACCARD_THRESHOLD))
        .ceil() as usize;

    let mut matched = flags[..span].iter().filter(|&&f| f).count();
    // The window's token total slides with it, from the index's per-line
    // counts: token-Jaccard never exceeds `min/max` over the two totals, so
    // this one number — free, already in the index — screens a window before
    // any of its tokens are read.
    let mut window_tokens: u64 = lines.tokens[..span].iter().map(|&t| u64::from(t)).sum();
    // The scored window's tokens as a multiset, keyed by bytes as the block's
    // is. Rebuilt per scored window rather than slid line by line: the windows
    // that reach the score are few, and a slide would have to keep per-line
    // token maps alive for every line the window passes over. Kept across
    // windows for its capacity; only the raw path — every block but a
    // replacement-carrying one — borrows from this one.
    let mut window: HashMap<&[u8], u32> = HashMap::new();
    for start in 0..=lines.line_count() - span {
        if start > 0 {
            if flags[start - 1] {
                matched -= 1;
            }
            if flags[start + span - 1] {
                matched += 1;
            }
            window_tokens -= u64::from(lines.tokens[start - 1]);
            window_tokens += u64::from(lines.tokens[start + span - 1]);
        }
        if matched < floor {
            continue;
        }
        let containment = matched as f64 / (2.0 * span as f64 - matched as f64);
        // Token-Jaccard never exceeds `min/max` of the two totals, so the score
        // is capped by `(min/max)·containment` — and both totals are known
        // before a token is read. A window whose cap falls short of the
        // threshold cannot reach it however similar its tokens are, and the
        // two floats of the cap and the score are the same expression over the
        // same containment, so the cap is an exact upper bound, not a
        // heuristic one.
        let (lo, hi) = if window_tokens < block.token_total {
            (window_tokens, block.token_total)
        } else {
            (block.token_total, window_tokens)
        };
        if hi == 0 || (lo as f64 / hi as f64) * containment < FUZZY_JACCARD_THRESHOLD {
            continue;
        }
        let token = if block.counts.any_fffd {
            // The decoder rewrote the block's own tokens, so the candidate's
            // decoded text is what has to be compared: re-tokenize it (the
            // block that forces this is rare enough to pay for it). Scoped to
            // the window, because the multiset borrows from it.
            let held: Vec<Cow<'_, str>> = (start..start + span)
                .map(|i| String::from_utf8_lossy(lines.line(bytes, i)))
                .collect();
            let mut decoded: HashMap<&[u8], u32> = HashMap::new();
            for line in &held {
                for token in line.split_whitespace() {
                    *decoded.entry(token.as_bytes()).or_insert(0) += 1;
                }
            }
            debug_assert_eq!(
                decoded.values().map(|&count| u64::from(count)).sum::<u64>(),
                window_tokens,
                "the index's window total and the decoded window tokenize the same text",
            );
            block.window_token_jaccard(&decoded, window_tokens)
        } else {
            window.clear();
            for i in start..start + span {
                let line = lines.line(bytes, i);
                for &(s, e) in lines.line_runs(i) {
                    *window.entry(&line[s as usize..e as usize]).or_insert(0) += 1;
                }
            }
            debug_assert_eq!(
                window.values().map(|&count| u64::from(count)).sum::<u64>(),
                window_tokens,
                "the index's window total and the window's own runs are the same tokens",
            );
            block.window_token_jaccard(&window, window_tokens)
        };
        if token * containment >= FUZZY_JACCARD_THRESHOLD {
            hits.push(crate::rk64::Location {
                path: path.to_string(),
                start_line: (start + 1) as u32,
                end_line: (start + span) as u32,
            });
        }
    }
}

/// Does line `i` of a candidate file match any certified line? Three steps,
/// each letting the next see less: a line with no tokens is a match exactly
/// when the block has a blank line; a line whose token count is outside every
/// certified line's Jaccard window cannot match one and is never read; the
/// rest are compared by bytes ([`line_matches_any_cert`]).
///
/// The count window is load-bearing here, not just an optimization detail: on
/// a corpus whose bulk is media, a line of a GIF has a handful of tokens —
/// its bytes are mostly one long run between stray whitespace — so without it
/// every such line would be scanned against the block. And the count is free:
/// it is a fact the line index already holds ([`FuzzyLines`]).
fn line_flag(
    bytes: &[u8],
    lines: &FuzzyLines,
    i: usize,
    block: &CertBlock<'_>,
    runs: &mut Vec<(u32, u32, u32)>,
) -> bool {
    let tokens = lines.tokens[i];
    if tokens == 0 {
        return block.counts.any_blank;
    }
    if !block.counts.could_match(tokens) {
        return false;
    }
    let raw = lines.line(bytes, i);
    // A run the decoder would rewrite can only match a certified token that
    // carries a replacement character too. A block without one — the only
    // case in practice — is therefore compared against the raw runs, and the
    // line is never decoded. A block with one is read through a decode: the
    // decoded text's runs are the same tokens, and every run of it is valid,
    // so the same comparison serves both — at the price of re-walking the
    // decoded bytes, paid only for a block that holds a replacement
    // character.
    if block.counts.any_fffd {
        let lossy = String::from_utf8_lossy(raw);
        let ranges: Vec<(u32, u32)> = Runs::new(lossy.as_bytes())
            .map(|(start, end)| (start as u32, end as u32))
            .collect();
        line_matches_any_cert(lossy.as_bytes(), tokens, &ranges, block, runs)
    } else {
        line_matches_any_cert(raw, tokens, lines.line_runs(i), block, runs)
    }
}

/// The containment half of [`fuzzy_window_score`]: does this candidate line
/// match at least one certified line — blank against blank, or a token
/// multiset Jaccard reaching [`FUZZY_LINE_MATCH_THRESHOLD`]? The certified
/// token index limits full intersections to certified lines that share a
/// token with the candidate, and the Jaccard bound
/// `(1+T)·min ≥ T·(|a|+|b|)` (intersection never exceeds either side) skips
/// the rest.
///
/// Nothing here is a `&str`: `line_bytes` is the line's bytes as the decoder
/// would produce them — for a block without a replacement character, the
/// line's own bytes — so a token is a byte range of it, compared by bytes
/// against certified tokens keyed the same way. `total` and `line_runs` are
/// the line's token count and its tokens' ranges, both from the index: the
/// count is the line's total on both sides of the comparison, since [`Runs`]
/// counts the decoder's tokens, and the ranges are the runs a re-walk would
/// find in `line_bytes`, in order — the caller passes the index's own ranges
/// for the raw bytes, and freshly walked ones for a decoded line, whose
/// offsets differ from the raw line's.
///
/// `block` carries the block's token filter: a token the block cannot contain
/// is not looked up at all, and on the corpus that dominates this pass —
/// binary runs and prose against a block of code — that is every token of
/// nearly every line.
///
/// `runs` is the caller's scratch buffer, overwritten with the line's
/// distinct certified tokens — (start, end, multiplicity) triples in
/// first-appearance order, which for a line's handful of them beats a hash
/// map, and which borrows nothing, so the pass never holds a decoded line
/// alive.
fn line_matches_any_cert(
    line_bytes: &[u8],
    total: u32,
    line_runs: &[(u32, u32)],
    block: &CertBlock<'_>,
    runs: &mut Vec<(u32, u32, u32)>,
) -> bool {
    debug_assert_eq!(
        line_runs.len() as u32,
        total,
        "the line's ranges and count describe the same tokens",
    );
    // The line's tokens that the block could possibly contain, with their
    // multiplicities — and nothing else. Repeats are collapsed only for those:
    // a token the block does not have contributes `min(a, 0) = 0` to every
    // intersection, so tracking it is bookkeeping with nothing to compute from
    // it, and collapsing repeats is the one super-linear step in the matcher.
    // On a corpus of media and prose the filter rejects a line after a length
    // comparison and a handful of bit tests, where the full tokenization ran
    // for every line of every file of every scan.
    runs.clear();
    for &(start, end) in line_runs {
        let token = &line_bytes[start as usize..end as usize];
        if !block.fps.may_contain(token) || !block.index.contains_key(token) {
            continue;
        }
        match runs
            .iter_mut()
            .find(|(s, e, _)| line_bytes[*s as usize..*e as usize] == *token)
        {
            Some((_, _, count)) => *count += 1,
            None => runs.push((start, end, 1)),
        }
    }
    if runs.is_empty() {
        return false;
    }
    let mut seen: Vec<u32> = Vec::new();
    for &(start, end, _) in runs.iter() {
        let Some(idxs) = block.index.get(&line_bytes[start as usize..end as usize]) else {
            continue;
        };
        for &ci in idxs {
            if seen.contains(&ci) {
                continue;
            }
            seen.push(ci);
            let c = &block.data[ci as usize];
            if c.blank {
                continue;
            }
            let b_total = c.total;
            if (1.0 + FUZZY_LINE_MATCH_THRESHOLD) * (f64::from(total.min(b_total)))
                < FUZZY_LINE_MATCH_THRESHOLD * f64::from(total + b_total)
            {
                continue;
            }
            let inter: u32 = runs
                .iter()
                .map(|&(s, e, ca)| {
                    c.counts
                        .get(&line_bytes[s as usize..e as usize])
                        .map_or(0, |&cb| ca.min(cb))
                })
                .sum();
            let jaccard = f64::from(inter) / f64::from(total + b_total - inter);
            if jaccard >= FUZZY_LINE_MATCH_THRESHOLD {
                return true;
            }
        }
    }
    false
}

/// A move verdict. Exact-tier matches keep the certified content byte-for-
/// byte (`content_identical: true`); fuzzy-tier matches are lightly-edited
/// near-copies (`content_identical: false`), which the fix phase must report
/// honestly instead of claiming "certified content moved".
fn moved_to(location: &crate::rk64::Location, content_identical: bool) -> MoveScanDecision {
    MoveScanDecision::Moved {
        location: location.clone(),
        content_identical,
    }
}

/// Window height of an inclusive 1-based range; `0` for a degenerate range
/// (`start == 0` or `end < start`) that selects no content.
fn line_range_span(start: u32, end: u32) -> usize {
    if start == 0 || end < start {
        return 0;
    }
    (end - start + 1) as usize
}

/// True when the 1-based range selects real lines of `bytes` — every part of
/// the range must lie within the line count, so a partially overhanging
/// range is Broken per the card.
fn extent_fits(bytes: &[u8], start: u32, end: u32) -> bool {
    if start == 0 || end < start {
        return false;
    }
    let idx = LineIndex::build(bytes);
    (end as usize) <= idx.line_count()
}

/// One candidate file of the cross-file move scan: the path the scan reports
/// it under, the bytes both tiers read it as, and each tier's index — the
/// exact tier's [`ScanIndex`] and the fuzzy tier's [`FuzzyLines`].
///
/// The indexes are built with the inventory, not per query. Every link's move
/// scan asks the same question of the same inventory — "is this certified
/// block somewhere in the repo?" — so indexing each candidate per query (one
/// canonical pass and one whole-buffer hash, *per file, per link*) multiplies
/// the corpus by the link count. On the cards corpus that was 96 scans over
/// 5,678 files: 95.7 s, 85.8% of the whole command. Indexed once per
/// inventory, each query is window arithmetic over tables already in cache.
struct CandidateFile {
    path: String,
    bytes: Vec<u8>,
    index: ScanIndex,
    fuzzy: FuzzyLines,
}

/// Repo candidate files for the cross-file move scan: every tracked file
/// readable from the current side, excluding build output. Read through the
/// source-aware reader so `--source head`/`index` scan the same layer the
/// targets were read from.
fn candidate_files(
    reader: &crate::git::GitReader,
    repo_root: &Path,
    source: DocSource,
) -> Result<Vec<CandidateFile>, EpochError> {
    let list = git_output(repo_root, &["ls-files", "-z"])?;
    let paths: Vec<&str> = list
        .split('\0')
        .filter(|path| !path.is_empty())
        .filter(|path| {
            !path.starts_with("node_modules/")
                && !path.starts_with("target/")
                && !path.starts_with("dist/")
        })
        .collect();
    // The reads are independent of one another — one open per tracked file,
    // no ordering between them — and the batch is every tracked candidate in
    // the repo (tens of megabytes across thousands of files on a real wiki
    // corpus), read once per run on the first link that needs a move scan.
    // The worktree layer is a plain filesystem open with no shared state, so
    // it fans out across the worker pool; the blob layers read through the
    // run's gix reader, which holds `Rc` index snapshots and is not `Sync`,
    // so they stay serial.
    let read: Vec<Option<Vec<u8>>> = match source {
        DocSource::WorkingTree => parallel_map(&paths, |path| read_worktree(repo_root, path)),
        DocSource::Head | DocSource::Index => {
            let mut read = Vec::with_capacity(paths.len());
            for path in &paths {
                read.push(read_current(reader, repo_root, source, path)?);
            }
            read
        }
    };
    let mut present: Vec<(String, Vec<u8>)> = Vec::with_capacity(paths.len());
    for (path, bytes) in paths.iter().zip(read) {
        if let Some(bytes) = bytes {
            present.push(((*path).to_string(), bytes));
        }
    }
    // Indexing is a pure function of the bytes that borrows nothing, so it
    // fans out across the pool whichever layer read them — including the blob
    // layers, whose reads hold a non-`Sync` gix reader and so stay serial.
    let indexes = parallel_map(&present, |(_, bytes)| {
        (ScanIndex::build(bytes), FuzzyLines::build(bytes))
    });
    Ok(present
        .into_iter()
        .zip(indexes)
        .map(|((path, bytes), (index, fuzzy))| CandidateFile {
            path,
            bytes,
            index,
            fuzzy,
        })
        .collect())
}

/// The bytes of `path` from the layer selected by `source` (the
/// `read_anchor_source` pattern): worktree fs, `HEAD` blob, or index blob.
/// `Ok(None)` when the path is absent in that layer.
fn read_current(
    reader: &crate::git::GitReader,
    repo_root: &Path,
    source: DocSource,
    path: &str,
) -> Result<Option<Vec<u8>>, EpochError> {
    match source {
        DocSource::WorkingTree => Ok(read_worktree(repo_root, path)),
        DocSource::Head => read_blob_at(reader, repo_root, "HEAD", path),
        DocSource::Index => read_blob_at(reader, repo_root, "", path),
    }
}

/// The worktree layer's read of `path`: a plain filesystem open, absent-file
/// included. Split out from [`read_current`] because it needs no reader — the
/// one layer whose candidate reads carry no shared state and so may run on
/// the worker pool.
fn read_worktree(repo_root: &Path, path: &str) -> Option<Vec<u8>> {
    std::fs::read(repo_root.join(path)).ok()
}

/// Decision 5 target resolution: resolve the href's path part against the
/// page's directory, read it from the current side, and — for `WorkingTree`
/// sources, when the direct read misses — salvage the longest existing
/// repo-relative suffix (`old/dir/file.md` → `file.md`). The effective path
/// comes back alongside the bytes; a target that stays missing yields the
/// resolved path with `None`, so callers still report where the link
/// pointed. The blob layers (`Head`/`Index`) never salvage: there is no
/// worktree filesystem to consult, so they stay fail-closed.
fn read_target(
    reader: &crate::git::GitReader,
    repo_root: &Path,
    source: DocSource,
    page_path: &str,
    path_part: &str,
) -> Result<(String, Option<Vec<u8>>), EpochError> {
    let target_path = resolve_target_path(repo_root, page_path, path_part);
    if let Some(bytes) = read_current(reader, repo_root, source, &target_path)? {
        return Ok((target_path, Some(bytes)));
    }
    if source == DocSource::WorkingTree
        && let Some(salvaged) = super::locate_existing_suffix(&target_path, repo_root)
        && let Some(bytes) = read_current(reader, repo_root, source, &salvaged)?
    {
        return Ok((salvaged, Some(bytes)));
    }
    Ok((target_path, None))
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
    if !frontmatter::has_wiki_frontmatter(content) {
        return None;
    }
    let (yaml_start, yaml_end, _) = frontmatter::yaml_block_bounds(content)?;
    let yaml = &content[yaml_start..yaml_end];
    if yaml.lines().any(line_declares_links_reviewed) {
        return None;
    }
    // Match the block's own line ending so CRLF checkouts stay CRLF.
    let eol = if yaml.contains("\r\n") { "\r\n" } else { "\n" };
    let mut out = String::with_capacity(content.len() + "links-reviewed: 1".len() + eol.len());
    out.push_str(&content[..yaml_end]);
    out.push_str("links-reviewed: 1");
    out.push_str(eol);
    out.push_str(&content[yaml_end..]);
    Some(out)
}

/// True when a YAML line declares the `links-reviewed` key (bare or quoted).
fn line_declares_links_reviewed(line: &str) -> bool {
    let t = line.trim_start();
    let t = t
        .strip_prefix('"')
        .or_else(|| t.strip_prefix('\''))
        .unwrap_or(t);
    t.strip_prefix("links-reviewed")
        .is_some_and(|rest| rest.trim_start().starts_with(':'))
}

// ── Tests ─────────────────────────────────────────────────────────────────────
//
// Phase 0 P2 (tdd-bootstrap): acceptance checks against the stubs, all
// pending. P3 unskips them one concern at a time. Every repo-backed test uses
// a real temp repository with real git history — no mocks.

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    use tempfile::TempDir;

    use crate::cache::NoopCache;
    use super::*;

    const BLOCK: &str = "block-line-1\nblock-line-2\nblock-line-3";

    /// A six-line code block for the fuzzy-tier tests. Six lines (not three,
    /// like `BLOCK`) because the 0.7 tier threshold is deliberately out of
    /// reach for a whole-line replacement in a short block — see
    /// `fuzzy_window_score_three_line_whole_edit_stays_below`.
    const FUZZY_BLOCK: &str = "\
fn resolve_target_path(root: &Path, page: &str, href: &str) -> String {
    let joined = root.join(page).join(href);
    let normalized = normalize_segments(&joined);
    assert!(normalized.is_relative(), \"target escaped the repo\");
    normalized
}";

    /// `FUZZY_BLOCK` with the third line's call renamed — a whole-line edit
    /// that the exact tier cannot follow but the fuzzy tier must.
    const FUZZY_BLOCK_EDITED: &str = "\
fn resolve_target_path(root: &Path, page: &str, href: &str) -> String {
    let joined = root.join(page).join(href);
    let normalized = canonicalize_segments(&joined);
    assert!(normalized.is_relative(), \"target escaped the repo\");
    normalized
}";

    /// Real corpus excerpt: the `collect_with_source` declaration block in
    /// check.rs as it reads at the Phase 1c commit.
    const REAL_COLLECT_WITH_SOURCE: &str = "/// Collect diagnostics with an explicit `DocSource`.
pub fn collect_with_source(
    globs: &[String],
    repo_root: &Path,
    source: DocSource,
) -> Result<Vec<CheckDiagnostic>> {
    // This entry point (hook/tests) scans from the repo root rather than a
    // narrower working directory.  discover_files returns Ok(vec![]) for an
    // empty corpus; propagate that as an error so the caller sees \"no wiki
    // pages found\" rather than an empty diagnostic list with exit 0.";

    /// The same block with one comment word changed — the move-and-edit
    /// shape the fuzzy tier exists for.
    const REAL_COLLECT_WITH_SOURCE_EDITED: &str = "/// Collect diagnostics with a specific `DocSource`.
pub fn collect_with_source(
    globs: &[String],
    repo_root: &Path,
    source: DocSource,
) -> Result<Vec<CheckDiagnostic>> {
    // This entry point (hook/tests) scans from the repo root rather than a
    // narrower working directory.  discover_files returns Ok(vec![]) for an
    // empty corpus; propagate that as an error so the caller sees \"no wiki
    // pages found\" rather than an empty diagnostic list with exit 0.";

    /// Real corpus excerpt: a drift-check paragraph taken from the mesh-era
    /// architecture page (conformed in Phase 1c; the page died with the mesh).
    const REAL_WIKI_DRIFT_PARAGRAPH: &str = "Classifies every internal fragment link with a line range through the drift engine: the page's `links-reviewed` field selects its certification commit from git history, and each link's target range is compared against the content that certification recorded.";

    /// The same paragraph with one phrase swapped.
    const REAL_WIKI_DRIFT_PARAGRAPH_EDITED: &str = "Classifies every internal fragment link with a line range through the drift engine: the page's `links-reviewed` field selects its certification commit from git history, and each link's target range is compared against the content that review recorded.";

    /// Real corpus excerpt from another page: the wikiignore exclusion
    /// paragraph of wiki-cli-advanced-usage.md.
    const REAL_WIKIIGNORE_PARAGRAPH: &str = "excludes paths from `wiki check` entirely — before frontmatter parsing, link validation, or line-range drift classification ever runs.";

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
            self.create_file_bytes(path, content.as_bytes());
        }

        /// [`create_file`] for content that is not valid UTF-8.
        fn create_file_bytes(&self, path: &str, content: &[u8]) {
            let full = self.dir.path().join(path);
            if let Some(parent) = full.parent() {
                fs::create_dir_all(parent).expect("create_dir_all");
            }
            fs::write(&full, content).expect("write file");
        }

        fn remove_file(&self, path: &str) {
            fs::remove_file(self.dir.path().join(path)).expect("remove file");
        }

        /// A committed rename: `git mv` stages the rename so the commit
        /// records an `R###` row — the identity evidence the cross-file
        /// relocation rule requires.
        fn rename_file(&self, from: &str, to: &str) {
            self.git(&["mv", from, to]);
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
        /// `git clone --depth 1`. The `file://` transport matters: git
        /// ignores `--depth` on local-path clones and copies full history.
        fn shallow_clone(&self) -> TempDir {
            let dst = tempfile::tempdir().expect("tempdir for clone");
            let src = format!("file://{}", self.dir.path().display());
            let output = Command::new("git")
                .args(["clone", "-q", "--depth", "1"])
                .arg(&src)
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

    /// A [`crate::git::GitReader`] for `root`. The drift entry points take a
    /// shared handle (one repository open per run) rather than opening the
    /// repository per page; the tests open one per call, which is what they
    /// did before the handle existed.
    fn reader(root: &Path) -> crate::git::GitReader {
        crate::git::GitReader::open(root).expect("open reader")
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
            &reader(repo.path()),
            &NoopCache,
            DocSource::WorkingTree,
            page,
            &repo.read(page),
            epoch,
            &mut MoveScanCtx::new(&DriftRunCtx::new()),
        )
    }

    fn field_value(content: &str) -> LinksReviewedRead {
        read_links_reviewed(content)
    }

    /// A readable field value for `find_anchor_commit` call sites.
    fn read(v: Option<&str>) -> LinksReviewedRead {
        LinksReviewedRead::Readable(v.map(str::to_owned))
    }

    // ── read_links_reviewed ──

    #[test]
    fn extracts_scalar_values_to_their_string_form() {
        assert_eq!(
            field_value(&make_wiki_page("P", "body\n", Some("1"))),
            LinksReviewedRead::Readable(Some("1".into()))
        );
        assert_eq!(
            field_value(&make_wiki_page("P", "body\n", Some("v2"))),
            LinksReviewedRead::Readable(Some("v2".into()))
        );
        assert_eq!(
            field_value(&make_wiki_page("P", "body\n", Some("\"quoted value\""))),
            LinksReviewedRead::Readable(Some("quoted value".into())),
            "YAML string scalars unquote"
        );
        assert_eq!(
            field_value(&make_wiki_page("P", "body\n", Some("2"))),
            LinksReviewedRead::Readable(Some("2".into())),
            "numeric scalars coerce to their string form"
        );
    }

    #[test]
    fn reads_absent_field_and_unparseable_yaml_distinctly() {
        assert_eq!(
            field_value(&make_wiki_page("P", "body\n", None)),
            LinksReviewedRead::Readable(None)
        );
        assert_eq!(
            field_value("no frontmatter at all\n"),
            LinksReviewedRead::Readable(None)
        );
        // A field-looking line in the BODY is not frontmatter.
        let body = "links-reviewed: 5\n";
        assert_eq!(
            field_value(&make_wiki_page("P", body, None)),
            LinksReviewedRead::Readable(None)
        );
        // A broken YAML block is its own state — not a value, not an absence
        // (finding yaml-breakage-rebaselines).
        let broken = "---\ntitle: [unclosed\nlinks-reviewed: 1\n---\nbody\n";
        assert_eq!(
            field_value(broken),
            LinksReviewedRead::Unparseable,
            "unparseable YAML must read as Unparseable, never as an absent field"
        );
    }

    // ── parse_line_range_links: non-content scrubbing ──

    #[test]
    fn range_links_inside_inline_code_are_not_parsed() {
        let content = "See `[x](path#L10-L20)` and a real [y](./target.rs#L2-L4).\n";
        let links = parse_line_range_links(content);
        assert_eq!(links.len(), 1, "inline-code link must not parse: {links:?}");
        assert_eq!(links[0].original_href, "./target.rs#L2-L4");
    }

    #[test]
    fn range_links_inside_code_blocks_are_not_parsed() {
        let content = "```rust\n[x](path#L10-L20)\n```\nReal [y](./target.rs#L2-L4).\n";
        let links = parse_line_range_links(content);
        assert_eq!(links.len(), 1, "code-block link must not parse: {links:?}");
        assert_eq!(links[0].original_href, "./target.rs#L2-L4");
    }

    // ── insert_links_reviewed ──

    #[test]
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
    fn preserves_crlf_and_missing_trailing_newline() {
        let crlf = "---\r\ntitle: P\r\nsummary: S\r\n---\r\nbody\r\n";
        let with_field = insert_links_reviewed(crlf).expect("has frontmatter");
        assert_eq!(
            with_field, "---\r\ntitle: P\r\nsummary: S\r\nlinks-reviewed: 1\r\n---\r\nbody\r\n",
            "the inserted line matches the file's EOL"
        );

        let no_nl = "---\ntitle: P\nsummary: S\n---\nbody";
        let with_field = insert_links_reviewed(no_nl).expect("has frontmatter");
        assert_eq!(
            with_field,
            "---\ntitle: P\nsummary: S\nlinks-reviewed: 1\n---\nbody"
        );
    }

    #[test]
    fn preserves_multiline_and_quoted_neighbor_values() {
        let content =
            "---\ntitle: P\nsummary: |\n  multi\n  line\nkeywords: [\"a\", \"b\"]\n---\nbody\n";
        let with_field = insert_links_reviewed(content).expect("has frontmatter");
        assert_eq!(
            with_field,
            "---\ntitle: P\nsummary: |\n  multi\n  line\nkeywords: [\"a\", \"b\"]\nlinks-reviewed: 1\n---\nbody\n"
        );
    }

    #[test]
    fn refuses_pages_without_wiki_frontmatter() {
        assert_eq!(insert_links_reviewed("just text\n"), None);
        assert_eq!(insert_links_reviewed("---\nname: skill\n---\nbody\n"), None);
    }

    // ── find_anchor_commit: pending-certification rule ──

    #[test]
    fn pending_bump_makes_current_state_the_epoch() {
        let repo = TestRepo::new();
        repo.create_file("wiki/page.md", &make_wiki_page("P", "body\n", Some("1")));
        repo.commit("field=1");
        repo.create_file("wiki/page.md", &make_wiki_page("P", "body\n", Some("2")));
        let epoch = find_anchor_commit(repo.path(), &reader(repo.path()), &NoopCache, &DriftRunCtx::new(), "wiki/page.md", &read(Some("2")), &read(Some("1")))
            .expect("resolves");
        assert_eq!(
            epoch,
            LinkEpoch::Current {
                value: Some("2".into())
            }
        );
    }

    #[test]
    fn field_added_but_uncommitted_is_pending() {
        let repo = TestRepo::new();
        repo.create_file("wiki/page.md", &make_wiki_page("P", "body\n", None));
        repo.commit("no field");
        repo.create_file("wiki/page.md", &make_wiki_page("P", "body\n", Some("1")));
        let epoch =
            find_anchor_commit(repo.path(), &reader(repo.path()), &NoopCache, &DriftRunCtx::new(), "wiki/page.md", &read(Some("1")), &read(None)).expect("resolves");
        assert_eq!(
            epoch,
            LinkEpoch::Current {
                value: Some("1".into())
            }
        );
    }

    #[test]
    fn field_removed_but_uncommitted_is_pending_with_none_value() {
        let repo = TestRepo::new();
        repo.create_file("wiki/page.md", &make_wiki_page("P", "body\n", Some("1")));
        repo.commit("field=1");
        repo.create_file("wiki/page.md", &make_wiki_page("P", "body\n", None));
        let epoch =
            find_anchor_commit(repo.path(), &reader(repo.path()), &NoopCache, &DriftRunCtx::new(), "wiki/page.md", &read(None), &read(Some("1"))).expect("resolves");
        assert_eq!(epoch, LinkEpoch::Current { value: None });
    }

    #[test]
    fn field_absent_everywhere_is_missing() {
        let repo = TestRepo::new();
        repo.create_file("wiki/page.md", &make_wiki_page("P", "body\n", None));
        repo.commit("no field");
        let epoch = find_anchor_commit(repo.path(), &reader(repo.path()), &NoopCache, &DriftRunCtx::new(), "wiki/page.md", &read(None), &read(None)).expect("resolves");
        assert_eq!(epoch, LinkEpoch::Missing);
    }

    // ── find_anchor_commit: the history walk ──

    #[test]
    fn anchors_at_the_newest_value_changing_commit() {
        let repo = TestRepo::new();
        repo.create_file("wiki/page.md", &make_wiki_page("P", "body\n", Some("1")));
        repo.commit("field=1");
        repo.create_file("wiki/page.md", &make_wiki_page("P", "edited\n", Some("1")));
        repo.commit("body edit");
        repo.create_file("wiki/page.md", &make_wiki_page("P", "edited\n", Some("2")));
        let bump_sha = repo.commit("bump to 2");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("P", "edited again\n", Some("2")),
        );
        repo.commit("body edit after bump");

        let epoch = find_anchor_commit(repo.path(), &reader(repo.path()), &NoopCache, &DriftRunCtx::new(), "wiki/page.md", &read(Some("2")), &read(Some("2")))
            .expect("resolves");
        assert_eq!(
            epoch,
            LinkEpoch::Commit {
                sha: bump_sha,
                path_at_commit: "wiki/page.md".into(),
                value: Some("2".into()),
            }
        );
    }

    #[test]
    fn anchors_at_field_introduction_when_no_pair_differs() {
        let repo = TestRepo::new();
        repo.create_file("wiki/page.md", &make_wiki_page("P", "body\n", Some("1")));
        let intro_sha = repo.commit("field=1");
        repo.create_file("wiki/page.md", &make_wiki_page("P", "edited\n", Some("1")));
        repo.commit("body edit");

        let epoch = find_anchor_commit(repo.path(), &reader(repo.path()), &NoopCache, &DriftRunCtx::new(), "wiki/page.md", &read(Some("1")), &read(Some("1")))
            .expect("resolves");
        assert_eq!(
            epoch,
            LinkEpoch::Commit {
                sha: intro_sha,
                path_at_commit: "wiki/page.md".into(),
                value: Some("1".into()),
            }
        );
    }

    #[test]
    fn nonsquash_merge_preserves_feature_branch_certification() {
        let repo = TestRepo::new();
        repo.create_file("wiki/page.md", &make_wiki_page("P", "body\n", Some("1")));
        repo.commit("field=1");
        repo.git(&["checkout", "-q", "-b", "feature"]);
        repo.create_file("wiki/page.md", &make_wiki_page("P", "body\n", Some("2")));
        let bump_sha = repo.commit("bump on feature only");
        repo.git(&["checkout", "-q", "master"]);
        repo.create_file("wiki/other.md", &make_wiki_page("Other", "x\n", None));
        repo.commit("unrelated on master");
        repo.git(&["merge", "--no-ff", "-q", "-m", "merge feature", "feature"]);

        // HEAD is the merge commit; the certification exists only on the
        // feature branch — a --first-parent walk would never see it.
        let epoch = find_anchor_commit(repo.path(), &reader(repo.path()), &NoopCache, &DriftRunCtx::new(), "wiki/page.md", &read(Some("2")), &read(Some("2")))
            .expect("resolves");
        assert_eq!(
            epoch,
            LinkEpoch::Commit {
                sha: bump_sha,
                path_at_commit: "wiki/page.md".into(),
                value: Some("2".into()),
            }
        );
    }

    #[test]
    fn two_chained_renames_still_resolve_with_the_anchor_time_name() {
        let repo = TestRepo::new();
        repo.create_file("wiki/page.md", &make_wiki_page("P", "body\n", Some("1")));
        let intro_sha = repo.commit("field=1 at page.md");
        repo.git(&["mv", "wiki/page.md", "wiki/renamed.md"]);
        repo.commit("rename to renamed.md");
        repo.git(&["mv", "wiki/renamed.md", "wiki/final-name.md"]);
        repo.commit("rename to final-name.md");

        let epoch = find_anchor_commit(
            repo.path(),
            &reader(repo.path()),
            &NoopCache,
            &DriftRunCtx::new(),
            "wiki/final-name.md",
            &read(Some("1")),
            &read(Some("1")),
        )
        .expect("resolves");
        assert_eq!(
            epoch,
            LinkEpoch::Commit {
                sha: intro_sha,
                path_at_commit: "wiki/page.md".into(),
                value: Some("1".into()),
            },
            "the blob is read under the name in effect at the anchor commit"
        );
    }

    #[test]
    fn shallow_clone_fails_closed() {
        let repo = TestRepo::new();
        repo.create_file("wiki/page.md", &make_wiki_page("P", "body\n", Some("1")));
        repo.commit("field=1");
        repo.create_file("wiki/page.md", &make_wiki_page("P", "edited\n", Some("1")));
        repo.commit("body edit");

        let clone = repo.shallow_clone();
        let err = find_anchor_commit(
            clone.path(),
            &reader(clone.path()),
            &NoopCache,
            &DriftRunCtx::new(),
            "wiki/page.md",
            &read(Some("1")),
            &read(Some("1")),
        )
        .expect_err("shallow history cannot resolve an anchor epoch");
        assert!(matches!(err, EpochError::ShallowClone), "got {err:?}");
    }

    // ── walk: unparseable commits are not values, not epoch events ──

    #[test]
    fn unparseable_commit_is_skipped_so_repair_cannot_reanchor() {
        let repo = TestRepo::new();
        repo.create_file("wiki/page.md", &make_wiki_page("P", "body\n", Some("1")));
        let intro_sha = repo.commit("field=1");
        // Commit B breaks the YAML block entirely; commit C repairs it with
        // the SAME value (no bump). Under the old conflating read, B parsed
        // as `None` and the (C, B) pair differed — re-anchoring at C and
        // silently certifying whatever C's page carries.
        repo.create_file(
            "wiki/page.md",
            "---\ntitle: [unclosed\nlinks-reviewed: 1\n---\nbody\n",
        );
        repo.commit("break yaml");
        repo.create_file("wiki/page.md", &make_wiki_page("P", "edited\n", Some("1")));
        repo.commit("repair, no bump");

        let epoch = find_anchor_commit(repo.path(), &reader(repo.path()), &NoopCache, &DriftRunCtx::new(), "wiki/page.md", &read(Some("1")), &read(Some("1")))
            .expect("resolves");
        assert_eq!(
            epoch,
            LinkEpoch::Commit {
                sha: intro_sha,
                path_at_commit: "wiki/page.md".into(),
                value: Some("1".into()),
            },
            "the repair commit must not re-anchor: B is skipped entirely, C \
             compares equal to A, and the anchor stays at the introduction"
        );
    }

    #[test]
    fn unparseable_current_side_fails_closed() {
        let repo = TestRepo::new();
        repo.create_file("wiki/page.md", &make_wiki_page("P", "body\n", Some("1")));
        repo.commit("field=1");
        // The worktree YAML is broken right now: no classification may run
        // against it — neither a silent Healthy pass nor a relocation.
        repo.create_file(
            "wiki/page.md",
            "---\ntitle: [unclosed\nlinks-reviewed: 1\n---\nbody\n",
        );
        let err = find_anchor_commit(
            repo.path(),
            &reader(repo.path()),
            &NoopCache,
            &DriftRunCtx::new(),
            "wiki/page.md",
            &field_value(&repo.read("wiki/page.md")),
            &read(Some("1")),
        )
        .expect_err("an unparseable current side must fail closed");
        assert!(matches!(err, EpochError::UnparseableYaml { .. }), "got {err:?}");
    }

    #[test]
    fn unparseable_committed_side_walks_the_readable_history() {
        let repo = TestRepo::new();
        repo.create_file("wiki/page.md", &make_wiki_page("P", "body\n", Some("1")));
        let intro_sha = repo.commit("field=1");
        repo.create_file(
            "wiki/page.md",
            "---\ntitle: [unclosed\nlinks-reviewed: 1\n---\nbody\n",
        );
        repo.commit("break yaml");
        // A broken HEAD is not a value and not `Missing`: the pending rule
        // cannot compare it, so the walk resolves the epoch from the
        // readable history.
        let epoch = find_anchor_commit(
            repo.path(),
            &reader(repo.path()),
            &NoopCache,
            &DriftRunCtx::new(),
            "wiki/page.md",
            &read(Some("1")),
            &LinksReviewedRead::Unparseable,
        )
        .expect("resolves");
        assert_eq!(
            epoch,
            LinkEpoch::Commit {
                sha: intro_sha,
                path_at_commit: "wiki/page.md".into(),
                value: Some("1".into()),
            }
        );
    }

    #[test]
    fn all_unparseable_history_fails_closed() {
        let repo = TestRepo::new();
        repo.create_file(
            "wiki/page.md",
            "---\ntitle: [unclosed\n---\nbody\n",
        );
        repo.commit("broken from the start");
        let err = find_anchor_commit(
            repo.path(),
            &reader(repo.path()),
            &NoopCache,
            &DriftRunCtx::new(),
            "wiki/page.md",
            &read(Some("1")),
            &LinksReviewedRead::Unparseable,
        )
        .expect_err("no readable value anywhere must fail closed");
        assert!(matches!(err, EpochError::UnparseableYaml { .. }), "got {err:?}");
    }

    // ── classify_page: one test per outcome ──

    #[test]
    fn healthy_when_target_unchanged() {
        let (repo, c1) = repo_with_certified_link();
        let epoch = LinkEpoch::Commit {
            sha: c1,
            path_at_commit: "wiki/page.md".into(),
            value: Some("1".into()),
        };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(classes.len(), 1);
        assert_eq!(classes[0].outcome, DriftOutcome::Healthy);
    }

    #[test]
    fn drift_when_target_content_changed() {
        let (repo, c1) = repo_with_certified_link();
        repo.create_file(
            "wiki/target.md",
            "T0\nblock-line-1\nblock-line-2\nCHANGED\nT1\n",
        );
        repo.commit("target edited");
        let epoch = LinkEpoch::Commit {
            sha: c1,
            path_at_commit: "wiki/page.md".into(),
            value: Some("1".into()),
        };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(classes[0].outcome, DriftOutcome::Drift);
    }

    #[test]
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
        let epoch = LinkEpoch::Commit {
            sha: c1,
            path_at_commit: "wiki/page.md".into(),
            value: Some("1".into()),
        };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(classes.len(), 2);
        assert_eq!(classes[0].outcome, DriftOutcome::Healthy);
        assert_eq!(classes[1].outcome, DriftOutcome::Uncertified);
    }

    #[test]
    fn broken_when_target_deleted() {
        let (repo, c1) = repo_with_certified_link();
        repo.remove_file("wiki/target.md");
        repo.commit("target deleted");
        let epoch = LinkEpoch::Commit {
            sha: c1,
            path_at_commit: "wiki/page.md".into(),
            value: Some("1".into()),
        };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(classes[0].outcome, DriftOutcome::Broken);
    }

    #[test]
    fn broken_when_extent_overhangs_target() {
        let (repo, c1) = repo_with_certified_link();
        repo.create_file("wiki/target.md", "T0\n");
        // Even with the certified content visible verbatim elsewhere, a
        // truncated target is Broken per the card's flowchart: the extent no
        // longer fits its target's line count, so the content comparison is
        // uncomputable and the move scan is not consulted (Decision 5 step 3).
        repo.create_file("wiki/other.md", &format!("H\n{BLOCK}\nF\n"));
        repo.commit("target truncated");
        let epoch = LinkEpoch::Commit {
            sha: c1,
            path_at_commit: "wiki/page.md".into(),
            value: Some("1".into()),
        };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(classes[0].outcome, DriftOutcome::Broken);
    }

    /// A repo where the certified page at `wiki/page.md` links
    /// `[b](../target.md#L2-L4)` and the target lives at the repo root — the
    /// shape the suffix-salvage tests need: a stale href prefix drops
    /// segment by segment until `target.md` matches.
    fn repo_with_root_target() -> (TestRepo, String) {
        let repo = TestRepo::new();
        repo.create_file(
            "target.md",
            "T0\nblock-line-1\nblock-line-2\nblock-line-3\nT1\n",
        );
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "[b](../target.md#L2-L4)\n", Some("1")),
        );
        let c1 = repo.commit("certified page and root target");
        (repo, c1)
    }

    #[test]
    fn working_tree_salvage_reunites_stale_href_with_its_target() {
        let (repo, c1) = repo_with_root_target();
        // The href gains a directory prefix that does not exist anywhere:
        // `wiki/dir/target.md` misses, suffix salvage falls back to
        // `target.md`, and the link is judged against the certified target —
        // Healthy, with the effective path reported, not the stale href path.
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "[b](dir/target.md#L2-L4)\n", Some("1")),
        );
        let epoch = LinkEpoch::Commit {
            sha: c1,
            path_at_commit: "wiki/page.md".into(),
            value: Some("1".into()),
        };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(classes.len(), 1);
        assert_eq!(classes[0].outcome, DriftOutcome::Healthy);
        assert_eq!(classes[0].target_path, "target.md");
    }

    #[test]
    fn head_source_skips_worktree_salvage_and_stays_fail_closed() {
        let (repo, c1) = repo_with_root_target();
        // The href edit is COMMITTED, so HEAD's page points at
        // `wiki/dir/target.md`, absent in HEAD. The blob layers have no
        // worktree filesystem to consult: salvage is skipped, and the move
        // scan finds the certified content still sitting at its own epoch
        // coordinates (excluded from relocation targets), so the edited
        // locator resolves to nothing → Broken — never blessed via the
        // worktree copy on disk.
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "[b](dir/target.md#L2-L4)\n", Some("1")),
        );
        repo.commit("href moved to a nonexistent directory");
        let epoch = LinkEpoch::Commit {
            sha: c1,
            path_at_commit: "wiki/page.md".into(),
            value: Some("1".into()),
        };
        let page_content = repo.read("wiki/page.md");
        let classes = classify_page(
            repo.path(),
            &reader(repo.path()),
            &NoopCache,
            DocSource::Head,
            "wiki/page.md",
            &page_content,
            &epoch,
            &mut MoveScanCtx::new(&DriftRunCtx::new()),
        )
        .expect("classifies");
        assert_eq!(classes.len(), 1);
        assert_eq!(classes[0].outcome, DriftOutcome::Broken);
        // The effective path stays the resolved href path — no salvage.
        assert_eq!(classes[0].target_path, "wiki/dir/target.md");
    }

    #[test]
    fn salvage_yields_to_a_resolved_target_that_exists() {
        let (repo, c1) = repo_with_root_target();
        // `wiki/dir/target.md` EXISTS with different content, so the direct
        // resolution wins and salvage never runs: the link points at real
        // content that is not the certified block, and the certified block
        // never moved → RangeDiffered (the hand-edited href no longer points
        // at the certified block; review required).
        repo.create_file("wiki/dir/target.md", "X0\nX1\nX2\nX3\nX4\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "[b](dir/target.md#L2-L4)\n", Some("1")),
        );
        let epoch = LinkEpoch::Commit {
            sha: c1,
            path_at_commit: "wiki/page.md".into(),
            value: Some("1".into()),
        };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(classes.len(), 1);
        assert_eq!(classes[0].outcome, DriftOutcome::RangeDiffered);
        assert_eq!(classes[0].target_path, "wiki/dir/target.md");
    }

    #[test]
    fn moved_when_content_shifted_within_target() {
        let (repo, c1) = repo_with_certified_link();
        repo.create_file(
            "wiki/target.md",
            "A1\nA2\nT0\nblock-line-1\nblock-line-2\nblock-line-3\nT1\n",
        );
        repo.commit("two lines inserted above the block");
        let epoch = LinkEpoch::Commit {
            sha: c1,
            path_at_commit: "wiki/page.md".into(),
            value: Some("1".into()),
        };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(
            classes[0].outcome,
            DriftOutcome::Moved {
                new_path: "wiki/target.md".into(),
                new_start: 4,
                new_end: 6,
                content_identical: true
            }
        );
    }

    #[test]
    fn moved_cross_file_rewrites_path_and_range() {
        // Cross-file Moved arises when the target is gone and the destination
        // carries identity evidence — here a committed rename (git mv), which
        // the evidence rule requires: a content-only copy in an unrelated
        // file never relocates. A merely truncated target is Broken per the
        // card — see `broken_when_extent_overhangs_target`.
        let (repo, c1) = repo_with_certified_link();
        repo.rename_file("wiki/target.md", "wiki/other.md");
        repo.commit("block moved to other.md");
        let epoch = LinkEpoch::Commit {
            sha: c1,
            path_at_commit: "wiki/page.md".into(),
            value: Some("1".into()),
        };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(
            classes[0].outcome,
            DriftOutcome::Moved {
                new_path: "wiki/other.md".into(),
                new_start: 2,
                new_end: 4,
                content_identical: true
            }
        );
    }

    #[test]
    fn unknown_when_content_is_duplicated() {
        let (repo, c1) = repo_with_certified_link();
        repo.create_file(
            "wiki/target.md",
            &format!("T0\nchanged\nX\n{BLOCK}\nY\n{BLOCK}\nZ\n"),
        );
        repo.commit("certified block now occurs twice");
        let epoch = LinkEpoch::Commit {
            sha: c1,
            path_at_commit: "wiki/page.md".into(),
            value: Some("1".into()),
        };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(classes[0].outcome, DriftOutcome::Unknown);
    }

    #[test]
    fn deleted_duplicate_classifies_unknown_label_deleted() {
        // Round-2 drift-message-honesty: a duplicate with the same display
        // text deleted since the epoch leaves the survivor unpaired. Content
        // identity resolves the pairing FIRST — only when the current
        // locator's content matches NO same-label candidate's certified
        // block does the pairing fail closed, as here (the locator cites
        // content that was never certified).
        let repo = TestRepo::new();
        repo.create_file(
            "wiki/target.md",
            "T0\nblock-line-1\nblock-line-2\nblock-line-3\nT1\n\
             block-line-1\nblock-line-2\nblock-line-3\nT2\n",
        );
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page(
                "Page",
                "[b](target.md#L2-L4)\n[b](target.md#L6-L8)\n",
                Some("1"),
            ),
        );
        let c1 = repo.commit("certify two same-display-text links");
        // The first link is deleted; the survivor is re-pointed to content
        // matching no candidate's certified block.
        repo.create_file(
            "wiki/target.md",
            "T0\nblock-line-1\nblock-line-2\nblock-line-3\nT1\n\
             block-line-1\nblock-line-2\nblock-line-3\nT2\nX0\nX1\nX2\n",
        );
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "[b](target.md#L10-L12)\n", Some("1")),
        );
        repo.commit("delete duplicate, re-point to uncertified content");
        let epoch = LinkEpoch::Commit {
            sha: c1,
            path_at_commit: "wiki/page.md".into(),
            value: Some("1".into()),
        };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(classes.len(), 1);
        assert_eq!(classes[0].outcome, DriftOutcome::UnknownLabelDeleted);
    }

    #[test]
    fn repointed_duplicate_resolves_healthy_via_content_identity() {
        // The relocated-but-healthy carve-out generalized to the ambiguous-
        // pairing case: the survivor is re-pointed to its block's new
        // location (coordinates that never existed at the epoch), and the
        // current content equals a same-label candidate's certified block —
        // the pairing is resolved by content identity, not a guess.
        let repo = TestRepo::new();
        repo.create_file(
            "wiki/target.md",
            "T0\nblock-line-1\nblock-line-2\nblock-line-3\nT1\n\
             block-line-1\nblock-line-2\nblock-line-3\nT2\n",
        );
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page(
                "Page",
                "[b](target.md#L2-L4)\n[b](target.md#L6-L8)\n",
                Some("1"),
            ),
        );
        let c1 = repo.commit("certify two same-display-text links");
        // The first link is deleted and the target shifts down two lines;
        // the survivor is re-pointed to follow its block (L6-L8 -> L8-L10).
        repo.create_file(
            "wiki/target.md",
            "A\nB\nT0\nblock-line-1\nblock-line-2\nblock-line-3\nT1\n\
             block-line-1\nblock-line-2\nblock-line-3\nT2\n",
        );
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "[b](target.md#L8-L10)\n", Some("1")),
        );
        repo.commit("delete duplicate, block shifted down");
        let epoch = LinkEpoch::Commit {
            sha: c1,
            path_at_commit: "wiki/page.md".into(),
            value: Some("1".into()),
        };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(classes.len(), 1);
        assert_eq!(classes[0].outcome, DriftOutcome::Healthy);
    }

    // ── Fuzzy Jaccard tier (Phase 1d, P2 acceptance checks — all pending) ──

    #[test]
    fn jaccard_identical_groups_are_one() {
        let lines: Vec<&str> = FUZZY_BLOCK.lines().collect();
        assert_eq!(window_jaccard(&lines, &lines), 1.0);
    }

    #[test]
    fn jaccard_disjoint_groups_are_zero() {
        let a: Vec<&str> = vec!["alpha beta gamma"];
        let b: Vec<&str> = vec!["delta epsilon zeta"];
        assert_eq!(window_jaccard(&a, &b), 0.0);
    }

    #[test]
    fn jaccard_duplicate_tokens_weight_the_multiset() {
        // intersection = 1·a + 1·b = 2; union = 3 + 3 − 2 = 4.
        let a: Vec<&str> = vec!["a a b"];
        let b: Vec<&str> = vec!["a b b"];
        assert_eq!(window_jaccard(&a, &b), 0.5);
    }

    #[test]
    fn jaccard_empty_groups_are_zero() {
        assert_eq!(window_jaccard(&[], &[]), 0.0);
        let b: Vec<&str> = vec!["x"];
        assert_eq!(window_jaccard(&[], &b), 0.0);
    }

    #[test]
    fn fuzzy_window_score_rejects_overlap_windows_accepts_the_block() {
        // The design contract behind the containment weighting: a window
        // made of the moved block's own lines (one line edited) reaches the
        // threshold, while every overlapping window that includes filler
        // lines stays below it — sliding-window neighbors must not turn a
        // single move into an ambiguous multi-match.
        let cert: Vec<&str> = FUZZY_BLOCK.lines().collect();
        let edited: Vec<&str> = FUZZY_BLOCK_EDITED.lines().collect();
        assert!(fuzzy_window_score(&cert, &edited) >= FUZZY_JACCARD_THRESHOLD);

        // The strongest overlap window: one filler line plus five block lines.
        let overlap: Vec<&str> = ["A3"]
            .iter()
            .copied()
            .chain(FUZZY_BLOCK_EDITED.lines().take(5))
            .collect();
        assert!(
            fuzzy_window_score(&cert, &overlap) < FUZZY_JACCARD_THRESHOLD,
            "overlap window must stay below the threshold"
        );
    }

    #[test]
    fn fuzzy_window_score_three_line_whole_edit_stays_below() {
        // Replacing one of three single-token lines keeps 2/4 of the token
        // multiset shared; the containment weighting leaves the score well
        // under 0.7. The tier deliberately misses short-block whole-line
        // edits — a false Moved relocates a link, a false Drift just
        // re-certifies.
        let a: Vec<&str> = vec!["l1", "l2", "l3"];
        let b: Vec<&str> = vec!["l1", "X", "l3"];
        assert!(fuzzy_window_score(&a, &b) < FUZZY_JACCARD_THRESHOLD);
    }

    #[test]
    fn fuzzy_threshold_separates_real_content() {
        // The threshold's tuning evidence: real wiki markdown and code from
        // this corpus. Moved-and-lightly-edited excerpts must score at or
        // above the threshold; unrelated excerpts must stay below it.
        let a: Vec<&str> = REAL_COLLECT_WITH_SOURCE.lines().collect();
        let b: Vec<&str> = REAL_COLLECT_WITH_SOURCE_EDITED.lines().collect();
        assert!(
            fuzzy_window_score(&a, &b) >= FUZZY_JACCARD_THRESHOLD,
            "real code with one comment word changed must reach the tier"
        );
        let a: Vec<&str> = REAL_WIKI_DRIFT_PARAGRAPH.lines().collect();
        let b: Vec<&str> = REAL_WIKI_DRIFT_PARAGRAPH_EDITED.lines().collect();
        assert!(
            fuzzy_window_score(&a, &b) >= FUZZY_JACCARD_THRESHOLD,
            "real wiki prose with one phrase swapped must reach the tier"
        );

        let a: Vec<&str> = REAL_WIKI_DRIFT_PARAGRAPH.lines().collect();
        let b: Vec<&str> = REAL_WIKIIGNORE_PARAGRAPH.lines().collect();
        assert!(
            fuzzy_window_score(&a, &b) < FUZZY_JACCARD_THRESHOLD,
            "unrelated wiki paragraphs must stay below the tier"
        );
        let a: Vec<&str> = REAL_COLLECT_WITH_SOURCE.lines().collect();
        let b: Vec<&str> = "pub fn insert_links_reviewed(content: &str) -> Option<String> {\n    if !frontmatter::has_wiki_frontmatter(content) {\n        return None;\n    }\n    let (yaml_start, yaml_end, _) = frontmatter::yaml_block_bounds(content)?;"
            .lines()
            .collect();
        assert!(
            fuzzy_window_score(&a, &b) < FUZZY_JACCARD_THRESHOLD,
            "unrelated code blocks must stay below the tier"
        );
    }

    /// The fuzzy fixture: a certified link `[b](target.md#L2-L7)` whose range
    /// covers `FUZZY_BLOCK`.
    fn repo_with_certified_fuzzy_block() -> (TestRepo, String) {
        let repo = TestRepo::new();
        repo.create_file("wiki/target.md", &format!("T0\n{FUZZY_BLOCK}\nT1\n"));
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "[b](target.md#L2-L7)\n", Some("1")),
        );
        let c1 = repo.commit("certified page and target");
        (repo, c1)
    }

    #[test]
    fn fuzzy_moved_within_target_after_small_edit() {
        let (repo, c1) = repo_with_certified_fuzzy_block();
        // The edited block moves down three lines; the href range L2-L7 now
        // covers unrelated filler, so both exact tiers find nothing and the
        // fuzzy tier must relocate the link to the edited block.
        repo.create_file(
            "wiki/target.md",
            &format!("T0\nA1\nA2\nA3\n{FUZZY_BLOCK_EDITED}\nT1\n"),
        );
        repo.commit("block edited and moved down");
        let epoch = LinkEpoch::Commit {
            sha: c1,
            path_at_commit: "wiki/page.md".into(),
            value: Some("1".into()),
        };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(
            classes[0].outcome,
            DriftOutcome::Moved {
                new_path: "wiki/target.md".into(),
                new_start: 5,
                new_end: 10,
                content_identical: false
            }
        );
    }

    #[test]
    fn in_place_edit_is_drift_not_a_self_move() {
        let (repo, c1) = repo_with_certified_fuzzy_block();
        // The certified block is lightly edited IN PLACE: the href range still
        // covers it, so nothing moved. The fuzzy tier's match at the link's
        // own coordinates must not be treated as a relocation — `--fix` would
        // rewrite the href to itself on every run. In-place drift classifies
        // Drift, with the bump-`links-reviewed:` remedy.
        repo.create_file("wiki/target.md", &format!("T0\n{FUZZY_BLOCK_EDITED}\nT1\n"));
        repo.commit("block edited in place");
        let epoch = LinkEpoch::Commit {
            sha: c1,
            path_at_commit: "wiki/page.md".into(),
            value: Some("1".into()),
        };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(classes[0].outcome, DriftOutcome::Drift);
    }

    #[test]
    fn fuzzy_move_into_a_file_that_is_not_valid_utf8() {
        // The destination's surrounding lines are binary, and the decoder
        // replaces them wholesale — but the block's own lines are intact, so
        // the relocation must still be found and rewritten.
        let (repo, c1) = repo_with_certified_fuzzy_block();
        repo.rename_file("wiki/target.md", "wiki/other.md");
        let mut dest = b"\x89PNG\r\n\x1a\n\xff\x00\xfe\n".to_vec();
        dest.extend_from_slice(format!("H\n{FUZZY_BLOCK_EDITED}\nF\n").as_bytes());
        dest.extend_from_slice(b"\x00\xff\xc3\x28");
        repo.create_file_bytes("wiki/other.md", &dest);
        repo.commit("block edited and moved into a file that is not valid UTF-8");
        let epoch = LinkEpoch::Commit {
            sha: c1,
            path_at_commit: "wiki/page.md".into(),
            value: Some("1".into()),
        };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(
            classes[0].outcome,
            DriftOutcome::Moved {
                new_path: "wiki/other.md".into(),
                new_start: 5,
                new_end: 10,
                content_identical: false
            }
        );
    }

    #[test]
    fn fuzzy_moved_cross_file_after_small_edit() {
        // Cross-file Moved arises when the target is gone; the edited block
        // lives in another file, and the fuzzy tier is what finds it. The
        // destination carries identity evidence — the committed rename.
        let (repo, c1) = repo_with_certified_fuzzy_block();
        repo.rename_file("wiki/target.md", "wiki/other.md");
        repo.create_file("wiki/other.md", &format!("H\n{FUZZY_BLOCK_EDITED}\nF\n"));
        repo.commit("block edited and moved to other.md");
        let epoch = LinkEpoch::Commit {
            sha: c1,
            path_at_commit: "wiki/page.md".into(),
            value: Some("1".into()),
        };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(
            classes[0].outcome,
            DriftOutcome::Moved {
                new_path: "wiki/other.md".into(),
                new_start: 2,
                new_end: 7,
                content_identical: false
            }
        );
    }

    #[test]
    fn fuzzy_two_candidates_is_unknown() {
        // The edited block appears twice within the target: two at-threshold
        // windows → Unknown, per the card's unconditional multi-match rule.
        // (Cross-file duplication is never ambiguous: a content-only match in
        // an unrelated file carries no identity evidence and is dropped.)
        let (repo, c1) = repo_with_certified_fuzzy_block();
        repo.create_file(
            "wiki/target.md",
            &format!("T0\nA1\nA2\nA3\n{FUZZY_BLOCK_EDITED}\nX\n{FUZZY_BLOCK_EDITED}\nT1\n"),
        );
        repo.commit("edited block duplicated within the target");
        let epoch = LinkEpoch::Commit {
            sha: c1,
            path_at_commit: "wiki/page.md".into(),
            value: Some("1".into()),
        };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(classes[0].outcome, DriftOutcome::Unknown);
    }

    #[test]
    fn fuzzy_no_match_stays_drift() {
        let (repo, c1) = repo_with_certified_fuzzy_block();
        // The certified range now covers unrelated filler and nothing in the
        // repo resembles the block → the tier's zero-matches outcome (Drift
        // on the range-equal path).
        repo.create_file(
            "wiki/target.md",
            "T0\nX1\nX2\nX3\nX4\nX5\nX6\nT1\n",
        );
        repo.commit("range content replaced by unrelated lines");
        let epoch = LinkEpoch::Commit {
            sha: c1,
            path_at_commit: "wiki/page.md".into(),
            value: Some("1".into()),
        };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(classes[0].outcome, DriftOutcome::Drift);
    }

    #[test]
    fn fuzzy_no_match_missing_target_stays_broken() {
        let (repo, c1) = repo_with_certified_fuzzy_block();
        // Target deleted and nothing resembles the block → Broken on the
        // missing-target path.
        repo.remove_file("wiki/target.md");
        repo.create_file("wiki/unrelated.md", "Y0\nY1\nY2\nY3\nY4\nY5\nY6\nY7\n");
        repo.commit("target deleted, nothing similar remains");
        let epoch = LinkEpoch::Commit {
            sha: c1,
            path_at_commit: "wiki/page.md".into(),
            value: Some("1".into()),
        };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(classes[0].outcome, DriftOutcome::Broken);
    }

    /// The raw tokenizer must count and slice exactly what
    /// `str::split_whitespace` sees in the decoded text — the whole tier's
    /// soundness rests on those being the same tokens. The shapes reach every
    /// branch of the separator table, including the characters on either side
    /// of the `White_Space` boundary and the sequences error recovery splits.
    #[test]
    fn raw_tokenizer_agrees_with_the_decoded_split() {
        let shapes: Vec<Vec<u8>> = [
            "a b\tc\nd",
            "a\u{85}b", // NEL
            "a\u{a0}b", // NBSP
            "a\u{1680}b",
            "a\u{2000}b\u{200a}c",
            "a\u{200b}b", // zero width space: not a separator
            "a\u{2028}b\u{2029}c",
            "a\u{202f}b\u{205f}c\u{3000}d",
            "a\u{2060}b", // word joiner: not a separator
            "\u{a0}\u{3000}", // separators only, no tokens
            "a\u{00ad}b", // soft hyphen: not a separator
            "a\r\nb",
            "a\rb",
            "\r",
        ]
        .map(|s| s.as_bytes().to_vec())
        .into_iter()
        .chain([
            b"a\xff\xfe b".to_vec(),
            b"\xc2\x85".to_vec(),     // a bare NEL
            b"\xe2\x80".to_vec(),     // a truncated sequence
            b"\xe2\x80\x0a".to_vec(), // truncated, then a newline
            b"\xc0\x85".to_vec(),     // an overlong encoding
            b"\xed\xa0\x80".to_vec(), // a surrogate
            b"\xf0\x9f\x8c\x8a".to_vec(),
            b"".to_vec(),
            b"\n\n".to_vec(),
            b"a".to_vec(),
        ])
        .collect();
        for bytes in &shapes {
            let decoded = String::from_utf8_lossy(bytes);
            let expected: Vec<String> = decoded
                .split_whitespace()
                .map(str::to_string)
                .collect();
            let got: Vec<String> = Runs::new(bytes)
                .map(|(start, end)| String::from_utf8_lossy(&bytes[start..end]).into_owned())
                .collect();
            assert_eq!(got, expected, "{bytes:?}");
            assert_eq!(token_count(bytes), expected.len() as u32, "{bytes:?}");

            // The index stores those ranges instead of re-walking the bytes
            // per scan, so they must slice out exactly the decoded tokens, in
            // order, with the same counts — the property every scan and every
            // window's token total now rests on.
            let lines = FuzzyLines::build(bytes);
            let indexed: Vec<String> = (0..lines.line_count())
                .flat_map(|i| {
                    let raw = lines.line(bytes, i);
                    lines
                        .line_runs(i)
                        .iter()
                        .map(|&(s, e)| {
                            String::from_utf8_lossy(&raw[s as usize..e as usize]).into_owned()
                        })
                        .collect::<Vec<String>>()
                })
                .collect();
            assert_eq!(indexed, expected, "{bytes:?}");
            for i in 0..lines.line_count() {
                assert_eq!(
                    lines.line_runs(i).len() as u32,
                    lines.tokens[i],
                    "line {i} of {bytes:?} counts and slices different tokens",
                );
            }
        }
    }

    /// Candidate shapes the tier's fast paths have to reason about: line
    /// endings the canonical form rewrites, bytes that are not UTF-8 at all,
    /// separators the decoder does not spell in ASCII, no trailing newline,
    /// and a certified block whose own tokens carry replacement characters —
    /// the one case that puts the collector back on the decoder.
    fn fuzzy_shape_fixtures() -> Vec<(&'static str, String, Vec<u8>)> {
        let filler = "// filler sharing the block's vocabulary without being it\n";
        let block = format!("{filler}{FUZZY_BLOCK}\n{FUZZY_BLOCK_EDITED}\n");
        let blank_line_block = format!("{}\n\n{}\n", FUZZY_BLOCK, FUZZY_BLOCK_EDITED);
        // The block's bytes with a non-ASCII separator where the decoder
        // agrees it is whitespace, and with one it does not (U+2020).
        let wide_separators = FUZZY_BLOCK.replace(" -> ", "\u{a0}->\u{2000}");
        // A block the decoder had to rewrite: its tokens carry replacement
        // characters, so no raw run of a candidate can be compared to them.
        let lossy_block = String::from_utf8_lossy(b"alpha \xff\xfe beta\ngamma \x80 delta").into_owned();
        vec![
            ("plain", FUZZY_BLOCK.into(), block.clone().into_bytes()),
            (
                "crlf line endings",
                FUZZY_BLOCK.into(),
                block.replace('\n', "\r\n").into_bytes(),
            ),
            (
                "no trailing newline",
                FUZZY_BLOCK.into(),
                block.trim_end_matches('\n').as_bytes().to_vec(),
            ),
            (
                "blank certified line",
                blank_line_block.clone(),
                blank_line_block.replace('\n', "\n\n").into_bytes(),
            ),
            (
                "non-ASCII separators",
                wide_separators.clone(),
                format!("x\n{wide_separators}\ny\n").into_bytes(),
            ),
            (
                "binary around the block",
                FUZZY_BLOCK.into(),
                [
                    b"\x89PNG\r\n\x1a\n\xff\x00\xfe\n".as_slice(),
                    block.as_bytes(),
                    b"\x00\xff\xc3\x28".as_slice(),
                ]
                .concat(),
            ),
            (
                "block whose tokens carry replacement characters",
                lossy_block.clone(),
                [b"head \xf0\x9f\n".as_slice(), lossy_block.as_bytes(), b"\nc3\n".as_slice()].concat(),
            ),
            (
                "shapes with no hit at all",
                FUZZY_BLOCK.into(),
                b"   \n\n\t\n \n \x89PNG\x0d\x0a\xff\x00\x7f\n".to_vec(),
            ),
        ]
    }

    /// The byte-level line match must agree with the decoded computation, line
    /// for line: for every line of every shape, the fast path reports a match
    /// exactly when the decoded line's token multiset reaches the line
    /// threshold against some certified line. The count gate is part of the
    /// fast path under test — a line it rejects is one the oracle must reject
    /// too — and so is the replacement-character branch.
    #[test]
    fn fuzzy_line_match_agrees_with_the_decoded_oracle_line_for_line() {
        for (name, cert_block, bytes) in fuzzy_shape_fixtures() {
            let cert_lines: Vec<&str> = cert_block.lines().collect();
            let block = CertBlock::of(&cert_lines);
            let lines = FuzzyLines::build(bytes.as_slice());
            let mut runs = Vec::new();
            for i in 0..lines.line_count() {
                let text = String::from_utf8_lossy(lines.line(&bytes, i));
                let blank = text.split_whitespace().next().is_none();
                // The oracle's rule, as `fuzzy_window_score` applies it to a
                // one-line candidate against the block.
                let oracle = if blank {
                    cert_lines.iter().any(|l| l.split_whitespace().next().is_none())
                } else {
                    cert_lines.iter().any(|cert| {
                        !cert.split_whitespace().next().is_none()
                            && window_jaccard(&[text.as_ref()], &[cert])
                                >= FUZZY_LINE_MATCH_THRESHOLD
                    })
                };
                let optimized = line_flag(&bytes, &lines, i, &block, &mut runs);
                assert_eq!(optimized, oracle, "{name}: line {i} of {:?}", lines.line(&bytes, i));
            }
        }
    }

    /// Differential test: the optimized sliding collector must agree with the
    /// brute-force per-window formula on *every* window of every shape — the
    /// count gate, the byte-level line match and the matched-count floor must
    /// never skip a window the brute force would admit, nor admit one it would
    /// not. The oracle decodes every line it reads, so this pins the whole
    /// pipeline to the decoded semantics the corpus's hits were computed under.
    #[test]
    fn fuzzy_collector_agrees_with_brute_force_on_every_window() {
        for (name, cert_block, bytes) in fuzzy_shape_fixtures() {
            let cert_lines: Vec<&str> = cert_block.lines().collect();
            let span = cert_lines.len();
            let block = CertBlock::of(&cert_lines);
            let mut optimized = Vec::new();
            collect_fuzzy_hits(
                &bytes,
                &FuzzyLines::build(&bytes),
                "candidate.rs",
                &block,
                span,
                &mut optimized,
            );

            let decoded = String::from_utf8_lossy(&bytes);
            let lines: Vec<&str> = decoded.lines().collect();
            let mut brute: Vec<(u32, u32)> = Vec::new();
            if lines.len() >= span {
                for start in 0..=lines.len() - span {
                    let window = &lines[start..start + span];
                    if fuzzy_window_score(&cert_lines, window) >= FUZZY_JACCARD_THRESHOLD {
                        brute.push(((start + 1) as u32, (start + span) as u32));
                    }
                }
            }
            let optimized_ranges: Vec<(u32, u32)> = optimized
                .iter()
                .map(|l| (l.start_line, l.end_line))
                .collect();
            assert_eq!(optimized_ranges, brute, "{name}");
        }
    }

    /// The differential fixtures must actually exercise what they claim, or
    /// the comparison above is vacuous.
    #[test]
    fn fuzzy_shape_fixtures_exercise_the_fast_paths() {
        let mut binary_lines = 0usize;
        let mut non_ascii_separators = 0usize;
        let mut replacement_characters = 0usize;
        let mut hits = 0usize;
        for (_, cert_block, bytes) in fuzzy_shape_fixtures() {
            let cert_lines: Vec<&str> = cert_block.lines().collect();
            let block = CertBlock::of(&cert_lines);
            let lines = FuzzyLines::build(&bytes);
            let mut runs = Vec::new();
            for i in 0..lines.line_count() {
                let raw = lines.line(&bytes, i);
                binary_lines += usize::from(std::str::from_utf8(raw).is_err());
                non_ascii_separators += usize::from(
                    Runs::new(raw).any(|(s, _)| raw[s] >= 0x80),
                );
                replacement_characters +=
                    usize::from(String::from_utf8_lossy(raw).contains('\u{fffd}'));
                hits += usize::from(line_flag(&bytes, &lines, i, &block, &mut runs));
            }
        }
        assert!(binary_lines > 0, "no fixture line is invalid UTF-8");
        assert!(non_ascii_separators > 0, "no fixture separates tokens with a non-ASCII character");
        assert!(replacement_characters > 0, "no fixture line decodes to a replacement character");
        assert!(hits > 0, "no fixture line matches its certified block");
    }

    /// [`parallel_scan`] must be indistinguishable from the serial loop it
    /// replaces: same hits, in the same inventory order, for every inventory
    /// size — including sizes that straddle the worker and chunk boundaries,
    /// where an off-by-one in the chunking would drop or duplicate a chunk.
    #[test]
    fn parallel_scan_matches_the_serial_scan_in_order() {
        let items: Vec<(String, Vec<u8>)> = (0..37)
            .map(|i| {
                let body = if i % 3 == 0 {
                    format!("hit {i}\n")
                } else {
                    format!("miss {i}\n")
                };
                (format!("file{i}.txt"), body.into_bytes())
            })
            .collect();
        let refs: Vec<(&str, &[u8])> = items
            .iter()
            .map(|(p, b)| (p.as_str(), b.as_slice()))
            .collect();

        // The serial reference: the same predicate, run in inventory order.
        let mut serial = Vec::new();
        for item in &refs {
            parallel_scan_probe(item, &mut serial);
        }
        let parallel = parallel_scan(&refs, parallel_scan_probe);
        assert_eq!(parallel, serial);
        assert_eq!(parallel.len(), 13, "every third file holds one hit");
    }

    /// The probe [`parallel_scan_matches_the_serial_scan_in_order`] drives
    /// through both paths: one hit for every line that reads `hit`.
    fn parallel_scan_probe(item: &(&str, &[u8]), out: &mut Vec<crate::rk64::Location>) {
        let (path, bytes) = item;
        for (i, line) in String::from_utf8_lossy(bytes).lines().enumerate() {
            if line.starts_with("hit") {
                out.push(crate::rk64::Location {
                    path: (*path).to_string(),
                    start_line: (i + 1) as u32,
                    end_line: (i + 1) as u32,
                });
            }
        }
    }

    /// The prefilter floor admits exactly the windows whose containment can
    /// reach the tier: every matched count at or above the floor satisfies
    /// containment ≥ T, and the one below it does not — so the optimized
    /// collector can never skip a window the brute-force formula admits.
    #[test]
    fn fuzzy_collector_floor_bounds_containment_exactly() {
        for span in 1..40usize {
            let floor = ((2.0 * span as f64 * FUZZY_JACCARD_THRESHOLD)
                / (1.0 + FUZZY_JACCARD_THRESHOLD))
                .ceil() as usize;
            for matched in floor..=span {
                let containment = matched as f64 / (2.0 * span as f64 - matched as f64);
                assert!(
                    containment >= FUZZY_JACCARD_THRESHOLD,
                    "span {span} matched {matched}"
                );
            }
            if floor > 0 {
                let below = floor - 1;
                let containment = below as f64 / (2.0 * span as f64 - below as f64);
                assert!(
                    containment < FUZZY_JACCARD_THRESHOLD,
                    "span {span} matched {below}"
                );
            }
        }
    }

    #[test]
    fn pending_bump_suppresses_certification_but_flags_broken() {
        let (repo, _c1) = repo_with_certified_link();
        repo.create_file("wiki/other.md", &format!("H\n{BLOCK}\nF\n"));
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
        repo.create_file(
            "wiki/target.md",
            "T0\nblock-line-1\nblock-line-2\nCHANGED\nT1\n",
        );
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page(
                "Page",
                "[b](target.md#L2-L4)\n[gone](gone.md#L1-L1)\n",
                Some("2"),
            ),
        );
        let epoch = LinkEpoch::Current {
            value: Some("2".into()),
        };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(classes.len(), 2);
        assert_eq!(
            classes[0].outcome,
            DriftOutcome::Healthy,
            "certification outcomes are suppressed under a pending bump"
        );
        assert_eq!(
            classes[1].outcome,
            DriftOutcome::Broken,
            "structural failures still flag under a pending bump"
        );
    }

    #[test]
    fn missing_epoch_is_rejected_fail_closed() {
        let (repo, _c1) = repo_with_certified_link();
        let err = classify(&repo, &LinkEpoch::Missing, "wiki/page.md").expect_err("fails closed");
        assert!(matches!(err, EpochError::MissingEpoch), "got {err:?}");
    }

    #[test]
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
        let epoch = LinkEpoch::Commit {
            sha: c1,
            path_at_commit: "wiki/page.md".into(),
            value: Some("1".into()),
        };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(classes[0].outcome, DriftOutcome::Healthy);
    }

    #[test]
    fn href_edited_to_different_content_is_range_differed() {
        // The link's epoch record still matches (same label), and the edited
        // locator resolves to a range that fits — but the range differs from
        // every certified range, the content there is not the certified
        // block, the certified block never moved, and the page at the epoch
        // DID review this link → RangeDiffered (review required), never
        // Uncertified for a reviewed link.
        let (repo, c1) = repo_with_certified_link();
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "[b](target.md#L1-L1)\n", Some("1")),
        );
        repo.commit("href now points at different content");
        let epoch = LinkEpoch::Commit {
            sha: c1,
            path_at_commit: "wiki/page.md".into(),
            value: Some("1".into()),
        };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(classes[0].outcome, DriftOutcome::RangeDiffered);
    }

    #[test]
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
        let epoch = LinkEpoch::Commit {
            sha: c1,
            path_at_commit: "wiki/page.md".into(),
            value: Some("1".into()),
        };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(classes.len(), 1, "only line-range links are classified");
        assert_eq!(classes[0].target_path, "wiki/target.md");
        assert_eq!(classes[0].start_line, 2);
        assert_eq!(classes[0].end_line, 4);
    }

    #[test]
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
        let epoch = LinkEpoch::Commit {
            sha: c1,
            path_at_commit: "wiki/page.md".into(),
            value: Some("1".into()),
        };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(classes[0].outcome, DriftOutcome::Healthy);
        assert_eq!(classes[1].outcome, DriftOutcome::Uncertified);
    }

    #[test]
    fn cross_file_relocation_rerun_is_healthy_via_the_relocation_clause() {
        // The amendment scenario: --fix relocates a cross-file move (new path
        // AND range), and the next run must NOT flag its own rewrite. The
        // target is deleted — cross-file Moved requires a missing target;
        // a truncated one is Broken per the card.
        let (repo, c1) = repo_with_certified_link();
        repo.rename_file("wiki/target.md", "wiki/other.md");
        repo.commit("block moved to other.md");

        let epoch = LinkEpoch::Commit {
            sha: c1,
            path_at_commit: "wiki/page.md".into(),
            value: Some("1".into()),
        };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(
            classes[0].outcome,
            DriftOutcome::Moved {
                new_path: "wiki/other.md".into(),
                new_start: 2,
                new_end: 4,
                content_identical: true
            }
        );

        // Apply the fix the way --fix would: rewrite the full href.
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "[b](other.md#L2-L4)\n", Some("1")),
        );
        repo.commit("fix applied, field untouched");
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(
            classes[0].outcome,
            DriftOutcome::Healthy,
            "the relocation carve-out keeps the tool's own rewrite certified"
        );
    }

    // ── Out-of-scope links are never classified; page-level identity ──

    #[test]
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
        let epoch = LinkEpoch::Commit {
            sha: c1,
            path_at_commit: "wiki/page.md".into(),
            value: Some("1".into()),
        };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(classes[0].outcome, DriftOutcome::Healthy);
        // `source_line` reflects the CURRENT page content: frontmatter spans
        // lines 1–5, the inserted heading line 6, the blank line 7, the link
        // line 8 — identity is label/range-based, never position-based.
        assert_eq!(classes[0].source_line, 8);
    }

    // ── Fingerprint tier: the three-valued availability probe (Phase 2) ──

    /// The pure three-valued mapping of a completed `git ls-tree` output
    /// (plan decision 1), pinned directly — including the unknown branch the
    /// CLI cannot reach: a destroyed tree kills the page's `--follow` walk
    /// before the fingerprint tier or the probe ever engages (the P2 check
    /// `unreadable_target_tree_fails_closed_without_fingerprint_rows` pins
    /// the CLI-level observables; this pins the probe's own mapping, which
    /// is where the discrimination lives).
    #[test]
    fn ls_tree_availability_mapping_is_three_valued() {
        let ok = Command::new("true").output().expect("true").status;
        let fail = Command::new("false").output().expect("false").status;
        let output = |status: std::process::ExitStatus, stdout: &[u8]| std::process::Output {
            status,
            stdout: stdout.to_vec(),
            stderr: Vec::new(),
        };
        let listing = b"100644 blob 8ab686eafeb1f44702738c8b0f24f2567c36da6d\tdocs/target.md\n";
        assert_eq!(
            classify_ls_tree_output(&output(ok, listing)),
            Availability::Present
        );
        assert_eq!(
            classify_ls_tree_output(&output(ok, b"")),
            Availability::Absent
        );
        assert_eq!(
            classify_ls_tree_output(&output(fail, b"")),
            Availability::Unknown,
            "a nonzero exit is unknown, never absent"
        );
        assert_eq!(
            classify_ls_tree_output(&output(fail, listing)),
            Availability::Unknown,
            "a listing on a failed exit is still unknown — the exit code rules"
        );
        // The `not a tree object` shape the plan names: exit 128. Treated as
        // a plain nonzero — special-casing 128 as absence is exactly the
        // two-valued unsoundness the third value closes.
        let status_128 = std::os::unix::process::ExitStatusExt::from_raw(128 << 8);
        assert_eq!(
            classify_ls_tree_output(&output(status_128, b"")),
            Availability::Unknown,
            "exit 128 (unreadable tree) is unknown, not absent"
        );
    }

    /// The probe against a real repository: present, absent, and unknown —
    /// the unknown branch via a physically destroyed tree object. Pins the
    /// plan's claim that `git ls-tree` completes with an empty listing for
    /// a genuinely absent path and fails for an unreadable tree.
    #[test]
    fn availability_probe_on_a_real_repo_is_three_valued() {
        let repo = TestRepo::new();
        repo.create_file("docs/target.md", "target content\n");
        repo.commit("certify");
        let sha = repo.git(&["rev-parse", "HEAD"]);
        let target = "docs/target.md";

        assert_eq!(
            availability_probe(repo.path(), &sha, target),
            Availability::Present,
            "a readable tree with the entry is present"
        );
        assert_eq!(
            availability_probe(repo.path(), &sha, "docs/nope.md"),
            Availability::Absent,
            "a completed tree read with no entry is absent"
        );

        // Destroy the tree object containing the target (loose object).
        // `git ls-tree <sha>` then cannot read it — the unknown branch.
        let tree_sha = repo.git(&["rev-parse", &format!("{sha}:docs")]);
        let obj = repo
            .path()
            .join(".git/objects")
            .join(&tree_sha[..2])
            .join(&tree_sha[2..]);
        fs::remove_file(&obj).expect("remove tree object");
        assert_eq!(
            availability_probe(repo.path(), &sha, target),
            Availability::Unknown,
            "an unreadable tree is unknown — neither a listing nor absence"
        );
    }
}

