//! Markdown rendering for the build-then-render pipeline.
//!
//! Consumes deduplicated `MeshDraft`s and emits a review-ready markdown
//! document described by `tests/fixtures/mesh-scaffold/expected.md`.

use std::collections::HashMap;

use super::draft::MeshDraft;
use super::scaffold::ParseError;

/// Normalize heading or title text for comparison: strip inline markup chars,
/// collapse whitespace. Mirrors `normalize_heading_text` in scaffold.rs.
fn normalize_heading_text(s: &str) -> String {
    let stripped: String = s.chars().filter(|c| !"`*_[]".contains(*c)).collect();
    stripped.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Trim the leading entry of `heading_chain` when it matches `page_title`
/// after normalization. Returns the trimmed chain as a Vec.
fn trim_heading_chain(chain: &[String], page_title: &str) -> Vec<String> {
    if chain.is_empty() {
        return Vec::new();
    }
    let normalized_title = normalize_heading_text(page_title);
    let normalized_first = normalize_heading_text(&chain[0]);
    if !normalized_title.is_empty() && normalized_first.eq_ignore_ascii_case(&normalized_title) {
        chain[1..].to_vec()
    } else {
        chain.to_vec()
    }
}

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

    let page_count = by_page.len();
    for (page_idx, (page, page_meshes)) in by_page.iter().enumerate() {
        let title = page_titles.get(page).and_then(|t| t.as_deref());
        let header = match title {
            Some(t) if !t.is_empty() => format!("# {t} • {page}"),
            _ => format!("# {page}"),
        };
        let title_str = title.unwrap_or("").to_string();
        let _ = std::fmt::Write::write_fmt(&mut out, format_args!("{header}\n"));
        out.push('\n');
        for m in page_meshes.iter() {
            render_mesh_block(&mut out, m, &title_str);
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

fn render_mesh_block(out: &mut String, m: &MeshDraft, page_title: &str) {
    use std::fmt::Write as _;

    // Compute trimmed heading chain.
    let trimmed = trim_heading_chain(&m.heading_chain, page_title);
    if !trimmed.is_empty() {
        let chain_str = trimmed.join(" → ");
        let _ = writeln!(out, "## {chain_str}");
    }
    // When trimmed chain is empty, omit the ## line entirely.

    // Multi-line verbatim blockquote.
    if m.section_opening_lines.is_empty() {
        // Fallback: emit cleaned single-line opening if verbatim lines unavailable.
        if !m.section_opening.is_empty() {
            let _ = writeln!(out, "> {}", m.section_opening);
        }
    } else {
        for line in &m.section_opening_lines {
            let _ = writeln!(out, "> {line}");
        }
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
        section_opening_lines: Vec<&str>,
        anchors: Vec<&str>,
    ) -> MeshDraft {
        MeshDraft {
            page_path: page_path.to_string(),
            slug: slug.to_string(),
            anchors: anchors.iter().map(|s| s.to_string()).collect(),
            structured_anchors: Vec::new(),
            section_opening: String::new(),
            heading_chain: heading_chain.iter().map(|s| s.to_string()).collect(),
            section_opening_lines: section_opening_lines.iter().map(|s| s.to_string()).collect(),
            consolidated_count: 1,
        }
    }

    fn make_error(path: &str, kind: ParseErrorKind) -> ParseError {
        ParseError {
            path: path.to_string(),
            kind,
        }
    }

    // ── heading chain trim ────────────────────────────────────────────────────

    #[test]
    fn trim_positive_drops_leading_when_equals_title() {
        let chain = vec!["Billing".to_string(), "Charge handler".to_string()];
        let trimmed = trim_heading_chain(&chain, "Billing");
        assert_eq!(trimmed, vec!["Charge handler"]);
    }

    #[test]
    fn trim_negative_keeps_chain_when_top_differs() {
        let chain = vec!["Charge handler".to_string()];
        let trimmed = trim_heading_chain(&chain, "Billing");
        assert_eq!(trimmed, vec!["Charge handler"]);
    }

    #[test]
    fn trim_empties_to_nothing_when_single_equals_title() {
        let chain = vec!["Incremental indexing".to_string()];
        let trimmed = trim_heading_chain(&chain, "Incremental indexing");
        assert!(trimmed.is_empty());
    }

    #[test]
    fn trim_case_insensitive() {
        let chain = vec!["BILLING".to_string()];
        let trimmed = trim_heading_chain(&chain, "billing");
        assert!(trimmed.is_empty());
    }

    #[test]
    fn trim_strips_inline_markup_in_title() {
        let chain = vec!["Foo".to_string()];
        // title with backtick markup
        let trimmed = trim_heading_chain(&chain, "`Foo`");
        assert!(trimmed.is_empty());
    }

    // ── per-page separator placement ──────────────────────────────────────────

    #[test]
    fn per_page_separator_between_pages_not_terminal() {
        let d1 = make_draft(
            "wiki/page1.md",
            "wiki/foo",
            vec![],
            vec!["Some text."],
            vec!["wiki/page1.md", "src/a.rs#L1-L5"],
        );
        let d2 = make_draft(
            "wiki/page2.md",
            "wiki/bar",
            vec![],
            vec!["Other text."],
            vec!["wiki/page2.md", "src/b.rs#L1-L5"],
        );
        let mut titles = HashMap::new();
        titles.insert("wiki/page1.md".to_string(), Some("Page 1".to_string()));
        titles.insert("wiki/page2.md".to_string(), Some("Page 2".to_string()));
        let out = render_markdown(&[d1, d2], &titles, &[]);

        // Interior separator present.
        assert!(out.contains("\n---\n"), "interior separator missing:\n{out}");
        // Terminal section (page 2) does not end with `---`.
        assert!(!out.trim_end().ends_with("---"), "terminal --- must be absent:\n{out}");
    }

    #[test]
    fn single_page_no_separator() {
        let d = make_draft(
            "wiki/page1.md",
            "wiki/foo",
            vec![],
            vec!["Some text."],
            vec!["wiki/page1.md", "src/a.rs#L1-L5"],
        );
        let titles = HashMap::new();
        let out = render_markdown(&[d], &titles, &[]);
        assert!(!out.contains("\n---\n"), "no separator for single page:\n{out}");
    }

    // ── top-of-file link (empty chain) ────────────────────────────────────────

    #[test]
    fn top_of_file_link_omits_heading_line() {
        let d = make_draft(
            "wiki/page.md",
            "wiki/foo",
            vec![],  // empty chain
            vec!["Top of file prose."],
            vec!["wiki/page.md", "src/a.rs#L1-L5"],
        );
        let titles = HashMap::new();
        let out = render_markdown(&[d], &titles, &[]);
        assert!(
            !out.contains("## "),
            "## line must be absent for empty chain:\n{out}"
        );
        assert!(out.contains("> Top of file prose."), "blockquote missing:\n{out}");
    }

    // ── multi-line verbatim blockquote ────────────────────────────────────────

    #[test]
    fn multi_line_excerpt_with_inline_markup_preserved() {
        let d = make_draft(
            "wiki/page.md",
            "wiki/foo",
            vec!["Section"],
            vec![
                "See [[SomePage]] for details.",
                "Also [handleCharge](src/charge.ts#L1-L5) does dispatch.",
            ],
            vec!["wiki/page.md", "src/a.rs#L1-L5"],
        );
        let mut titles = HashMap::new();
        titles.insert("wiki/page.md".to_string(), Some("My Page".to_string()));
        let out = render_markdown(&[d], &titles, &[]);
        assert!(
            out.contains("> See [[SomePage]] for details."),
            "wikilink not preserved:\n{out}"
        );
        assert!(
            out.contains("> Also [handleCharge](src/charge.ts#L1-L5) does dispatch."),
            "inline link not preserved:\n{out}"
        );
    }

    // ── blockquote nesting when source line starts with > ─────────────────────

    #[test]
    fn source_line_starting_with_blockquote_nests() {
        let d = make_draft(
            "wiki/page.md",
            "wiki/foo",
            vec!["Section"],
            vec!["> This is a nested quote."],
            vec!["wiki/page.md", "src/a.rs#L1-L5"],
        );
        let titles = HashMap::new();
        let out = render_markdown(&[d], &titles, &[]);
        assert!(
            out.contains("> > This is a nested quote."),
            "nested blockquote not rendered:\n{out}"
        );
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

    #[test]
    fn render_empty_markdown_with_errors_block_alone() {
        let errors = vec![make_error("wiki/bad.md", ParseErrorKind::MissingTitle)];
        let out = render_empty_markdown(&errors);
        assert!(out.starts_with("Unable to generate scaffolding due to parsing errors:\n"));
        assert!(out.contains("wiki/bad.md (frontmatter present but `title:` is missing)"));
        assert!(!out.contains("\n---\n"), "separator must be absent");
        assert!(!out.contains("# wiki scaffold"), "success header must be absent");
    }

    #[test]
    fn render_parse_error_separator_present_when_meshes_follow() {
        let d = make_draft(
            "wiki/page.md",
            "wiki/foo",
            vec![],
            vec!["Some text."],
            vec!["wiki/page.md", "src/a.rs#L1-L5"],
        );
        let errors = vec![make_error("wiki/bad.md", ParseErrorKind::NoFrontmatter)];
        let titles = HashMap::new();
        let out = render_markdown(&[d], &titles, &errors);
        assert!(
            out.contains("\n---\n"),
            "separator must follow parse-error block when meshes present:\n{out}"
        );
    }

    #[test]
    fn render_parse_error_separator_absent_when_no_meshes() {
        let errors = vec![make_error("wiki/bad.md", ParseErrorKind::NoFrontmatter)];
        let out = render_empty_markdown(&errors);
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
}
