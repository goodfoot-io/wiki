use std::sync::OnceLock;

use regex::Regex;

#[allow(unused_imports)]
use crate::parser::scrub_non_content;
use crate::parser::FragmentLink;

/// A `FragmentLink` augmented with surrounding text and heading context.
#[derive(Clone)]
pub(crate) struct AugmentedLink {
    pub(crate) link: FragmentLink,
    /// Stack of ATX heading texts (levels 1–6) in effect at the link's source line.
    pub(crate) heading_chain: Vec<String>,
    /// The deepest heading text in `heading_chain` prefixed by its ATX hashes
    /// (e.g. `## Sync detection`). Empty when the link sits before any heading
    /// on the page.
    pub(crate) section_heading: String,
    /// 1-based start line of the section containing the link. When the link is
    /// enclosed by an ATX heading, this is the heading line itself; otherwise
    /// it is the first non-blank line of the paragraph the link sits in.
    pub(crate) section_start_line: u32,
    /// 1-based inclusive end line of the section. For headings, this is the
    /// line before the next heading (any level), trimmed of trailing blanks,
    /// or the last line of the file. For paragraphs, this is the last
    /// non-blank line of the link's block.
    pub(crate) section_end_line: u32,
}

/// Augment a slice of fragment links found in `content`.
pub(crate) fn augment(links: &[FragmentLink], content: &str) -> Vec<AugmentedLink> {
    let raw_lines: Vec<&str> = content.split('\n').collect();
    let scrubbed = scrub_non_content(content);
    // Heading chain is built from the *scrubbed* lines so headings inside code
    // blocks don't pollute the chain.
    let scrubbed_lines: Vec<&str> = scrubbed.split('\n').collect();
    let heading_chain_at = build_heading_chains_from_raw(&raw_lines, &scrubbed_lines);
    let heading_meta_at = build_heading_meta_from_raw(&raw_lines, &scrubbed_lines);
    let heading_at_line = collect_heading_lines(&raw_lines, &scrubbed_lines);

    links
        .iter()
        .map(|link| {
            let idx = link.source_line.saturating_sub(1);
            let heading_chain = heading_chain_at.get(idx).cloned().unwrap_or_default();
            let meta = heading_meta_at.get(idx).cloned().unwrap_or_default();
            let section_heading = match &meta {
                Some((level, _, text)) => {
                    let hashes = "#".repeat(*level);
                    format!("{hashes} {text}")
                }
                None => String::new(),
            };
            let (section_start, section_end) =
                compute_section_bounds(meta.as_ref(), idx, &heading_at_line, &raw_lines);
            AugmentedLink {
                link: link.clone(),
                heading_chain,
                section_heading,
                section_start_line: section_start as u32,
                section_end_line: section_end as u32,
            }
        })
        .collect()
}

/// (heading_level, heading_line_idx_0based, heading_text) for the deepest heading
/// in scope at each line. `None` when no heading precedes that line.
type HeadingMeta = Option<(usize, usize, String)>;

fn build_heading_meta_from_raw(raw_lines: &[&str], scrubbed_lines: &[&str]) -> Vec<HeadingMeta> {
    let mut out: Vec<HeadingMeta> = Vec::with_capacity(raw_lines.len());
    let mut stack: Vec<(usize, usize, String)> = Vec::new();
    let mut in_fence = false;
    for (i, raw) in raw_lines.iter().enumerate() {
        out.push(stack.last().cloned());
        let trimmed = raw.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if scrubbed_lines
            .get(i)
            .map(|s| s.trim())
            .unwrap_or("")
            .is_empty()
        {
            continue;
        }
        if let Some((level, text)) = parse_atx_heading(raw) {
            stack.retain(|(l, _, _)| *l < level);
            stack.push((level, i, text));
        }
    }
    out
}

/// Return a per-line bool flagging ATX heading lines (fence-aware), so section
/// bounds can scan forward to the next heading without re-toggling fence state.
fn collect_heading_lines(raw_lines: &[&str], scrubbed_lines: &[&str]) -> Vec<bool> {
    let mut out = vec![false; raw_lines.len()];
    let mut in_fence = false;
    for (i, raw) in raw_lines.iter().enumerate() {
        let trimmed = raw.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if scrubbed_lines
            .get(i)
            .map(|s| s.trim())
            .unwrap_or("")
            .is_empty()
        {
            continue;
        }
        if parse_atx_heading(raw).is_some() {
            out[i] = true;
        }
    }
    out
}

