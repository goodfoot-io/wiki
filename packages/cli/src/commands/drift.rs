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

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use thiserror::Error;

use crate::frontmatter::{self, scalar_to_string};
use crate::index::DocSource;
use crate::rk64::{
    Extent, LineIndex, cheap_fingerprint_with_extent, scan_for_content_hash_rk64, scan_indexed_rk64,
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
/// coerced to its string form. Returns `None` when the page has no leading
/// `---` YAML block, the field is absent, or its value is not a scalar.
/// Change detection compares these strings, so any later value change
/// re-certifies the page.
pub fn extract_links_reviewed(content: &str) -> Option<String> {
    let (yaml_start, yaml_end, _) = frontmatter::yaml_block_bounds(content)?;
    let yaml = &content[yaml_start..yaml_end];
    let parsed: serde_yaml::Value = serde_yaml::from_str(yaml).ok()?;
    let value = parsed.get("links-reviewed")?;
    scalar_to_string(value)
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
    if current_value != committed_value {
        // Pending-certification rule (Decision 2): a field bump, addition, or
        // removal that is not yet committed makes the anchor epoch the current
        // state itself.
        return Ok(LinkEpoch::Current {
            value: current_value.map(str::to_owned),
        });
    }
    let Some(_committed) = committed_value else {
        // Both sides absent — the page has no anchor epoch.
        return Ok(LinkEpoch::Missing);
    };
    walk_anchor_epoch(repo_root, page_path)
}

/// Full-ancestry walk per plan Decision 3: `git log --follow --name-status
/// --format=%H -- <page>` — no commit cap, no `--first-parent`, so a
/// certification made only on a feature branch survives a non-squash merge.
/// The page's per-commit name is tracked through `R###` rename rows (any
/// similarity suffix is a rename). The anchor is the newer commit of the
/// first adjacent pair (newest→oldest) whose parsed field values differ;
/// when no pair differs the field was introduced at the oldest walked
/// commit, which is the anchor.
fn walk_anchor_epoch(repo_root: &Path, page_path: &str) -> Result<LinkEpoch, EpochError> {
    // The repository state is the authority: a local-path `--depth 1` clone
    // can silently copy full history, so clone flags must not be trusted.
    if git_output(repo_root, &["rev-parse", "--is-shallow-repository"])?.trim() == "true" {
        return Err(EpochError::ShallowClone);
    }

    let log = git_output(
        repo_root,
        &[
            "log",
            "--follow",
            "--name-status",
            "--format=%H",
            "--",
            page_path,
        ],
    )?;

    // Walk newest→oldest: (sha, name in effect at that commit, parsed value).
    let mut name = page_path.to_string();
    let mut walked: Vec<(String, String, Option<String>)> = Vec::new();
    for (sha, rows) in parse_name_status_log(&log) {
        let value = blob_field_value(repo_root, &sha, &name)?;
        walked.push((sha, name.clone(), value));
        // The pre-commit name for the next (older) commit comes from the
        // rename row whose new path is the name in effect at this commit.
        if let Some(row) = rows.iter().find(|r| r.is_rename_to(&name)) {
            name = row.old_path.clone();
        }
    }

    for pair in walked.windows(2) {
        let (newer, older) = (&pair[0], &pair[1]);
        if newer.2 != older.2 {
            // Invariant: the walk starts at HEAD, whose value is the
            // committed value (Some) — the newer side of the first differing
            // pair always carries the field, never an absence.
            return Ok(LinkEpoch::Commit {
                sha: newer.0.clone(),
                path_at_commit: newer.1.clone(),
                value: newer
                    .2
                    .clone()
                    .expect("newer side of a differing pair always carries the field"),
            });
        }
    }
    let anchor = walked
        .last()
        .expect("--follow on an existing page yields at least one commit");
    Ok(LinkEpoch::Commit {
        sha: anchor.0.clone(),
        path_at_commit: anchor.1.clone(),
        value: anchor
            .2
            .clone()
            .expect("all walked values are equal to the committed value"),
    })
}

/// One `--name-status` row: the status token (letter plus informational
/// similarity suffix) and the path(s). Renames and copies carry two paths.
#[derive(Debug)]
struct NameStatusRow {
    status: String,
    old_path: String,
    new_path: String,
}

impl NameStatusRow {
    /// Any `R###` row is a rename — the similarity suffix is informational.
    fn is_rename(&self) -> bool {
        self.status.starts_with('R')
    }

    fn is_rename_to(&self, name: &str) -> bool {
        self.is_rename() && self.new_path == name
    }
}

/// Parse `git log --name-status --format=%H` output into `(sha, rows)` pairs,
/// newest commit first. A blank line separates commit records.
fn parse_name_status_log(log: &str) -> Vec<(String, Vec<NameStatusRow>)> {
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

/// The page's parsed field value at `commit`, read under the name in effect
/// there. `Ok(None)` when the path is absent at that commit — a walk
/// boundary, not an error.
fn blob_field_value(
    repo_root: &Path,
    commit: &str,
    path: &str,
) -> Result<Option<String>, EpochError> {
    match read_blob_at(repo_root, commit, path)? {
        None => Ok(None),
        Some(bytes) => Ok(extract_links_reviewed(&String::from_utf8_lossy(&bytes))),
    }
}

/// `git show <commit>:<path>` (or `git show :<path>` for the index when
/// `commit` is empty). `Ok(None)` when the path is absent there (exit 128);
/// any other git failure fails closed.
fn read_blob_at(repo_root: &Path, commit: &str, path: &str) -> Result<Option<Vec<u8>>, EpochError> {
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
        let Some(anchor_page) = read_blob_at(repo_root, sha, path_at_commit)? else {
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
            });
        }
    }

    let mut classes = Vec::new();
    for link in parse_line_range_links(page_content) {
        // The effective path is the resolved path after suffix salvage, so
        // `target_path` reports where the link was actually judged against.
        let (outcome, target_path) = classify_link(
            repo_root,
            source,
            page_path,
            &link,
            &certified,
            anchor.map(|(sha, _)| sha),
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
}

/// Parse every line-range fragment link in `content`, in document order.
/// Plain paths and heading-slug fragments are outside this system's scope.
fn parse_line_range_links(content: &str) -> Vec<ParsedLink> {
    let bytes = content.as_bytes();
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
        let href = &content[href_start..href_end];
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
            label: content[label_start + 1..i].to_string(),
            source_line: line_of(&line_starts, i),
            href_byte_start: href_start,
            href_byte_end: href_end,
            original_href: href.to_string(),
        });
        i = href_end + 1;
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
fn classify_link(
    repo_root: &Path,
    source: DocSource,
    page_path: &str,
    link: &ParsedLink,
    certified: &[CertifiedLink],
    anchor_sha: Option<&str>,
) -> Result<(DriftOutcome, String), EpochError> {
    // Decision 5 target resolution: resolve the href path part, then salvage
    // the longest existing repo-relative suffix when the direct read misses.
    let (target_path, target_bytes) =
        read_target(repo_root, source, page_path, &link.target_path_raw)?;

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
    // (same label text OR same href-range string) — plus the relocation
    // clause: same label AND the current link's target-range content equals
    // the certified link's. The clause's content equality is the canonical
    // rk64 comparison, so a CRLF checkout cannot regress it.
    let mut matched: Vec<&CertifiedLink> = Vec::new();
    let mut memo: HashMap<(String, u32, u32), u64> = HashMap::new();
    let current_fp = current_content_fp(target_bytes.as_deref(), link.start, link.end);
    for c in certified {
        let identity_arm =
            c.target_path == target_path && (c.label == link.label || c.fragment == link.fragment);
        // The clause arm short-circuits behind `!identity_arm`, so identity
        // matches never pay the anchor-side blob read.
        let clause_arm = !identity_arm
            && c.label == link.label
            && certified_content_fp(repo_root, anchor_sha, c, &mut memo)? == current_fp;
        if identity_arm || clause_arm {
            matched.push(c);
        }
    }
    if matched.is_empty() {
        return Ok((DriftOutcome::Uncertified, target_path));
    }
    let cert = primary_cert(&matched, &target_path, link.start, link.end);

    // Step 3: target present at the current side, extent still fitting.
    let Some(bytes) = target_bytes.as_deref() else {
        let outcome = move_scan_outcome(
            repo_root,
            source,
            page_path,
            &target_path,
            None,
            cert,
            anchor_sha,
            &mut memo,
            DriftOutcome::Broken,
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
        let cert_fp = certified_content_fp(repo_root, anchor_sha, cert, &mut memo)?;
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
            source,
            page_path,
            &target_path,
            Some(bytes),
            cert,
            anchor_sha,
            &mut memo,
            DriftOutcome::Drift,
        )?;
        return Ok((outcome, target_path));
    }

    // Step 5: the href's range differs from every certified range (the href
    // was edited). Content equal to the certified content → Healthy (already
    // relocated); different → Uncertified (the as-written locator was never
    // reviewed).
    let cur_fp = cheap_fingerprint_with_extent(
        bytes,
        &Extent::LineRange {
            start: link.start,
            end: link.end,
        },
    );
    for c in &matched {
        if certified_content_fp(repo_root, anchor_sha, c, &mut memo)? == cur_fp {
            return Ok((DriftOutcome::Healthy, target_path));
        }
    }
    Ok((DriftOutcome::Uncertified, target_path))
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
fn certified_content_fp(
    repo_root: &Path,
    anchor_sha: &str,
    cert: &CertifiedLink,
    memo: &mut HashMap<(String, u32, u32), u64>,
) -> Result<u64, EpochError> {
    let key = (cert.target_path.clone(), cert.start, cert.end);
    if let Some(&fp) = memo.get(&key) {
        return Ok(fp);
    }
    let fp = match read_blob_at(repo_root, anchor_sha, &cert.target_path)? {
        None => 0, // no content at all
        Some(bytes) => cheap_fingerprint_with_extent(
            &bytes,
            &Extent::LineRange {
                start: cert.start,
                end: cert.end,
            },
        ),
    };
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

/// The exact-tier move scan: find the certified content as a contiguous
/// window, same file first, then every other candidate file in the repo.
/// One match → `Moved`; ≥2 → `Unknown` (the card's multi-match rule is
/// unconditional — never first-hit-wins); zero → `zero_matches`. The fuzzy
/// Jaccard tier lands in Phase 1 with its re-tuned thresholds.
#[allow(clippy::too_many_arguments)]
fn move_scan_outcome(
    repo_root: &Path,
    source: DocSource,
    page_path: &str,
    target_path: &str,
    target_bytes: Option<&[u8]>,
    cert: &CertifiedLink,
    anchor_sha: &str,
    memo: &mut HashMap<(String, u32, u32), u64>,
    zero_matches: DriftOutcome,
) -> Result<DriftOutcome, EpochError> {
    let span = line_range_span(cert.start, cert.end);
    if span == 0 {
        // Degenerate certified content never matches a window.
        return Ok(zero_matches);
    }
    let cert_fp = certified_content_fp(repo_root, anchor_sha, cert, memo)?;
    let extent = Extent::LineRange {
        start: 1,
        end: span as u32,
    };

    // Same-file tier: the link's own target.
    if let Some(bytes) = target_bytes {
        let idx = LineIndex::build(bytes);
        let matches = scan_indexed_rk64(&[(target_path.to_string(), idx)], cert_fp, extent, None);
        match matches.len() {
            1 => {
                return moved_to(&matches[0]);
            }
            n if n >= 2 => return Ok(DriftOutcome::Unknown),
            _ => {}
        }
    }

    // Cross-file tier: every other candidate file in the repo. The page
    // itself is excluded — a body quoting the range must not relocate the
    // link into itself — as is the target (already scanned above).
    let candidates = candidate_files(repo_root, source)?;
    let others: Vec<(String, Vec<u8>)> = candidates
        .into_iter()
        .filter(|(path, _)| path != target_path && path != page_path)
        .collect();
    let matches = scan_for_content_hash_rk64(&others, cert_fp, extent, None);
    match matches.len() {
        1 => moved_to(&matches[0]),
        n if n >= 2 => Ok(DriftOutcome::Unknown),
        _ => Ok(zero_matches),
    }
}

fn moved_to(location: &crate::rk64::Location) -> Result<DriftOutcome, EpochError> {
    Ok(DriftOutcome::Moved {
        new_path: location.path.clone(),
        new_start: location.start_line,
        new_end: location.end_line,
    })
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

/// Repo candidate files for the cross-file move scan: every tracked file
/// readable from the current side, excluding wiki-mesh storage and build
/// output. Read through the source-aware reader so `--source head`/`index`
/// scan the same layer the targets were read from.
fn candidate_files(
    repo_root: &Path,
    source: DocSource,
) -> Result<Vec<(String, Vec<u8>)>, EpochError> {
    let list = git_output(repo_root, &["ls-files", "-z"])?;
    let mut out = Vec::new();
    for path in list.split('\0') {
        if path.is_empty() {
            continue;
        }
        if path.starts_with(".wiki/")
            || path.starts_with("node_modules/")
            || path.starts_with("target/")
            || path.starts_with("dist/")
        {
            continue;
        }
        if let Some(bytes) = read_current(repo_root, source, path)? {
            out.push((path.to_string(), bytes));
        }
    }
    Ok(out)
}

/// The bytes of `path` from the layer selected by `source` (the
/// `read_anchor_source` pattern): worktree fs, `HEAD` blob, or index blob.
/// `Ok(None)` when the path is absent in that layer.
fn read_current(
    repo_root: &Path,
    source: DocSource,
    path: &str,
) -> Result<Option<Vec<u8>>, EpochError> {
    match source {
        DocSource::WorkingTree => Ok(std::fs::read(repo_root.join(path)).ok()),
        DocSource::Head => read_blob_at(repo_root, "HEAD", path),
        DocSource::Index => read_blob_at(repo_root, "", path),
    }
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
    repo_root: &Path,
    source: DocSource,
    page_path: &str,
    path_part: &str,
) -> Result<(String, Option<Vec<u8>>), EpochError> {
    let target_path = resolve_target_path(repo_root, page_path, path_part);
    if let Some(bytes) = read_current(repo_root, source, &target_path)? {
        return Ok((target_path, Some(bytes)));
    }
    if source == DocSource::WorkingTree
        && let Some(salvaged) = super::locate_existing_suffix(&target_path, repo_root)
        && let Some(bytes) = read_current(repo_root, source, &salvaged)?
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
    fn extracts_none_when_field_absent_or_unparseable() {
        assert_eq!(field_value(&make_wiki_page("P", "body\n", None)), None);
        assert_eq!(field_value("no frontmatter at all\n"), None);
        // A field-looking line in the BODY is not frontmatter.
        let body = "links-reviewed: 5\n";
        assert_eq!(field_value(&make_wiki_page("P", body, None)), None);
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
        let epoch = find_anchor_commit(repo.path(), "wiki/page.md", Some("2"), Some("1"))
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
            find_anchor_commit(repo.path(), "wiki/page.md", Some("1"), None).expect("resolves");
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
            find_anchor_commit(repo.path(), "wiki/page.md", None, Some("1")).expect("resolves");
        assert_eq!(epoch, LinkEpoch::Current { value: None });
    }

    #[test]
    fn field_absent_everywhere_is_missing() {
        let repo = TestRepo::new();
        repo.create_file("wiki/page.md", &make_wiki_page("P", "body\n", None));
        repo.commit("no field");
        let epoch = find_anchor_commit(repo.path(), "wiki/page.md", None, None).expect("resolves");
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
    fn anchors_at_field_introduction_when_no_pair_differs() {
        let repo = TestRepo::new();
        repo.create_file("wiki/page.md", &make_wiki_page("P", "body\n", Some("1")));
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
    fn two_chained_renames_still_resolve_with_the_anchor_time_name() {
        let repo = TestRepo::new();
        repo.create_file("wiki/page.md", &make_wiki_page("P", "body\n", Some("1")));
        let intro_sha = repo.commit("field=1 at page.md");
        repo.git(&["mv", "wiki/page.md", "wiki/renamed.md"]);
        repo.commit("rename to renamed.md");
        repo.git(&["mv", "wiki/renamed.md", "wiki/final-name.md"]);
        repo.commit("rename to final-name.md");

        let epoch = find_anchor_commit(repo.path(), "wiki/final-name.md", Some("1"), Some("1"))
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
    fn shallow_clone_fails_closed() {
        let repo = TestRepo::new();
        repo.create_file("wiki/page.md", &make_wiki_page("P", "body\n", Some("1")));
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
    fn healthy_when_target_unchanged() {
        let (repo, c1) = repo_with_certified_link();
        let epoch = LinkEpoch::Commit {
            sha: c1,
            path_at_commit: "wiki/page.md".into(),
            value: "1".into(),
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
            value: "1".into(),
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
            value: "1".into(),
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
            value: "1".into(),
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
            value: "1".into(),
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
            value: "1".into(),
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
        // worktree filesystem to consult: salvage is skipped, the edited
        // locator resolves to nothing, and the link is Uncertified rather
        // than silently blessed via the worktree copy on disk.
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "[b](dir/target.md#L2-L4)\n", Some("1")),
        );
        repo.commit("href moved to a nonexistent directory");
        let epoch = LinkEpoch::Commit {
            sha: c1,
            path_at_commit: "wiki/page.md".into(),
            value: "1".into(),
        };
        let page_content = repo.read("wiki/page.md");
        let classes = classify_page(
            repo.path(),
            DocSource::Head,
            "wiki/page.md",
            &page_content,
            &epoch,
        )
        .expect("classifies");
        assert_eq!(classes.len(), 1);
        assert_eq!(classes[0].outcome, DriftOutcome::Uncertified);
        // The effective path stays the resolved href path — no salvage.
        assert_eq!(classes[0].target_path, "wiki/dir/target.md");
    }

    #[test]
    fn salvage_yields_to_a_resolved_target_that_exists() {
        let (repo, c1) = repo_with_root_target();
        // `wiki/dir/target.md` EXISTS with different content, so the direct
        // resolution wins and salvage never runs: the link points at real
        // content that is not the certified block → Uncertified.
        repo.create_file("wiki/dir/target.md", "X0\nX1\nX2\nX3\nX4\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "[b](dir/target.md#L2-L4)\n", Some("1")),
        );
        let epoch = LinkEpoch::Commit {
            sha: c1,
            path_at_commit: "wiki/page.md".into(),
            value: "1".into(),
        };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(classes.len(), 1);
        assert_eq!(classes[0].outcome, DriftOutcome::Uncertified);
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
            value: "1".into(),
        };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(
            classes[0].outcome,
            DriftOutcome::Moved {
                new_path: "wiki/target.md".into(),
                new_start: 4,
                new_end: 6
            }
        );
    }

    #[test]
    fn moved_cross_file_rewrites_path_and_range() {
        // Cross-file Moved arises when the target is gone (here: the content
        // moved out and the file was deleted). A merely truncated target is
        // Broken per the card — see `broken_when_extent_overhangs_target`.
        let (repo, c1) = repo_with_certified_link();
        repo.remove_file("wiki/target.md");
        repo.create_file("wiki/other.md", &format!("H\n{BLOCK}\nF\n"));
        repo.commit("block moved to other.md");
        let epoch = LinkEpoch::Commit {
            sha: c1,
            path_at_commit: "wiki/page.md".into(),
            value: "1".into(),
        };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(
            classes[0].outcome,
            DriftOutcome::Moved {
                new_path: "wiki/other.md".into(),
                new_start: 2,
                new_end: 4
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
            value: "1".into(),
        };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(classes[0].outcome, DriftOutcome::Unknown);
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
            value: "1".into(),
        };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(classes[0].outcome, DriftOutcome::Healthy);
    }

    #[test]
    fn href_edited_to_different_content_is_uncertified() {
        let (repo, c1) = repo_with_certified_link();
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "[b](target.md#L1-L1)\n", Some("1")),
        );
        repo.commit("href now points at different content");
        let epoch = LinkEpoch::Commit {
            sha: c1,
            path_at_commit: "wiki/page.md".into(),
            value: "1".into(),
        };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(classes[0].outcome, DriftOutcome::Uncertified);
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
            value: "1".into(),
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
            value: "1".into(),
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
        repo.remove_file("wiki/target.md");
        repo.create_file("wiki/other.md", &format!("H\n{BLOCK}\nF\n"));
        repo.commit("block moved to other.md");

        let epoch = LinkEpoch::Commit {
            sha: c1,
            path_at_commit: "wiki/page.md".into(),
            value: "1".into(),
        };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(
            classes[0].outcome,
            DriftOutcome::Moved {
                new_path: "wiki/other.md".into(),
                new_start: 2,
                new_end: 4
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
            "the relocation clause keeps the tool's own rewrite certified"
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
            value: "1".into(),
        };
        let classes = classify(&repo, &epoch, "wiki/page.md").expect("classifies");
        assert_eq!(classes[0].outcome, DriftOutcome::Healthy);
        // `source_line` reflects the CURRENT page content: frontmatter spans
        // lines 1–5, the inserted heading line 6, the blank line 7, the link
        // line 8 — identity is label/range-based, never position-based.
        assert_eq!(classes[0].source_line, 8);
    }
}
