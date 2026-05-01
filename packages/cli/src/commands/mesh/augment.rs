use std::sync::OnceLock;

use regex::Regex;

use crate::parser::{FragmentLink, scrub_non_content};

/// A `FragmentLink` augmented with surrounding text and heading context.
#[derive(Clone)]
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
    /// The deepest heading text in `heading_chain` prefixed by its ATX hashes
    /// (e.g. `## Sync detection`). Empty when the link sits before any heading
    /// on the page.
    pub(crate) section_heading: String,
    /// The first prose sentence of the section the link sits under, with
    /// markdown link syntax cleaned via the same pipeline as `surrounding_text`.
    /// Empty when no prose precedes the next heading.
    pub(crate) section_opening: String,
}

/// Augment a slice of fragment links found in `content`.
pub(crate) fn augment(links: &[FragmentLink], content: &str) -> Vec<AugmentedLink> {
    let raw_lines: Vec<&str> = content.split('\n').collect();
    let scrubbed = scrub_non_content(content);
    // Heading chain is built from the *scrubbed* lines so headings inside code
    // blocks don't pollute the chain.
    let scrubbed_lines: Vec<&str> = scrubbed.split('\n').collect();
    // Heading meta must read raw lines so backtick-wrapped identifiers in
    // ATX headings (e.g. `### \`git-mesh ls\``) survive — the scrubber blanks
    // inline-code content. The fence-skip is enforced by checking the
    // scrubbed line for emptiness at heading positions.
    let heading_chain_at = build_heading_chains_from_raw(&raw_lines, &scrubbed_lines);
    let heading_meta_at = build_heading_meta_from_raw(&raw_lines, &scrubbed_lines);

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
            let meta = heading_meta_at
                .get(link.source_line.saturating_sub(1))
                .cloned()
                .unwrap_or_default();
            let (section_heading, section_opening, _degenerate, _had_code_lead) =
                extract_section_opening(meta.as_ref(), &raw_lines, &scrubbed_lines);
            AugmentedLink {
                link: link.clone(),
                surrounding_text,
                line_text,
                heading_chain,
                section_heading,
                section_opening,
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
        // Sanity-check fence state against the scrubber (it blanks fence
        // bodies entirely, so a heading inside a fence will be blank in
        // scrubbed even when our toggle slips out of sync with weird input).
        if scrubbed_lines
            .get(i)
            .map(|s| s.trim())
            .unwrap_or("")
            .is_empty()
        {
            // Don't parse a heading from a line the scrubber blanked.
            continue;
        }
        if let Some((level, text)) = parse_atx_heading(raw) {
            stack.retain(|(l, _, _)| *l < level);
            stack.push((level, i, text));
        }
    }
    out
}

/// Walk forward from the heading's line until the first prose line, returning
/// `(section_heading, section_opening)`. The heading is rendered with its ATX
/// hashes (e.g. `## Sync detection`). The opening sentence is cleaned via the
/// same pipeline as `extract_surrounding_text` and truncated at the first
/// sentence terminator (`. `, `! `, `? `, end-of-paragraph).
///
/// Documented edge cases:
/// - No heading above link: `section_heading = ""`, opening walks from the top
///   of the file to the first prose line.
/// - Section's first content is a list item: the bullet text is treated as
///   prose (leading `- ` or `* ` stripped) — produces *something* reviewable.
/// - Section's first content is a fenced code block: skipped entirely; the
///   first prose line after the fence is returned. Empty when the section
///   never produces prose before the next heading.
pub(crate) fn extract_section_opening(
    meta: Option<&(usize, usize, String)>,
    raw_lines: &[&str],
    scrubbed_lines: &[&str],
) -> (String, String, bool, bool) {
    let (heading_text, start_idx) = match meta {
        Some((level, idx, text)) => {
            let hashes = "#".repeat(*level);
            (format!("{hashes} {text}"), *idx + 1)
        }
        None => (String::new(), 0),
    };
    let (opening, degenerate, had_code_lead) =
        walk_section_opening(start_idx, raw_lines, scrubbed_lines);
    (heading_text, opening, degenerate, had_code_lead)
}

