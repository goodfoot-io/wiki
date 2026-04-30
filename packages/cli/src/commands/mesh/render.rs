//! Shell rendering for the build-then-render pipeline.
//!
//! Consumes deduplicated `MeshDraft`s plus a `PreflightResult` and emits the
//! review-ready shell script described by `tests/fixtures/mesh-scaffold/expected.sh`.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use super::draft::MeshDraft;
use super::hints::{AntiPattern, FallbackReason, Hint};
use super::preflight::PreflightResult;

/// Total fixed display width of the per-page divider, in characters.
const DIVIDER_WIDTH: usize = 66;

/// Render `meshes` (already grouped per-page in declaration order) and a
/// pre-flight result into a shell script.
///
/// `uncovered_findings` is the pre-consolidation link count; `proposed_meshes`
/// is the post-consolidation count (i.e. `meshes.len()`).
pub(crate) fn render_shell(
    meshes: &[MeshDraft],
    preflight: &PreflightResult,
    uncovered_findings: usize,
) -> String {
    let proposed_meshes = meshes.len();
    let pages_covered = unique_pages(meshes);
    let ratio = consolidation_ratio(uncovered_findings, proposed_meshes);

    let mut out = String::new();
    let _ = writeln!(out, "#!/bin/sh");
    let _ = writeln!(
        out,
        "# wiki mesh scaffold — {uncovered_findings} uncovered findings → {proposed_meshes} proposed meshes (consolidation ratio {ratio})"
    );
    let _ = writeln!(out, "# Pages covered: {pages_covered}");
    out.push('\n');

    render_preflight(&mut out, preflight);
    out.push('\n');

    // Group by page in first-occurrence order.
    let mut page_order: Vec<String> = Vec::new();
    let mut by_page: Vec<(String, Vec<&MeshDraft>)> = Vec::new();
    for m in meshes {
        if let Some(entry) = by_page.iter_mut().find(|(k, _)| *k == m.page_path) {
            entry.1.push(m);
        } else {
            page_order.push(m.page_path.clone());
            by_page.push((m.page_path.clone(), vec![m]));
        }
    }

    for (page, page_meshes) in &by_page {
        let _ = writeln!(out, "{}", page_divider(page));
        for m in page_meshes {
            out.push('\n');
            render_mesh_block(&mut out, m);
        }
        out.push('\n');
    }

    let _ = writeln!(out, "# Run after reviewing whys above:");
    for m in meshes {
        let _ = writeln!(out, "git mesh commit {}", m.slug);
    }

    out
}

fn render_preflight(out: &mut String, preflight: &PreflightResult) {
    let _ = writeln!(
        out,
        "# Pre-flight: anchored paths must exist in HEAD before mesh commit."
    );
    match preflight {
        PreflightResult::Ok { missing } if missing.is_empty() => {
            let _ = writeln!(out, "# All anchored paths exist in HEAD.");
        }
        PreflightResult::Ok { missing } => {
            let _ = writeln!(out, "# Missing in HEAD:");
            for p in missing {
                let _ = writeln!(out, "#   {p}");
            }
        }
        PreflightResult::Skipped { reason } => {
            let _ = writeln!(out, "# Pre-flight skipped: {reason}");
        }
    }
}

fn render_mesh_block(out: &mut String, m: &MeshDraft) {
    // Source heading + opening sentence
    let heading = if m.section_heading.is_empty() {
        "(top of file)".to_string()
    } else {
        m.section_heading.clone()
    };
    let _ = writeln!(out, "# Source: {heading}");
    let _ = writeln!(out, "#   \"{}\"", m.section_opening);

    // Hint comments (in attachment order: build → consolidate → anti-pattern)
    for h in &m.hints {
        render_hint(out, h);
    }

    // git mesh add <slug> \
    //   <anchor1> \
    //   <anchor2>
    let _ = writeln!(out, "git mesh add {} \\", m.slug);
    let last = m.anchors.len().saturating_sub(1);
    for (i, a) in m.anchors.iter().enumerate() {
        if i == last {
            let _ = writeln!(out, "  {a}");
        } else {
            let _ = writeln!(out, "  {a} \\");
        }
    }
    let _ = writeln!(out, "git mesh why {} -m \"\"", m.slug);
}

fn render_hint(out: &mut String, h: &Hint) {
    match h {
        Hint::Consolidated { count } => {
            let _ = writeln!(out, "# Consolidated {count} occurrences of this anchor set");
        }
        Hint::ConsiderMerge { other_slug } => {
            let _ = writeln!(out, "# Consider merging with {other_slug}");
        }
        Hint::FallbackSlug { reason } => match reason {
            FallbackReason::NoHeadingUsedLabel => {
                let _ = writeln!(
                    out,
                    "# TODO: rename — fallback derivation (no section heading above link; used link label)"
                );
            }
            FallbackReason::NoHeadingUsedFileStem => {
                let _ = writeln!(
                    out,
                    "# TODO: rename — fallback derivation (no section heading or link label; used target file stem)"
                );
            }
        },
        Hint::WarnAntiPattern { pattern } => match pattern {
            AntiPattern::CouplingTemplate => {
                let _ = writeln!(
                    out,
                    "# WARN: source sentence describes the coupling rather than the subsystem;"
                );
                let _ = writeln!(
                    out,
                    "#       the why should name the subsystem and what it does across the anchors."
                );
            }
            AntiPattern::HeadlessPredicate => {
                let _ = writeln!(
                    out,
                    "# WARN: source sentence opens with a bare identifier predicate;"
                );
                let _ = writeln!(
                    out,
                    "#       the why should name the subsystem rather than restating the symbol."
                );
            }
            AntiPattern::VerbLead => {
                let _ = writeln!(
                    out,
                    "# WARN: source sentence opens with a verb rather than a subject;"
                );
                let _ = writeln!(
                    out,
                    "#       the why should name the subsystem and what it does across the anchors."
                );
            }
        },
    }
}

/// Build the per-page divider line, e.g. `# ── wiki/billing.md ───…` padded
/// with `─` to a fixed display-character width.
fn page_divider(page: &str) -> String {
    // Prefix `# ── ` is 5 codepoints; trailing space + dashes fill the rest.
    let prefix = "# ── ";
    let used = prefix.chars().count() + page.chars().count() + 1; // +1 for the space
    let dashes = DIVIDER_WIDTH.saturating_sub(used);
    let mut s = String::new();
    s.push_str(prefix);
    s.push_str(page);
    s.push(' ');
    for _ in 0..dashes {
        s.push('─');
    }
    s
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
    fn divider_pads_to_fixed_width() {
        let d = page_divider("wiki/billing.md");
        assert_eq!(d.chars().count(), DIVIDER_WIDTH);
        assert!(d.starts_with("# ── wiki/billing.md "));
    }
}
