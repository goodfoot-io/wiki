//! Markdown rendering for the build-then-render pipeline.
//!
//! Consumes deduplicated `MeshDraft`s and emits a review-ready markdown
//! document described by `tests/fixtures/mesh-scaffold/expected.md`.

use std::collections::HashMap;

use super::draft::MeshDraft;
use super::scaffold::ParseError;

/// Render `meshes` (already grouped per-page in declaration order) and the
/// per-page titles (frontmatter `title` keyed by `page_path`, `None` when
/// absent) into a markdown document.
pub(crate) fn render_markdown(
    meshes: &[MeshDraft],
    page_titles: &HashMap<String, Option<String>>,
    parse_errors: &[ParseError],
) -> String {
    let mut out = String::new();

    // Prepend parse-error block when non-empty.
    if !parse_errors.is_empty() {
        render_parse_errors(&mut out, parse_errors);
        // Separator only when other content follows (meshes is non-empty).
        if !meshes.is_empty() {
            use std::fmt::Write as _;
            let _ = writeln!(out, "---");
            out.push('\n');
        }
    }

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
        let _ = std::fmt::Write::write_fmt(&mut out, format_args!("{header}\n"));
        out.push('\n');
        for m in page_meshes {
            render_mesh_block(&mut out, m);
        }
    }

    out
}

/// Render the empty-corpus success markdown. No ratio, no preflight, no
/// per-page sections, no footer prompt — just a tiny readable header that
/// remains valid markdown.
pub(crate) fn render_empty_markdown(parse_errors: &[ParseError]) -> String {
    let mut out = String::new();

    if !parse_errors.is_empty() {
        // When every file fails to parse there is no corpus to report.
        // Emit only the parse-error block — no separator, no success line.
        render_parse_errors(&mut out, parse_errors);
        return out;
    }

    use std::fmt::Write as _;
    let _ = writeln!(out, "# wiki scaffold");
    out.push('\n');
    let _ = writeln!(
        out,
        "No uncovered fragment links — every link is already covered by a mesh."
    );
    out
}

fn render_parse_errors(out: &mut String, parse_errors: &[ParseError]) {
    use std::fmt::Write as _;
    let _ = writeln!(out, "Unable to generate scaffolding due to parsing errors:");
    for e in parse_errors {
        let _ = writeln!(out, "- {} ({})", e.path, e.kind.reason());
    }
    out.push('\n');
}

fn render_mesh_block(out: &mut String, m: &MeshDraft) {
    use std::fmt::Write as _;
    let heading_text = strip_atx_hashes(&m.section_heading);
    let heading = if heading_text.is_empty() {
        "(top of file)".to_string()
    } else {
        heading_text
    };
    let _ = writeln!(out, "## {heading}");
    let _ = writeln!(out, "> {}", m.section_opening);
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

fn strip_atx_hashes(s: &str) -> String {
    s.trim_start_matches(|c: char| c == '#' || c.is_whitespace())
        .trim()
        .to_string()
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_atx_hashes_drops_leading_hashes() {
        assert_eq!(strip_atx_hashes("## Sync detection"), "Sync detection");
        assert_eq!(
            strip_atx_hashes("# Charge handler notes"),
            "Charge handler notes"
        );
        assert_eq!(strip_atx_hashes(""), "");
    }

    // ── parse-error block unit tests ──────────────────────────────────────────

    use super::super::scaffold::{ParseError, ParseErrorKind};

    fn make_error(path: &str, kind: ParseErrorKind) -> ParseError {
        ParseError {
            path: path.to_string(),
            kind,
        }
    }

    #[test]
    fn render_empty_markdown_zero_errors_no_block() {
        let out = render_empty_markdown(&[]);
        assert!(
            !out.contains("Unable to generate"),
            "no parse-error block expected with zero errors"
        );
        assert!(out.contains("# wiki scaffold"));
    }

    #[test]
    fn render_empty_markdown_with_errors_block_alone() {
        let errors = vec![make_error("wiki/bad.md", ParseErrorKind::MissingTitle)];
        let out = render_empty_markdown(&errors);
        assert!(out.starts_with("Unable to generate scaffolding due to parsing errors:\n"));
        assert!(out.contains("wiki/bad.md (frontmatter present but `title:` is missing)"));
        // No separator and no success line when parse errors are present.
        assert!(!out.contains("\n---\n"), "separator must be absent");
        assert!(
            !out.contains("# wiki scaffold"),
            "success header must be absent"
        );
        assert!(
            !out.contains("No uncovered fragment links"),
            "success body must be absent"
        );
    }

    #[test]
    fn render_empty_markdown_no_errors_emits_success_body() {
        let out = render_empty_markdown(&[]);
        assert!(
            !out.contains("Unable to generate"),
            "no parse-error block expected with zero errors"
        );
        assert!(out.contains("# wiki scaffold"));
        assert!(out.contains("No uncovered fragment links"));
    }

    #[test]
    fn render_parse_error_reason_strings() {
        fn reason(kind: ParseErrorKind) -> String {
            kind.reason()
        }
        assert_eq!(
            reason(ParseErrorKind::NoFrontmatter),
            "no frontmatter block — file does not start with `---`"
        );
        assert_eq!(
            reason(ParseErrorKind::MissingTitle),
            "frontmatter present but `title:` is missing"
        );
        assert_eq!(
            reason(ParseErrorKind::EmptyTitle),
            "frontmatter present but `title:` is empty"
        );
        assert_eq!(
            reason(ParseErrorKind::Unreadable("oops".to_string())),
            "file could not be read: oops"
        );
        assert_eq!(
            reason(ParseErrorKind::Malformed),
            "malformed frontmatter — could not parse `title`"
        );
    }
}
