//! Markdown rendering for the build-then-render pipeline.
//!
//! Consumes deduplicated `MeshDraft`s and emits a review-ready markdown
//! document described by `tests/fixtures/mesh-scaffold/expected.md`.

use std::collections::{HashMap, HashSet};

use super::draft::MeshDraft;
use super::scaffold::ParseError;

/// Render `meshes` (already grouped per-page in declaration order) and the
/// per-page titles (frontmatter `title` keyed by `page_path`, `None` when
/// absent) into a markdown document. Pages whose paths appear in
/// `parse_error_paths` are excluded so `parseErrors` and `pages` are disjoint.
pub(crate) fn render_markdown(
    meshes: &[MeshDraft],
    page_titles: &HashMap<String, Option<String>>,
    parse_errors: &[ParseError],
    parse_error_paths: &HashSet<String>,
) -> String {
    let mut out = String::new();

    // Filter meshes to exclude parse-error pages before computing counts.
    let filtered: Vec<&MeshDraft> = meshes
        .iter()
        .filter(|m| !parse_error_paths.contains(&m.page_path))
        .collect();

    // Prepend parse-error block when non-empty.
    if !parse_errors.is_empty() {
        render_parse_errors(&mut out, parse_errors, !filtered.is_empty());
        // Separator only when other content follows.
        if !filtered.is_empty() {
            use std::fmt::Write as _;
            let _ = writeln!(out, "---");
            out.push('\n');
        }
    }

    // Group by page in first-occurrence order.
    let mut by_page: Vec<(String, Vec<&MeshDraft>)> = Vec::new();
    for m in filtered {
        if let Some(entry) = by_page.iter_mut().find(|(k, _)| *k == m.page_path) {
            entry.1.push(m);
        } else {
            by_page.push((m.page_path.clone(), vec![m]));
        }
    }

    let page_count = by_page.len();
    for (page_idx, (page, page_meshes)) in by_page.iter().enumerate() {
        let title = page_titles.get(page).and_then(|t| t.as_deref());
        let header = match title {
            Some(t) if !t.is_empty() => format!("# {t} • {page}"),
            _ => format!("# {page}"),
        };
        let _ = std::fmt::Write::write_fmt(&mut out, format_args!("{header}\n"));
        out.push('\n');
        for m in page_meshes.iter() {
            render_mesh_block(&mut out, m);
        }
        // Insert `---` separator between consecutive pages; suppress terminal one.
        if page_idx + 1 < page_count {
            use std::fmt::Write as _;
            let _ = writeln!(out, "---");
            out.push('\n');
        }
    }

    out
}

/// Render the empty-corpus success markdown.
pub(crate) fn render_empty_markdown(parse_errors: &[ParseError]) -> String {
    let mut out = String::new();

    if !parse_errors.is_empty() {
        // When every file fails to parse there is no corpus to report.
        // Emit only the parse-error block — no separator, no success line.
        // `has_scaffold_following` is false: no pages follow.
        render_parse_errors(&mut out, parse_errors, false);
        return out;
    }

    use std::fmt::Write as _;
    let _ = writeln!(out, "# wiki scaffold");
    out.push('\n');
    // Precondition: `scaffold::run` reached this branch because `all_inputs`
    // was empty after the link-collection loop — i.e. no internal fragment
    // link with a parsed line range was discovered. No coverage probe runs
    // before this branch, so the message must not claim coverage.
    let _ = writeln!(
        out,
        "No internal fragment links with line ranges were found in the discovered wiki pages."
    );
    out
}

/// Render the parse-error block.
///
/// `has_scaffold_following` controls the header phrasing:
/// - `true`  → advisory ("Some wiki pages could not be parsed and were skipped:")
/// - `false` → hard-stop ("Unable to generate scaffolding due to parsing errors:")
fn render_parse_errors(out: &mut String, parse_errors: &[ParseError], has_scaffold_following: bool) {
    use std::fmt::Write as _;
    let header = if has_scaffold_following {
        "Some wiki pages could not be parsed and were skipped:"
    } else {
        "Unable to generate scaffolding due to parsing errors:"
    };
    let _ = writeln!(out, "{header}");
    for e in parse_errors {
        let _ = writeln!(out, "- {} ({})", e.path, e.kind.reason());
    }
    out.push('\n');
}