/// Walk forward collecting prose paragraphs. Returns
/// `(opening, degenerate, had_code_span_lead)`.
///
/// `degenerate` is true when no real prose paragraph could be found before the
/// next heading — the best-available text is still emitted so the reviewer has
/// *something* to read, but the renderer attaches a `DegenerateExcerpt` warn.
///
/// A candidate paragraph is "degenerate" when, after marker-stripping and
/// prose cleanup, it: has no alphabetic content, is shorter than 12 chars, ends
/// with `:` (a code-block intro), is just a list marker, or is bold-label-only
/// (`**Where:**`). When the first candidate is degenerate, walk forward to the
/// next paragraph and prefer it; if none is found, return the best degenerate
/// candidate we saw with `degenerate = true`.
fn walk_section_opening(
    start_idx: usize,
    raw_lines: &[&str],
    _scrubbed_lines: &[&str],
) -> (String, bool, bool) {
    let mut i = start_idx;
    // Skip YAML frontmatter when starting from the top of the file.
    if i == 0 && raw_lines.first().map(|l| l.trim()) == Some("---") {
        let mut j = 1;
        while j < raw_lines.len() && raw_lines[j].trim() != "---" {
            j += 1;
        }
        if j < raw_lines.len() {
            i = j + 1; // step past the closing fence
        }
    }
    let mut in_fence = false;
    let mut best_degenerate: Option<(String, bool)> = None;
    while i < raw_lines.len() {
        let raw = raw_lines[i];
        let trimmed_raw = raw.trim();

        // Toggle fenced code block state on raw lines (the scrubber blanks
        // them out, but the fence markers themselves can survive).
        if trimmed_raw.starts_with("```") || trimmed_raw.starts_with("~~~") {
            in_fence = !in_fence;
            i += 1;
            continue;
        }
        if in_fence {
            i += 1;
            continue;
        }

        // Stop at the next ATX heading (end of section).
        if parse_atx_heading(raw).is_some() {
            break;
        }

        if trimmed_raw.is_empty() {
            i += 1;
            continue;
        }

        // Skip GitHub-flavored-markdown table blocks entirely — pipes don't
        // render usefully inside `# Source:` comments. The renderer would
        // rather see the prose paragraph that comes after.
        if trimmed_raw.starts_with('|') {
            while i < raw_lines.len() {
                let t = raw_lines[i].trim();
                if t.is_empty() || !t.starts_with('|') {
                    break;
                }
                i += 1;
            }
            continue;
        }

        // Strip a leading bullet, ordered-list marker (`1. `, `2) `), or both,
        // so list-led openings still produce a sentence.
        let candidate = strip_leading_marker(trimmed_raw);

        // Collect the prose paragraph (until blank line, heading, fence, or
        // table) so a sentence that wraps lines still terminates correctly.
        let mut paragraph = candidate.to_string();
        let mut j = i + 1;
        while j < raw_lines.len() {
            let next_raw = raw_lines[j].trim();
            if next_raw.is_empty() {
                break;
            }
            if parse_atx_heading(raw_lines[j]).is_some() {
                break;
            }
            if next_raw.starts_with("```")
                || next_raw.starts_with("~~~")
                || next_raw.starts_with('|')
            {
                break;
            }
            paragraph.push(' ');
            paragraph.push_str(next_raw);
            j += 1;
        }

        let had_code_lead = leading_token_was_code_span(&paragraph);
        let cleaned = clean_prose_line(&paragraph);
        let truncated = truncate_to_sentence(&cleaned);
        if !is_degenerate(&truncated) {
            return (truncated, false, had_code_lead);
        }
        if best_degenerate.is_none() {
            best_degenerate = Some((truncated, had_code_lead));
        }
        i = j;
    }
    match best_degenerate {
        Some((s, code_lead)) => (s, true, code_lead),
        None => (String::new(), false, false),
    }
}

