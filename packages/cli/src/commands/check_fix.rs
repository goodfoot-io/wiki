use serde::Serialize;

// ── Types ─────────────────────────────────────────────────────────────────────

/// What kind of rewrite the fix performs.
#[derive(Debug)]
#[allow(dead_code)]
pub enum FixKind {
    /// Fix 1: rewrite a broken link whose target was renamed.
    BrokenLinkRename,
    /// Fix 2: update a line-range anchor that drifted due to line insertions/deletions.
    MeshAnchorShift,
    /// Fix 3: rewrite an alias href to the canonical slug.
    AliasToCanonical,
    /// Fix 5: update a heading anchor that was renamed in-place (same position).
    HeadingRename,
}

/// How confident the fixer is that the proposed rewrite is correct.
#[derive(Debug)]
#[allow(dead_code)]
pub enum Confidence {
    /// One unambiguous rename; safe to apply automatically.
    High,
    /// Plausible but could be wrong; requires human review.
    Low,
}

/// A rewrite that the fixer determined is safe to apply.
#[derive(Debug, Serialize)]
pub struct Fix {
    /// Repo-relative path to the file that will be rewritten.
    pub file: String,
    /// 1-based line number of the link in the source file.
    pub line: usize,
    /// The old href text (as it appears in the source).
    pub old_href: String,
    /// The new href text that replaces it.
    pub new_href: String,
}

/// A fix that was skipped because it could not be applied safely.
#[derive(Debug, Serialize)]
pub struct SkippedFix {
    /// Repo-relative path to the file that would have been rewritten.
    pub file: String,
    /// 1-based line number of the link in the source file.
    pub line: usize,
    /// Human-readable explanation of why the fix was skipped.
    pub reason: String,
}

/// The result of a fix pass: what was applied and what was skipped.
#[derive(Debug)]
pub struct FixPlan {
    pub fixes: Vec<Fix>,
    pub skipped: Vec<SkippedFix>,
}

// ── Entry point ───────────────────────────────────────────────────────────────

/// Run the fix pass against `files` in `repo_root`.
///
/// When `dry_run` is true, no files are written; the returned `FixPlan` still
/// describes what *would* be applied. Currently a stub that returns an empty
/// plan — real fix logic is added in subsequent phases.
pub fn run_fix_pass(
    _files: &[std::path::PathBuf],
    _repo_root: &std::path::Path,
    _dry_run: bool,
) -> miette::Result<FixPlan> {
    Ok(FixPlan {
        fixes: vec![],
        skipped: vec![],
    })
}
