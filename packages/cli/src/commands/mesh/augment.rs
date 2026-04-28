use std::sync::OnceLock;

use regex::Regex;

use crate::parser::{FragmentLink, scrub_non_content};

/// A `FragmentLink` augmented with surrounding text and heading context.
pub(crate) struct AugmentedLink {
    pub(crate) link: FragmentLink,
    /// The link's source line ± 2 lines from the *raw* (unscrubbed) source,
    /// joined with `' '`, then run through the JS `surroundingText` cleanup
    /// pipeline (backticks blanked, `[label](href)` reduced to extension-trimmed
    /// label, `[[wikilink|display]]` unwrapped to display/title, whitespace
    /// collapsed). Mirrors `parseFragmentLinks` in `mesh-scaffold-v4.mjs`.
    pub(crate) surrounding_text: String,
    /// The raw (unscrubbed) text of the link's source line.
    pub(crate) line_text: String,
    /// Stack of ATX heading texts (levels 1–6) in effect at the link's source line.
    pub(crate) heading_chain: Vec<String>,
}

/// Augment a slice of fragment links found in `content`.
pub(crate) fn augment(links: &[FragmentLink], content: &str) -> Vec<AugmentedLink> {
    let raw_lines: Vec<&str> = content.split('\n').collect();
    let scrubbed = scrub_non_content(content);
    // Heading chain is built from the *scrubbed* lines so headings inside code
    // blocks don't pollute the chain.
    let scrubbed_lines: Vec<&str> = scrubbed.split('\n').collect();
    let heading_chain_at = build_heading_chains(&scrubbed_lines);

    links
        .iter()
        .map(|link| {
            let surrounding_text = extract_surrounding_text(link.source_line, &raw_lines);
            let line_text = raw_lines
                .get(link.source_line.saturating_sub(1))
                .map(|s| (*s).to_string())
                .unwrap_or_default();
            let heading_chain = heading_chain_at
                .get(link.source_line.saturating_sub(1))
                .cloned()
                .unwrap_or_default();
            AugmentedLink {
                link: link.clone(),
                surrounding_text,
                line_text,
                heading_chain,
            }
        })
        .collect()
}

fn build_heading_chains(lines: &[&str]) -> Vec<Vec<String>> {
    let mut chains: Vec<Vec<String>> = Vec::with_capacity(lines.len());
    let mut stack: Vec<(usize, String)> = Vec::new();

    for line in lines {
        chains.push(stack.iter().map(|(_, t)| t.clone()).collect());
        if let Some((level, text)) = parse_atx_heading(line) {
            stack.retain(|(l, _)| *l < level);
            stack.push((level, text));
        }
    }
    chains
}

fn parse_atx_heading(line: &str) -> Option<(usize, String)> {
    // JS uses HEADING_RE = /^(#{1,6})\s+(.+)/ on line.trimStart()
    // Then strips [`*_[\]] and trims the captured text.
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

fn surrounding_cleanup_re() -> &'static (Regex, Regex, Regex, Regex) {
    static RE: OnceLock<(Regex, Regex, Regex, Regex)> = OnceLock::new();
    RE.get_or_init(|| {
        // 1. backtick spans
        let bt = Regex::new(r"`[^`\n]+`").unwrap();
        // 2. [label](href) → label with .ext stripped from end of label
        let md = Regex::new(r"\[([^\[\]]*)\]\(([^)]*)\)").unwrap();
        // 3. [[t]] or [[t|d]] → d ?? t
        let wl = Regex::new(r"\[\[([^\]|]+)(?:\|([^\]]*))?\]\]").unwrap();
        // 4. whitespace collapse
        let ws = Regex::new(r"\s+").unwrap();
        (bt, md, wl, ws)
    })
}

fn ext_strip_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\.[a-z]{1,5}$").unwrap())
}