fn strip_leading_marker(s: &str) -> &str {
    let s = s
        .strip_prefix("- ")
        .or_else(|| s.strip_prefix("* "))
        .or_else(|| s.strip_prefix("+ "))
        .unwrap_or(s);
    // Ordered-list markers: `1. `, `12) `, etc. Hand-rolled to avoid pulling
    // in a regex when a small loop suffices.
    let bytes = s.as_bytes();
    let mut k = 0;
    while k < bytes.len() && bytes[k].is_ascii_digit() {
        k += 1;
    }
    if k > 0 && k < bytes.len() && (bytes[k] == b'.' || bytes[k] == b')') {
        let after = &s[k + 1..];
        if let Some(rest) = after.strip_prefix(' ') {
            return rest;
        }
    }
    s
}

/// Was the very first non-whitespace token in the paragraph wrapped in
/// inline code (i.e. a backtick span)? Used to gate the headless-predicate
/// detector: we only flag the anti-pattern when the leading subject of the
/// sentence was originally a code-spanned identifier.
fn leading_token_was_code_span(s: &str) -> bool {
    let t = s.trim_start();
    t.starts_with('`')
}

fn is_degenerate(s: &str) -> bool {
    let t = s.trim();
    if t.is_empty() {
        return true;
    }
    if !t.chars().any(|c| c.is_alphabetic()) {
        return true;
    }
    if t.ends_with(':') {
        return true;
    }
    // Bold-label-only: e.g. "**Where:**" reduces under cleanup to "Where:";
    // we already catch trailing-colon. Also catch a lone bold span.
    if t.len() < 12 {
        return true;
    }
    false
}

fn clean_prose_line(s: &str) -> String {
    let (bt, md, wl, ws) = surrounding_cleanup_re();
    // Preserve identifier text inside inline code spans — `Foo` becomes Foo,
    // not a blank. The backtick characters themselves drop out (they're
    // harmless inside `#` shell comments either way), but the content has to
    // survive so excerpts like "the parser is `parse_args`" don't collapse
    // to dangling parens or mid-sentence periods.
    let s = bt
        .replace_all(s, |caps: &regex::Captures| {
            let m = caps.get(0).map(|m| m.as_str()).unwrap_or("");
            // Strip the surrounding backticks; keep the content.
            m.trim_matches('`').to_string()
        })
        .into_owned();
    let s = md
        .replace_all(&s, |caps: &regex::Captures| {
            let label = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            ext_strip_re().replace(label, "").into_owned()
        })
        .into_owned();
    let s = wl
        .replace_all(&s, |caps: &regex::Captures| {
            caps.get(2)
                .or_else(|| caps.get(1))
                .map(|m| m.as_str().to_string())
                .unwrap_or_default()
        })
        .into_owned();
    ws.replace_all(&s, " ").trim().to_string()
}