/// Return the 1-based inclusive line range of the section the link sits in.
/// When `meta` is `Some`, the section is the deepest enclosing heading down to
/// (but not including) the next heading line of any level — trailing blanks
/// trimmed. When `meta` is `None`, the section is the blank-line-bounded
/// paragraph containing the link.
fn compute_section_bounds(
    meta: Option<&(usize, usize, String)>,
    link_idx: usize,
    heading_at_line: &[bool],
    raw_lines: &[&str],
) -> (usize, usize) {
    let total = raw_lines.len();
    if let Some((_, hidx, _)) = meta {
        let start = hidx + 1; // 1-based
        let mut end_idx = total.saturating_sub(1);
        for j in (*hidx + 1)..total {
            if heading_at_line.get(j).copied().unwrap_or(false) {
                end_idx = j.saturating_sub(1);
                break;
            }
            end_idx = j;
        }
        let mut end = end_idx + 1; // 1-based
        while end > start
            && raw_lines
                .get(end - 1)
                .map(|l| l.trim().is_empty())
                .unwrap_or(true)
        {
            end -= 1;
        }
        return (start, end);
    }
    // Paragraph fallback: blank-line- or heading-bounded block.
    let mut s = link_idx;
    while s > 0 {
        let prev = s - 1;
        let line = raw_lines[prev];
        if line.trim().is_empty() || heading_at_line.get(prev).copied().unwrap_or(false) {
            break;
        }
        s = prev;
    }
    let mut e = link_idx;
    while e + 1 < total {
        let next = e + 1;
        let line = raw_lines[next];
        if line.trim().is_empty() || heading_at_line.get(next).copied().unwrap_or(false) {
            break;
        }
        e = next;
    }
    (s + 1, e + 1)
}

fn build_heading_chains_from_raw(raw_lines: &[&str], scrubbed_lines: &[&str]) -> Vec<Vec<String>> {
    let mut chains: Vec<Vec<String>> = Vec::with_capacity(raw_lines.len());
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut in_fence = false;
    for (i, raw) in raw_lines.iter().enumerate() {
        chains.push(stack.iter().map(|(_, t)| t.clone()).collect());
        let trimmed = raw.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if scrubbed_lines
            .get(i)
            .map(|s| s.trim())
            .unwrap_or("")
            .is_empty()
        {
            continue;
        }
        if let Some((level, text)) = parse_atx_heading(raw) {
            stack.retain(|(l, _)| *l < level);
            stack.push((level, text));
        }
    }
    chains
}

fn parse_atx_heading(line: &str) -> Option<(usize, String)> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('#') {
        return None;
    }
    let level = trimmed.chars().take_while(|&c| c == '#').count();
    if level == 0 || level > 6 {
        return None;
    }
    let rest = &trimmed[level..];
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let text = rest.trim();
    if text.is_empty() {
        return None;
    }
    let stripped: String = text.chars().filter(|c| !"`*_[]".contains(*c)).collect();
    let cleaned = stripped.trim().to_string();
    Some((level, cleaned))
}

