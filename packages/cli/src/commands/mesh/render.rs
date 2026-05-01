//! Markdown rendering for the build-then-render pipeline.
//!
//! Consumes deduplicated `MeshDraft`s plus a `PreflightResult` and emits a
//! review-ready markdown document described by
//! `tests/fixtures/mesh-scaffold/expected.md`.

use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;

use super::draft::MeshDraft;
use super::hints::{AntiPattern, FallbackReason, Hint};
use super::preflight::PreflightResult;

/// Render `meshes` (already grouped per-page in declaration order), the
/// per-page titles (frontmatter `title` keyed by `page_path`, `None` when
/// absent), and the pre-flight result into a markdown document.
///
/// `uncovered_findings` is the pre-consolidation link count; `proposed_meshes`
/// is the post-consolidation count (i.e. `meshes.len()`).
pub(crate) fn render_markdown(
    meshes: &[MeshDraft],
    page_titles: &HashMap<String, Option<String>>,
    preflight: &PreflightResult,
    uncovered_findings: usize,
    skipped_fixtures: usize,
) -> String {
    let proposed_meshes = meshes.len();
    let pages_covered = unique_pages(meshes);
    let ratio = consolidation_ratio(uncovered_findings, proposed_meshes);

    let mut out = String::new();
    let _ = writeln!(out, "# wiki scaffold");
    out.push('\n');
    let _ = writeln!(
        out,
        "{uncovered_findings} uncovered findings → {proposed_meshes} proposed meshes (consolidation ratio {ratio})."
    );
    if skipped_fixtures > 0 {
        let suffix = if skipped_fixtures == 1 { "" } else { "s" };
        let _ = writeln!(
            out,
            "Pages covered: {pages_covered} ({skipped_fixtures} test fixture{suffix} skipped)."
        );
    } else {
        let _ = writeln!(out, "Pages covered: {pages_covered}.");
    }
    out.push('\n');

    render_preflight(&mut out, preflight);
    out.push('\n');

    // Group by page in first-occurrence order.
    let mut by_page: Vec<(String, Vec<&MeshDraft>)> = Vec::new();
    for m in meshes {
        if let Some(entry) = by_page.iter_mut().find(|(k, _)| *k == m.page_path) {
            entry.1.push(m);
        } else {
            by_page.push((m.page_path.clone(), vec![m]));
        }
    }

    for (page, page_meshes) in &by_page {
        let title = page_titles.get(page).and_then(|t| t.as_deref());
        let header = match title {
            Some(t) if !t.is_empty() => format!("# {t} • {page}"),
            _ => format!("# {page}"),
        };
        let _ = writeln!(out, "{header}");
        out.push('\n');
        for m in page_meshes {
            render_mesh_block(&mut out, m);
        }
    }

    let _ = writeln!(out, "# Commit Changes After Review");
    out.push('\n');
    let _ = writeln!(out, "```bash");
    for m in meshes {
        let _ = writeln!(out, "git mesh commit {}", m.slug);
    }
    let _ = writeln!(out, "```");

    out
}

/// Render the empty-corpus success markdown. No ratio, no preflight, no
/// per-page sections, no footer prompt — just a tiny readable header that
/// remains valid markdown.
pub(crate) fn render_empty_markdown() -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# wiki scaffold");
    out.push('\n');
    let _ = writeln!(
        out,
        "No uncovered fragment links — every link is already covered by a mesh."
    );
    out
}

fn render_preflight(out: &mut String, preflight: &PreflightResult) {
    let _ = writeln!(out, "## Pre-flight");
    out.push('\n');
    let _ = writeln!(
        out,
        "Anchored paths must exist in HEAD before mesh commit."
    );
    out.push('\n');
    match preflight {
        PreflightResult::Ok { missing } if missing.is_empty() => {
            let _ = writeln!(out, "All anchored paths exist in HEAD.");
        }
        PreflightResult::Ok { missing } => {
            let _ = writeln!(out, "Missing in HEAD:");
            out.push('\n');
            for p in missing {
                let _ = writeln!(out, "- `{p}`");
            }
        }
        PreflightResult::Skipped { reason } => {
            let _ = writeln!(out, "Pre-flight skipped: {reason}");
        }
    }
}