fn truncate_to_sentence(s: &str) -> String {
    // Find the first occurrence of `. `, `! `, `? `, OR end-of-string with
    // those terminators. Keep the terminator.
    let bytes = s.as_bytes();
    for i in 0..bytes.len() {
        let c = bytes[i];
        if c == b'.' || c == b'!' || c == b'?' {
            let next = bytes.get(i + 1).copied();
            if matches!(next, Some(b' ') | None) {
                return s[..=i].to_string();
            }
        }
    }
    s.to_string()
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
    // Strip wrappers but PRESERVE backtick contents — `### \`git-mesh ls\``
    // collapses to "git-mesh ls" so the rendered heading and derived slug
    // both carry the identifier instead of an empty string.
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
    fn section_opening_link_directly_under_heading() {
        let content =
            "## Sync detection\nThe WikiIndex sync detects changes. Then more.\n[label](foo.rs)\n";
        let links = parse_fragment_links(content);
        let result = augment(&links, content);
        assert_eq!(result[0].section_heading, "## Sync detection");
        assert_eq!(
            result[0].section_opening,
            "The WikiIndex sync detects changes."
        );
    }

    #[test]
    fn section_opening_skips_blanks_and_finds_first_prose() {
        let content =
            "## H\n\n\nFirst prose line. Second sentence.\n\nmore later [label](foo.rs)\n";
        let links = parse_fragment_links(content);
        let result = augment(&links, content);
        assert_eq!(result[0].section_heading, "## H");
        assert_eq!(result[0].section_opening, "First prose line.");
    }

    #[test]
    fn section_opening_uses_deepest_nested_heading() {
        let content = "# Top\n\nintro\n\n## Mid\n\nmid prose.\n\n### Deep\n\nDeep prose here. Tail.\n[label](foo.rs)\n";
        let links = parse_fragment_links(content);
        let result = augment(&links, content);
        assert_eq!(result[0].section_heading, "### Deep");
        assert_eq!(result[0].section_opening, "Deep prose here.");
    }

    #[test]
    fn section_opening_no_heading_above_walks_from_top() {
        let content = "Top of file prose. More.\n\n[label](foo.rs)\n# After\n";
        let links = parse_fragment_links(content);
        let result = augment(&links, content);
        assert_eq!(result[0].section_heading, "");
        assert_eq!(result[0].section_opening, "Top of file prose.");
    }

    #[test]
    fn section_opening_skips_code_fence_then_finds_prose() {
        let content =
            "## H\n\n```\ncode block\n```\n\nReal prose here. Then more.\n[label](foo.rs)\n";
        let links = parse_fragment_links(content);
        let result = augment(&links, content);
        assert_eq!(result[0].section_heading, "## H");
        assert_eq!(result[0].section_opening, "Real prose here.");
    }

    #[test]
    fn section_opening_treats_list_item_as_prose() {
        // Documented behavior: when the first content under a heading is a
        // list, the bullet marker is stripped and the item text is returned —
        // *something* reviewable beats nothing.
        let content = "## H\n\n- bullet item content. trailing.\n\n[label](foo.rs)\n";
        let links = parse_fragment_links(content);
        let result = augment(&links, content);
        assert_eq!(result[0].section_heading, "## H");
        assert_eq!(result[0].section_opening, "bullet item content.");
    }

    #[test]
    fn section_opening_cleans_markdown_link_syntax() {
        let content = "## H\n\nThe handler [handleCharge](src/charge.ts#L1-L5) validates input.\n[label](foo.rs)\n";
        let links = parse_fragment_links(content);
        let result = augment(&links, content);
        // Heading source = first link; both share section. Both have same opening.
        let opening = &result[0].section_opening;
        assert!(opening.contains("handleCharge"), "got: {opening:?}");
        assert!(!opening.contains("("), "link href leaked: {opening:?}");
    }

    #[test]
    fn heading_with_backtick_identifier_preserves_content() {
        let content = "## `git-mesh ls`\n\nThe command does X. [label](foo.rs#L1-L2)\n";
        let links = parse_fragment_links(content);
        let result = augment(&links, content);
        assert_eq!(result[0].section_heading, "## git-mesh ls");
        assert_eq!(result[0].section_opening, "The command does X.");
    }

    #[test]
    fn section_opening_keeps_inline_code_content() {
        let content = "## H\n\nThe parser entrypoint is `parse_args`. [label](foo.rs#L1-L2)\n";
        let links = parse_fragment_links(content);
        let result = augment(&links, content);
        assert_eq!(
            result[0].section_opening,
            "The parser entrypoint is parse_args."
        );
    }

    #[test]
    fn section_opening_walks_past_bold_label_only() {
        let content = "## H\n\n**Where:**\n\nReal prose paragraph here. [label](foo.rs#L1-L2)\n";
        let links = parse_fragment_links(content);
        let result = augment(&links, content);
        assert_eq!(result[0].section_opening, "Real prose paragraph here.");
    }

    #[test]
    fn section_opening_skips_table_block() {
        let content = "## H\n\n| col | val |\n|---|---|\n| a | b |\n\nProse after table. [label](foo.rs#L1-L2)\n";
        let links = parse_fragment_links(content);
        let result = augment(&links, content);
        assert_eq!(result[0].section_opening, "Prose after table.");
    }

    #[test]
    fn section_opening_strips_ordered_list_marker() {
        let content =
            "## H\n\n1. Validates the request payload before dispatch. [label](foo.rs#L1-L2)\n";
        let links = parse_fragment_links(content);
        let result = augment(&links, content);
        assert_eq!(
            result[0].section_opening,
            "Validates the request payload before dispatch."
        );
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