fn render_mesh_block(out: &mut String, m: &MeshDraft) {
    use std::fmt::Write as _;

    // heading_chain was already trimmed once in trim_chains_in_place.
    if !m.heading_chain.is_empty() {
        let chain_str = m.heading_chain.join(" → ");
        let _ = writeln!(out, "## {chain_str}");
        out.push('\n');
    }

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

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::draft::MeshDraft;
    use super::super::scaffold::ParseErrorKind;

    fn make_draft(
        page_path: &str,
        slug: &str,
        heading_chain: Vec<&str>,
        anchors: Vec<&str>,
    ) -> MeshDraft {
        MeshDraft {
            page_path: page_path.to_string(),
            slug: slug.to_string(),
            anchors: anchors.iter().map(|s| s.to_string()).collect(),
            structured_anchors: Vec::new(),
            heading_chain: heading_chain.iter().map(|s| s.to_string()).collect(),
            consolidated_count: 1,
            noun: String::new(),
            page_ns: super::super::scaffold::PageNamespace::default(),
        }
    }

    fn make_error(path: &str, kind: ParseErrorKind) -> ParseError {
        ParseError {
            path: path.to_string(),
            kind,
        }
    }

    // ── per-page separator placement ──────────────────────────────────────────

    #[test]
    fn per_page_separator_between_pages_not_terminal() {
        let d1 = make_draft(
            "wiki/page1.md",
            "wiki/foo",
            vec![],
            vec!["wiki/page1.md#L1-L5", "src/a.rs#L1-L5"],
        );
        let d2 = make_draft(
            "wiki/page2.md",
            "wiki/bar",
            vec![],
            vec!["wiki/page2.md#L1-L5", "src/b.rs#L1-L5"],
        );
        let mut titles = HashMap::new();
        titles.insert("wiki/page1.md".to_string(), Some("Page 1".to_string()));
        titles.insert("wiki/page2.md".to_string(), Some("Page 2".to_string()));
        let out = render_markdown(&[d1, d2], &titles, &[], &HashSet::new());

        assert!(out.contains("\n---\n"), "interior separator missing:\n{out}");
        assert!(!out.trim_end().ends_with("---"), "terminal --- must be absent:\n{out}");
    }

    #[test]
    fn single_page_no_separator() {
        let d = make_draft(
            "wiki/page1.md",
            "wiki/foo",
            vec![],
            vec!["wiki/page1.md#L1-L5", "src/a.rs#L1-L5"],
        );
        let titles = HashMap::new();
        let out = render_markdown(&[d], &titles, &[], &HashSet::new());
        assert!(!out.contains("\n---\n"), "no separator for single page:\n{out}");
    }

    #[test]
    fn empty_chain_omits_heading_line() {
        let d = make_draft(
            "wiki/page.md",
            "wiki/foo",
            vec![],
            vec!["wiki/page.md#L1-L5", "src/a.rs#L1-L5"],
        );
        let titles = HashMap::new();
        let out = render_markdown(&[d], &titles, &[], &HashSet::new());
        assert!(
            !out.contains("## "),
            "## line must be absent for empty chain:\n{out}"
        );
        // No blockquote — section_opening removed from output entirely.
        assert!(!out.contains("> "), "blockquote must be absent:\n{out}");
    }

    #[test]
    fn page_section_anchor_renders_first_in_block() {
        let d = make_draft(
            "wiki/page.md",
            "wiki/foo",
            vec!["Section"],
            vec![
                "wiki/page.md#L10-L20",
                "src/a.rs#L1-L5",
                "src/b.rs#L1-L5",
            ],
        );
        let titles = HashMap::new();
        let out = render_markdown(&[d], &titles, &[], &HashSet::new());
        let add_idx = out.find("git mesh add wiki/foo").expect("add line present");
        let page_idx = out[add_idx..].find("wiki/page.md#L10-L20").expect("page anchor present");
        let target_idx = out[add_idx..].find("src/a.rs#L1-L5").expect("target present");
        assert!(page_idx < target_idx, "page anchor must precede targets:\n{out}");
    }

    // ── parse-error block integration ─────────────────────────────────────────

    #[test]
    fn render_empty_markdown_zero_errors_no_block() {
        let out = render_empty_markdown(&[]);
        assert!(
            !out.contains("Unable to generate"),
            "no parse-error block expected with zero errors"
        );
        assert!(out.contains("# wiki scaffold"));
    }

    // Regression: the empty-corpus success branch fires whenever `all_inputs`
    // is empty in `scaffold::run` — i.e. when no internal fragment link with a
    // parsed line range was discovered. No coverage probe runs before it, so
    // the message must not claim coverage filtering decided the outcome.
    #[test]
    fn render_empty_markdown_does_not_claim_coverage() {
        let out = render_empty_markdown(&[]);
        assert!(
            !out.contains("covered by a mesh"),
            "empty-corpus message must not claim mesh coverage — no coverage \
             check runs before this branch:\n{out}"
        );
        assert!(
            !out.contains("No uncovered fragment links"),
            "empty-corpus message must not frame the result as a coverage \
             outcome:\n{out}"
        );
    }

    #[test]
    fn render_empty_markdown_with_errors_block_alone() {
        let errors = vec![make_error("wiki/bad.md", ParseErrorKind::MissingTitle)];
        let out = render_empty_markdown(&errors);
        // No scaffold follows → hard-stop header.
        assert!(out.starts_with("Unable to generate scaffolding due to parsing errors:\n"));
        assert!(out.contains("wiki/bad.md (frontmatter present but `title:` is missing)"));
        assert!(!out.contains("\n---\n"), "separator must be absent");
        assert!(!out.contains("# wiki scaffold"), "success header must be absent");
    }

    #[test]
    fn render_parse_error_advisory_header_when_meshes_follow() {
        let d = make_draft(
            "wiki/page.md",
            "wiki/foo",
            vec![],
            vec!["wiki/page.md#L1-L5", "src/a.rs#L1-L5"],
        );
        let errors = vec![make_error("wiki/bad.md", ParseErrorKind::NoFrontmatter)];
        let titles = HashMap::new();
        let out = render_markdown(&[d], &titles, &errors, &HashSet::new());
        // Advisory header when scaffold follows.
        assert!(
            out.starts_with("Some wiki pages could not be parsed and were skipped:\n"),
            "expected advisory header, got:\n{out}"
        );
        assert!(
            out.contains("\n---\n"),
            "separator must follow parse-error block when meshes present:\n{out}"
        );
    }

    #[test]
    fn render_parse_error_hard_stop_header_when_no_meshes() {
        let errors = vec![make_error("wiki/bad.md", ParseErrorKind::NoFrontmatter)];
        let out = render_empty_markdown(&errors);
        // Hard-stop header when no scaffold follows.
        assert!(
            out.starts_with("Unable to generate scaffolding due to parsing errors:\n"),
            "expected hard-stop header, got:\n{out}"
        );
        assert!(
            !out.contains("\n---\n"),
            "separator must be absent when no meshes:\n{out}"
        );
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

    // ── parse-error page excluded from pages output ───────────────────────────

    #[test]
    fn parse_error_page_excluded_from_render() {
        // Draft whose page_path is in the parse_error_paths set.
        let bad = make_draft(
            "wiki/bad.md",
            "wiki/bad-slug",
            vec![],
            vec!["wiki/bad.md#L1-L5", "src/a.rs#L1-L5"],
        );
        let good = make_draft(
            "wiki/good.md",
            "wiki/good-slug",
            vec![],
            vec!["wiki/good.md#L1-L5", "src/b.rs#L1-L5"],
        );
        let errors = vec![make_error("wiki/bad.md", ParseErrorKind::NoFrontmatter)];
        let mut parse_error_paths = HashSet::new();
        parse_error_paths.insert("wiki/bad.md".to_string());
        let titles = HashMap::new();
        let out = render_markdown(&[bad, good], &titles, &errors, &parse_error_paths);
        assert!(!out.contains("bad-slug"), "parse-error page must not appear in pages:\n{out}");
        assert!(out.contains("good-slug"), "good page must appear:\n{out}");
    }
}