#[allow(dead_code)]
fn _unused_regex_anchor() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^$").unwrap())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_fragment_links;

    #[test]
    fn heading_chain_nesting_h1_h2_h3() {
        let content = "# Top\n## Mid\n### Deep\n[label](foo.rs)\n";
        let links = parse_fragment_links(content);
        assert_eq!(links.len(), 1);
        let result = augment(&links, content);
        assert_eq!(result[0].heading_chain, vec!["Top", "Mid", "Deep"]);
    }

    #[test]
    fn heading_chain_resets_at_same_level() {
        let content = "# Top\n## First\n## Second\n[label](foo.rs)\n";
        let links = parse_fragment_links(content);
        let result = augment(&links, content);
        assert_eq!(result[0].heading_chain, vec!["Top", "Second"]);
    }

    #[test]
    fn links_inside_code_block_are_excluded_by_parser() {
        let content = "```\n[label](foo.rs)\n```\n";
        let links = parse_fragment_links(content);
        assert!(links.is_empty());
        let result = augment(&links, content);
        assert!(result.is_empty());
    }

    #[test]
    fn heading_inside_code_block_not_in_chain() {
        let content = "# Real\n```\n# Fake\n```\n[label](foo.rs)\n";
        let links = parse_fragment_links(content);
        let result = augment(&links, content);
        assert_eq!(result[0].heading_chain, vec!["Real"]);
    }

    #[test]
    fn link_on_first_line_no_panic() {
        let content = "[label](foo.rs)\n# After\n";
        let links = parse_fragment_links(content);
        let result = augment(&links, content);
        assert!(result[0].heading_chain.is_empty());
    }

    #[test]
    fn link_on_last_line_no_panic() {
        let content = "# Heading\nsome text\n[label](foo.rs)";
        let links = parse_fragment_links(content);
        let result = augment(&links, content);
        assert_eq!(result[0].heading_chain, vec!["Heading"]);
    }

    // ── section bounds ────────────────────────────────────────────────────────

    #[test]
    fn bounds_under_heading_run_to_next_heading_minus_blanks() {
        // Lines: 1 `# Top`, 2 blank, 3 `prose`, 4 blank, 5 `## Next`, 6 blank, 7 link.
        let content = "# Top\n\nprose\n\n## Next\n\n[l](f.rs#L1-L2)\n";
        let links = parse_fragment_links(content);
        let augmented = augment(&links, content);
        // Link is in `## Next` section.
        let a = &augmented[0];
        assert_eq!(a.section_heading, "## Next");
        // Heading at line 5 → start=5; next heading: none → run to last non-blank.
        assert_eq!(a.section_start_line, 5);
        // Line 7 has the link; trailing newline produces an empty entry which is
        // trimmed; end is 7.
        assert_eq!(a.section_end_line, 7);
    }

    #[test]
    fn bounds_under_heading_stop_before_sibling_heading() {
        // 1 `# Top`, 2 blank, 3 `prose [l](f.rs#L1-L2)`, 4 blank, 5 `# Other`.
        let content = "# Top\n\nprose [l](f.rs#L1-L2)\n\n# Other\n";
        let links = parse_fragment_links(content);
        let augmented = augment(&links, content);
        let a = &augmented[0];
        assert_eq!(a.section_heading, "# Top");
        assert_eq!(a.section_start_line, 1);
        // Trailing blank trimmed; section ends at line 3.
        assert_eq!(a.section_end_line, 3);
    }

    #[test]
    fn bounds_paragraph_fallback_when_no_enclosing_heading() {
        // Link before any heading: section is its blank-line-bounded paragraph.
        // 1 blank, 2 `prose line one`, 3 `[l](f.rs#L1-L2) line two`, 4 blank, 5 `# After`.
        let content = "\nprose line one\n[l](f.rs#L1-L2) line two\n\n# After\n";
        let links = parse_fragment_links(content);
        let augmented = augment(&links, content);
        let a = &augmented[0];
        assert_eq!(a.section_heading, "");
        assert_eq!(a.section_start_line, 2);
        assert_eq!(a.section_end_line, 3);
    }

    #[test]
    fn bounds_paragraph_fallback_single_line() {
        // 1 blank, 2 `[l](f.rs#L1-L2)`, 3 blank.
        let content = "\n[l](f.rs#L1-L2)\n\n";
        let links = parse_fragment_links(content);
        let augmented = augment(&links, content);
        let a = &augmented[0];
        assert_eq!(a.section_start_line, 2);
        assert_eq!(a.section_end_line, 2);
    }

    #[test]
    fn two_links_in_same_section_share_bounds() {
        // 1 `## H`, 2 blank, 3 `[a](x.rs#L1-L2)`, 4 `[b](y.rs#L1-L2)`, 5 blank.
        let content = "## H\n\n[a](x.rs#L1-L2)\n[b](y.rs#L1-L2)\n";
        let links = parse_fragment_links(content);
        let augmented = augment(&links, content);
        assert_eq!(augmented.len(), 2);
        assert_eq!(augmented[0].section_start_line, augmented[1].section_start_line);
        assert_eq!(augmented[0].section_end_line, augmented[1].section_end_line);
    }
}