fn render_mesh_block(out: &mut String, m: &MeshDraft) {
    let heading_text = strip_atx_hashes(&m.section_heading);
    let heading = if heading_text.is_empty() {
        "(top of file)".to_string()
    } else {
        heading_text
    };
    let _ = writeln!(out, "## {heading}");
    let _ = writeln!(out, "> {}", m.section_opening);

    // Hint annotations as additional blockquote paragraphs.
    for h in &m.hints {
        out.push('\n');
        render_hint(out, h);
    }
    out.push('\n');

    let _ = writeln!(out, "```bash");
    let _ = writeln!(out, "git mesh add {} \\", m.slug);
    let last = m.anchors.len().saturating_sub(1);
    for (i, a) in m.anchors.iter().enumerate() {
        if i == last {
            let _ = writeln!(out, "  {a}");
        } else {
            let _ = writeln!(out, "  {a} \\");
        }
    }
    let _ = writeln!(out, "git mesh why {} -m \"[why]\"", m.slug);
    let _ = writeln!(out, "```");
    out.push('\n');
}

fn render_hint(out: &mut String, h: &Hint) {
    match h {
        Hint::Consolidated { count } => {
            let _ = writeln!(
                out,
                "> **Consolidated** {count} occurrences of this anchor set."
            );
        }
        Hint::ConsiderMerge { other_slug } => {
            let _ = writeln!(out, "> **Consider merging** with `{other_slug}`.");
        }
        Hint::FallbackSlug { reason } => match reason {
            FallbackReason::NoHeadingUsedLabel => {
                let _ = writeln!(
                    out,
                    "> **TODO: rename** — fallback derivation (no section heading above link; used link label)."
                );
            }
            FallbackReason::NoHeadingUsedFileStem => {
                let _ = writeln!(
                    out,
                    "> **TODO: rename** — fallback derivation (no section heading or link label; used target file stem)."
                );
            }
        },
        Hint::WarnAntiPattern { pattern } => match pattern {
            AntiPattern::CouplingTemplate => {
                let _ = writeln!(
                    out,
                    "> **WARN:** source sentence describes the coupling rather than the subsystem; the why should name the subsystem and what it does across the anchors."
                );
            }
            AntiPattern::HeadlessPredicate => {
                let _ = writeln!(
                    out,
                    "> **WARN:** source sentence opens with a bare identifier predicate; the why should name the subsystem rather than restating the symbol."
                );
            }
            AntiPattern::VerbLead => {
                let _ = writeln!(
                    out,
                    "> **WARN:** source sentence opens with a verb rather than a subject; the why should name the subsystem and what it does across the anchors."
                );
            }
            AntiPattern::DegenerateExcerpt => {
                let _ = writeln!(
                    out,
                    "> **WARN:** degenerate excerpt — open the source page to write the why by hand."
                );
            }
        },
    }
}

fn strip_atx_hashes(s: &str) -> String {
    s.trim_start_matches(|c: char| c == '#' || c.is_whitespace())
        .trim()
        .to_string()
}

fn unique_pages(meshes: &[MeshDraft]) -> usize {
    let set: BTreeSet<&str> = meshes.iter().map(|m| m.page_path.as_str()).collect();
    set.len()
}

fn consolidation_ratio(uncovered: usize, proposed: usize) -> String {
    if proposed == 0 {
        return "0.00×".to_string();
    }
    let r = uncovered as f64 / proposed as f64;
    format!("{r:.2}×")
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratio_two_decimals_with_unicode_x() {
        assert_eq!(consolidation_ratio(11, 9), "1.22×");
        assert_eq!(consolidation_ratio(10, 10), "1.00×");
        assert_eq!(consolidation_ratio(0, 0), "0.00×");
    }

    #[test]
    fn strip_atx_hashes_drops_leading_hashes() {
        assert_eq!(strip_atx_hashes("## Sync detection"), "Sync detection");
        assert_eq!(strip_atx_hashes("# Charge handler notes"), "Charge handler notes");
        assert_eq!(strip_atx_hashes(""), "");
    }
}