/// Extract the link line ± 2 lines from the *raw* content, joined with `' '`,
/// then apply the JS cleanup pipeline.
fn extract_surrounding_text(source_line: usize, raw_lines: &[&str]) -> String {
    if raw_lines.is_empty() {
        return String::new();
    }
    let idx = source_line.saturating_sub(1);
    // JS: lines.slice(Math.max(0, sourceLine - 3), sourceLine + 2) on 1-based,
    // which yields raw_lines[idx-2 ..= idx+2] in 0-based (5-line window).
    let start = idx.saturating_sub(2);
    let end_excl = (idx + 3).min(raw_lines.len());
    let joined = raw_lines[start..end_excl].join(" ");

    let (bt, md, wl, ws) = surrounding_cleanup_re();
    // 1. backticks → ' '
    let s = bt.replace_all(&joined, " ").into_owned();
    // 2. [label](href) → label with extension stripped from end of label
    let s = md
        .replace_all(&s, |caps: &regex::Captures| {
            let label = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            ext_strip_re().replace(label, "").into_owned()
        })
        .into_owned();
    // 3. [[t|d]] → d ?? t
    let s = wl
        .replace_all(&s, |caps: &regex::Captures| {
            caps.get(2)
                .or_else(|| caps.get(1))
                .map(|m| m.as_str().to_string())
                .unwrap_or_default()
        })
        .into_owned();
    // 4. whitespace collapse + trim
    ws.replace_all(&s, " ").trim().to_string()
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
        assert!(
            links.is_empty(),
            "parser must not return links inside code blocks"
        );
        let result = augment(&links, content);
        assert!(result.is_empty());
    }

    #[test]
    fn heading_inside_code_block_not_in_chain() {
        let content = "# Real\n```\n# Fake\n```\n[label](foo.rs)\n";
        let links = parse_fragment_links(content);
        assert_eq!(links.len(), 1);
        let result = augment(&links, content);
        assert_eq!(result[0].heading_chain, vec!["Real"]);
    }

    #[test]
    fn link_on_first_line_no_panic() {
        let content = "[label](foo.rs)\n# After\n";
        let links = parse_fragment_links(content);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].source_line, 1);
        let result = augment(&links, content);
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
    fn surrounding_text_window_is_five_lines() {
        // sourceLine=3 → raw_lines[1..=5] (1-based: lines 1..=5)
        let content = "L1 alpha\nL2 beta\nL3 [label](foo.rs)\nL4 delta\nL5 epsilon\nL6 zeta\n";
        let links = parse_fragment_links(content);
        assert_eq!(links.len(), 1);
        let result = augment(&links, content);
        let text = &result[0].surrounding_text;
        // Window: L1..L5, joined with space, label substituted in for the link.
        assert!(text.contains("L1 alpha"), "got: {text:?}");
        assert!(text.contains("L5 epsilon"), "got: {text:?}");
        assert!(
            !text.contains("L6"),
            "L6 should be outside window: {text:?}"
        );
        // [label](foo.rs) → label (ext stripped from label, but no dot ext here)
        assert!(text.contains("label"));
    }

    #[test]
    fn cleanup_strips_backticks() {
        let content = "use the `Foo` widget [label](foo.rs#L1-L2) here\n";
        let links = parse_fragment_links(content);
        let result = augment(&links, content);
        let text = &result[0].surrounding_text;
        assert!(!text.contains('`'), "backticks not stripped: {text:?}");
        assert!(!text.contains("Foo"), "code-span content leaked: {text:?}");
    }

    #[test]
    fn cleanup_label_with_href_keeps_label_strips_extension() {
        let content = "click [config.ts](foo.rs#L1-L2) please\n";
        let links = parse_fragment_links(content);
        let result = augment(&links, content);
        let text = &result[0].surrounding_text;
        // Label is "config.ts" — extension stripped → "config"
        assert!(text.contains("config"));
        assert!(
            !text.contains("config.ts"),
            "extension not stripped: {text:?}"
        );
        assert!(!text.contains("foo.rs"), "href leaked: {text:?}");
    }

    #[test]
    fn cleanup_unwraps_wikilink_with_display() {
        let content = "see [[Real Title|display name]] and [label](foo.rs#L1-L2)\n";
        let links = parse_fragment_links(content);
        let result = augment(&links, content);
        let text = &result[0].surrounding_text;
        assert!(text.contains("display name"));
        assert!(!text.contains("[["), "wikilink not unwrapped: {text:?}");
    }

    #[test]
    fn cleanup_unwraps_wikilink_without_display() {
        let content = "see [[JustTitle]] and [label](foo.rs#L1-L2)\n";
        let links = parse_fragment_links(content);
        let result = augment(&links, content);
        let text = &result[0].surrounding_text;
        assert!(text.contains("JustTitle"));
        assert!(!text.contains("[["));
    }

    #[test]
    fn line_text_is_raw_unscrubbed() {
        let content = "hello `code` [label](foo.rs#L1-L2) world\n";
        let links = parse_fragment_links(content);
        let result = augment(&links, content);
        // line_text preserves the raw line including backticks.
        assert!(result[0].line_text.contains('`'));
        assert!(result[0].line_text.contains("code"));
    }
}
