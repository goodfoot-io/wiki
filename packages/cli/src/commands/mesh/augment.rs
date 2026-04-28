use crate::parser::{FragmentLink, scrub_non_content};

/// A `FragmentLink` augmented with surrounding text and heading context.
pub(crate) struct AugmentedLink {
    pub(crate) link: FragmentLink,
    /// The link's source line ± 1 line, scrubbed via `scrub_non_content`, joined with `\n`.
    pub(crate) surrounding_text: String,
    /// Stack of ATX heading texts (levels 1–6) in effect at the link's source line.
    pub(crate) heading_chain: Vec<String>,
}

/// Augment a slice of fragment links found in `content`.
///
/// Returns one `AugmentedLink` per input link. Links whose `source_line` falls
/// inside a fenced code block are included — the surrounding-text window is
/// still extracted from the *scrubbed* content, so code fences become blank
/// lines and contribute no meaningful text.  Headings that appear inside code
/// blocks are never pushed onto the heading chain because they are blanked out
/// by the scrubber before the chain is built.
pub(crate) fn augment(links: &[FragmentLink], content: &str) -> Vec<AugmentedLink> {
    let scrubbed = scrub_non_content(content);
    let scrubbed_lines: Vec<&str> = scrubbed.lines().collect();
    let heading_chain_at = build_heading_chains(&scrubbed_lines);

    links
        .iter()
        .map(|link| {
            let surrounding_text = extract_surrounding_text(link.source_line, &scrubbed_lines);
            let heading_chain = heading_chain_at
                .get(link.source_line.saturating_sub(1))
                .cloned()
                .unwrap_or_default();
            AugmentedLink {
                link: link.clone(),
                surrounding_text,
                heading_chain,
            }
        })
        .collect()
}

/// Build the heading chain in effect at each line index (0-based).
///
/// Returns a `Vec` of length equal to `lines.len()`.  Each element is the
/// heading chain that is active *at* that line (i.e. before any heading on
/// that line is processed — so a heading at line N is reflected in the chains
/// for lines N+1 and beyond).
fn build_heading_chains(lines: &[&str]) -> Vec<Vec<String>> {
    let mut chains: Vec<Vec<String>> = Vec::with_capacity(lines.len());
    // Stack: (level, text)
    let mut stack: Vec<(usize, String)> = Vec::new();

    for line in lines {
        // Record the chain *before* processing this line's heading.
        chains.push(stack.iter().map(|(_, t)| t.clone()).collect());

        if let Some((level, text)) = parse_atx_heading(line) {
            // Pop any headings at the same or deeper level.
            stack.retain(|(l, _)| *l < level);
            stack.push((level, text));
        }
    }

    chains
}

/// Parse an ATX heading (`# … ` through `###### … `) from a single line.
///
/// Returns `(level, heading_text)` on success, or `None` if the line is not
/// an ATX heading.  The heading text has its optional closing `#` sequence
/// stripped and is trimmed.
fn parse_atx_heading(line: &str) -> Option<(usize, String)> {
    let line = line.trim_end();
    if !line.starts_with('#') {
        return None;
    }
    let level = line.chars().take_while(|&c| c == '#').count();
    if level > 6 {
        return None;
    }
    let rest = &line[level..];
    // Must be followed by a space (or be an empty heading `##`)
    if !rest.is_empty() && !rest.starts_with(' ') {
        return None;
    }
    let text = rest.trim();
    // Strip optional closing `#` sequence
    let text = text.trim_end_matches('#').trim_end();
    Some((level, text.to_string()))
}

/// Extract the link line ± 1 lines from the scrubbed content and join with `\n`.
///
/// `source_line` is 1-based.  Clamps to the available range so that links on
/// the first or last line do not panic.
fn extract_surrounding_text(source_line: usize, scrubbed_lines: &[&str]) -> String {
    if scrubbed_lines.is_empty() {
        return String::new();
    }
    let idx = source_line.saturating_sub(1); // convert to 0-based
    let start = idx.saturating_sub(1);
    let end = (idx + 1).min(scrubbed_lines.len() - 1);
    scrubbed_lines[start..=end].join("\n")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{LinkKind, parse_fragment_links};

    #[allow(dead_code)]
    fn make_link(source_line: usize) -> FragmentLink {
        FragmentLink {
            kind: LinkKind::Internal,
            path: "foo.rs".to_string(),
            start_line: None,
            end_line: None,
            text: "label".to_string(),
            original_text: "label".to_string(),
            original_href: "foo.rs".to_string(),
            source_line,
        }
    }

    // ── heading nesting ───────────────────────────────────────────────────────

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
        // After a second h2 the first h2 is no longer in scope.
        let content = "# Top\n## First\n## Second\n[label](foo.rs)\n";
        let links = parse_fragment_links(content);
        let result = augment(&links, content);
        assert_eq!(result[0].heading_chain, vec!["Top", "Second"]);
    }

    // ── code-fenced regions ───────────────────────────────────────────────────

    #[test]
    fn links_inside_code_block_are_excluded_by_parser() {
        // `parse_fragment_links` already scrubs code blocks; links inside them
        // are never returned. Verify augment receives an empty slice and returns
        // an empty vec.
        let content = "```\n[label](foo.rs)\n```\n";
        let links = parse_fragment_links(content);
        assert!(
            links.is_empty(),
            "parser must not return links inside code blocks"
        );
        let result = augment(&links, content);
        assert!(result.is_empty());
    }

    #[test]
    fn heading_inside_code_block_not_in_chain() {
        // A heading inside a fenced block must not appear in the chain for
        // a link that comes after the block.
        let content = "# Real\n```\n# Fake\n```\n[label](foo.rs)\n";
        let links = parse_fragment_links(content);
        assert_eq!(links.len(), 1);
        let result = augment(&links, content);
        // Only "Real" should be in the chain; "Fake" was inside a code block.
        assert_eq!(result[0].heading_chain, vec!["Real"]);
    }

    // ── start / end of file edges ─────────────────────────────────────────────

    #[test]
    fn link_on_first_line_no_panic() {
        let content = "[label](foo.rs)\n# After\n";
        let links = parse_fragment_links(content);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].source_line, 1);
        let result = augment(&links, content);
        // source_line 1 means idx 0; start clamps to 0, end is min(1, last).
        assert!(!result[0].surrounding_text.is_empty());
        assert!(result[0].heading_chain.is_empty());
    }

    #[test]
    fn link_on_last_line_no_panic() {
        let content = "# Heading\nsome text\n[label](foo.rs)";
        let links = parse_fragment_links(content);
        assert_eq!(links.len(), 1);
        let result = augment(&links, content);
        assert!(!result[0].surrounding_text.is_empty());
    }

    #[test]
    fn surrounding_text_window_includes_adjacent_lines() {
        let content = "line one\n[label](foo.rs)\nline three\n";
        let links = parse_fragment_links(content);
        assert_eq!(links.len(), 1);
        let result = augment(&links, content);
        let text = &result[0].surrounding_text;
        assert!(
            text.contains("line one"),
            "expected prev line, got: {text:?}"
        );
        assert!(
            text.contains("line three"),
            "expected next line, got: {text:?}"
        );
    }
}
