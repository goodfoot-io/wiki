use regex::Regex;
use std::sync::OnceLock;

// ── Regex singletons ──────────────────────────────────────────────────────────

fn md_link_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Matches [text](href) — text may contain nested brackets
        Regex::new(r"\[([^\[\]]*)\]\(([^)]*)\)").unwrap()
    })
}

fn wikilink_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Matches [[...]] where the inner content does not contain '[' or ']'
        Regex::new(r"\[\[([^\[\]]+)\]\]").unwrap()
    })
}

fn url_scheme_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[a-zA-Z][a-zA-Z0-9+\-.]*://").unwrap())
}

// ── Public types ──────────────────────────────────────────────────────────────

/// Classification of a markdown fragment link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkKind {
    /// Link with a URL scheme (http://, https://, etc.)
    External,
    /// Internal link (no URL scheme).
    Internal,
}

/// A parsed `[label](path#L10-L20)` fragment link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FragmentLink {
    pub kind: LinkKind,
    /// The path portion (before `#`). For external links, the full URL.
    pub path: String,
    /// First line of the referenced range, if present.
    pub start_line: Option<u32>,
    /// Last line of the referenced range, if present.
    pub end_line: Option<u32>,
    /// The link text (the `[label]` part) - may be scrubbed if it contains code.
    pub text: String,
    /// The original, unscrubbed link text.
    pub original_text: String,
    /// The original, unscrubbed href text.
    pub original_href: String,
    /// 1-based line number in the source wiki page.
    pub source_line: usize,
}

/// A parsed `[[Title#Heading|display]]` wikilink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wikilink {
    /// Namespace prefix from `[[ns:Article]]` syntax. `None` for bare
    /// `[[Article]]` links. Populated by `parse_wikilinks` in Phase 2; until
    /// then every construction site defaults to `None`.
    pub namespace: Option<String>,
    pub title: String,
    pub heading: Option<String>,
    pub display: Option<String>,
    /// 1-based line number in the source wiki page.
    pub source_line: usize,
}

// ── Code-region scrubber ──────────────────────────────────────────────────────

/// Replace code blocks, inline code, and HTML comments with spaces of equal
/// byte length so that byte offsets (and therefore line numbers) are preserved.
pub(crate) fn scrub_non_content(content: &str) -> String {
    let bytes = content.as_bytes();
    let len = bytes.len();
    let mut out = bytes.to_vec();
    let mut i = 0;

    while i < len {
        // ── HTML comments <!-- ... --> ────────────────────────────────────
        if bytes[i..].starts_with(b"<!--") {
            if let Some(end_rel) = find_bytes(&bytes[i..], b"-->") {
                let end = i + end_rel + 3;
                blank_region(&mut out, i, end);
                i = end;
            } else {
                // Unclosed HTML comment: blank from here to EOF
                blank_region(&mut out, i, len);
                i = len;
            }
            continue;
        }

        // ── Fenced code blocks ``` or ~~~ ────────────────────────────────
        // Only at the start of a line
        if (i == 0 || bytes[i - 1] == b'\n')
            && (bytes[i..].starts_with(b"```") || bytes[i..].starts_with(b"~~~"))
        {
            let fence_char = bytes[i];
            // Count fence length (at least 3)
            let mut fence_len = 3;
            while i + fence_len < len && bytes[i + fence_len] == fence_char {
                fence_len += 1;
            }
            // Find the matching closing fence on its own line
            let fence = &bytes[i..i + fence_len];
            let body_start = i;
            // Move past opening fence line
            let mut j = i + fence_len;
            // Skip to end of opening fence line
            while j < len && bytes[j] != b'\n' {
                j += 1;
            }
            if j < len {
                j += 1; // skip newline
            }
            // Search for closing fence
            let mut found = false;
            while j < len {
                if bytes[j..].starts_with(b"\n") {
                    j += 1;
                    continue;
                }
                // Check if this line starts with the same fence
                if bytes[j..].starts_with(fence) {
                    // Verify it's followed by optional spaces then newline/EOF
                    let mut k = j + fence_len;
                    while k < len && bytes[k] == b' ' {
                        k += 1;
                    }
                    if k >= len || bytes[k] == b'\n' {
                        // Include the closing fence line
                        let close_end = if k < len { k + 1 } else { k };
                        blank_region(&mut out, body_start, close_end);
                        i = close_end;
                        found = true;
                        break;
                    }
                }
                // Skip to end of line
                while j < len && bytes[j] != b'\n' {
                    j += 1;
                }
                if j < len {
                    j += 1;
                }
            }
            if !found {
                // Unterminated fence: blank to end of file
                blank_region(&mut out, body_start, len);
                i = len;
            }
            continue;
        }

        // ── Inline code `` `...` `` or `` ``...`` `` ─────────────────────
        if bytes[i] == b'`' {
            // Count opening backticks
            let mut tick_count = 1;
            while i + tick_count < len && bytes[i + tick_count] == b'`' {
                tick_count += 1;
            }
            // Do NOT treat triple+ backticks as inline code (those are fences handled above)
            if tick_count < 3 {
                let closing = vec![b'`'; tick_count];
                // Search for matching closing backticks (not crossing a newline for single backtick)
                let search_start = i + tick_count;
                if let Some(rel) = find_bytes(&bytes[search_start..], &closing) {
                    let end = search_start + rel + tick_count;
                    // Blank the content INSIDE the backticks, but keep the backticks
                    // themselves so they are part of the parsed link text.
                    blank_region(&mut out, search_start, search_start + rel);
                    i = end;
                    continue;
                }
            } else {
                // 3+ backticks not at start of line: skip them
                i += tick_count;
                continue;
            }
        }

        i += 1;
    }

    String::from_utf8(out).expect("blanking preserves UTF-8 structure (bytes replaced with 0x20)")
}

fn blank_region(buf: &mut [u8], start: usize, end: usize) {
    for b in buf[start..end].iter_mut() {
        if *b != b'\n' {
            *b = b' ';
        }
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

// ── Fragment link parser ──────────────────────────────────────────────────────

/// Parse all fragment links from markdown `content`.
pub fn parse_fragment_links(content: &str) -> Vec<FragmentLink> {
    let scrubbed = scrub_non_content(content);
    let mut results = Vec::new();

    for cap in md_link_re().captures_iter(&scrubbed) {
        let m = cap.get(0).unwrap();
        let text = cap[1].to_string();
        let href = &cap[2];

        // Get the original, unscrubbed text from the original content
        // capture[1] corresponds to the text part
        let text_match = cap.get(1).unwrap();
        let original_text = content[text_match.start()..text_match.end()].to_string();

        // Compute 1-based source line from byte offset in scrubbed (same newlines)
        let source_line = scrubbed[..m.start()]
            .bytes()
            .filter(|&b| b == b'\n')
            .count()
            + 1;

        if url_scheme_re().is_match(href) {
            results.push(FragmentLink {
                kind: LinkKind::External,
                path: href.to_string(),
                start_line: None,
                end_line: None,
                text,
                original_text,
                original_href: href.to_string(),
                source_line,
            });
            continue;
        }

        // Split on '#' to separate path from fragment (plain line range, e.g. #L10-L20)
        let (path_part, fragment) = match href.find('#') {
            Some(idx) => (&href[..idx], Some(&href[idx + 1..])),
            None => (href as &str, None),
        };

        let (start_line, end_line) = parse_line_range(fragment);

        results.push(FragmentLink {
            kind: LinkKind::Internal,
            path: path_part.to_string(),
            start_line,
            end_line,
            text,
            original_text,
            original_href: href.to_string(),
            source_line,
        });
    }

    results
}

/// Parse `L10`, `L10-L20`, or `L10-20` fragment into (start, end).
fn parse_line_range(fragment: Option<&str>) -> (Option<u32>, Option<u32>) {
    let Some(frag) = fragment else {
        return (None, None);
    };
    // Strip leading 'L' (case-insensitive)
    let frag = frag.trim();
    if !frag.starts_with('L') && !frag.starts_with('l') {
        return (None, None);
    }
    // Truncate at the first non-line-range character so trailing markers
    // like `&<sha>` (e.g. `#L2-L7&f8b9169`) don't break parsing.
    let end_idx = frag
        .find(|c: char| !c.is_ascii_digit() && c != 'L' && c != 'l' && c != '-')
        .unwrap_or(frag.len());
    let frag = &frag[1..end_idx];
    if let Some(dash_pos) = frag.find('-') {
        let start_str = &frag[..dash_pos];
        let end_str = frag[dash_pos + 1..]
            .trim_start_matches('L')
            .trim_start_matches('l');
        let start = start_str.parse::<u32>().ok();
        let end = end_str.parse::<u32>().ok();
        (start, end)
    } else {
        let start = frag.parse::<u32>().ok();
        (start, None)
    }
}

// ── Wikilink parser ───────────────────────────────────────────────────────────

/// Parse all wikilinks from markdown `content`.
pub fn parse_wikilinks(content: &str) -> Vec<Wikilink> {
    let scrubbed = scrub_non_content(content);
    let mut results = Vec::new();

    for cap in wikilink_re().captures_iter(&scrubbed) {
        let m = cap.get(0).unwrap();
        let inner = &cap[1];
        let source_line = scrubbed[..m.start()]
            .bytes()
            .filter(|&b| b == b'\n')
            .count()
            + 1;

        // Split on '|' for display text
        let (title_part, display) = match inner.find('|') {
            Some(idx) => (&inner[..idx], Some(inner[idx + 1..].to_string())),
            None => (inner, None),
        };

        // Split title on '#' for heading fragment
        let (title, heading) = match title_part.find('#') {
            Some(idx) => (
                title_part[..idx].to_string(),
                Some(title_part[idx + 1..].to_string()),
            ),
            None => (title_part.to_string(), None),
        };

        // Skip entirely empty titles
        if title.is_empty() {
            continue;
        }

        results.push(Wikilink {
            namespace: None,
            title,
            heading,
            display,
            source_line,
        });
    }

    results
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Fragment link tests ───────────────────────────────────────────────────

    #[test]
    fn test_external_link() {
        let links = parse_fragment_links("[Google](https://google.com)");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].kind, LinkKind::External);
        assert_eq!(links[0].path, "https://google.com");
    }

    #[test]
    fn test_internal_link_with_range() {
        let links = parse_fragment_links("[Foo](src/foo.rs#L10-L20)");
        assert_eq!(links.len(), 1);
        let l = &links[0];
        assert_eq!(l.kind, LinkKind::Internal);
        assert_eq!(l.path, "src/foo.rs");
        assert_eq!(l.start_line, Some(10));
        assert_eq!(l.end_line, Some(20));
        assert_eq!(l.text, "Foo");
    }

    #[test]
    fn test_internal_link_no_fragment() {
        let links = parse_fragment_links("[F](src/f.rs)");
        assert_eq!(links.len(), 1);
        let l = &links[0];
        assert_eq!(l.kind, LinkKind::Internal);
        assert_eq!(l.path, "src/f.rs");
        assert_eq!(l.start_line, None);
        assert_eq!(l.end_line, None);
    }

    #[test]
    fn test_internal_link_with_line_range() {
        let links = parse_fragment_links("[Bar](src/bar.ts#L5)");
        assert_eq!(links.len(), 1);
        let l = &links[0];
        assert_eq!(l.kind, LinkKind::Internal);
        assert_eq!(l.path, "src/bar.ts");
        assert_eq!(l.start_line, Some(5));
        assert_eq!(l.end_line, None);
    }

    #[test]
    fn test_scoped_package_path_with_range() {
        let links = parse_fragment_links("[pkg](node_modules/@scope/pkg/index.ts#L1-L10)");
        assert_eq!(links.len(), 1);
        let l = &links[0];
        assert_eq!(l.kind, LinkKind::Internal);
        assert_eq!(l.path, "node_modules/@scope/pkg/index.ts");
        assert_eq!(l.start_line, Some(1));
        assert_eq!(l.end_line, Some(10));
    }

    #[test]
    fn test_scoped_package_path_without_fragment() {
        let links = parse_fragment_links("[pkg](node_modules/@scope/pkg/index.ts#L5)");
        assert_eq!(links.len(), 1);
        let l = &links[0];
        assert_eq!(l.kind, LinkKind::Internal);
        assert_eq!(l.path, "node_modules/@scope/pkg/index.ts");
        assert_eq!(l.start_line, Some(5));
    }

    #[test]
    fn test_source_line_tracking() {
        let content = "line one\n\n[Link](src/file.rs#L1)\n";
        let links = parse_fragment_links(content);
        assert_eq!(links[0].source_line, 3);
    }

    #[test]
    fn test_line_range_no_fragment() {
        let links = parse_fragment_links("[F](src/f.rs)");
        assert_eq!(links[0].start_line, None);
        assert_eq!(links[0].end_line, None);
    }

    // ── Wikilink tests ────────────────────────────────────────────────────────

    #[test]
    fn test_wikilink_simple() {
        let links = parse_wikilinks("See [[SomePage]].");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].title, "SomePage");
        assert!(links[0].heading.is_none());
        assert!(links[0].display.is_none());
    }

    #[test]
    fn test_wikilink_with_display() {
        let links = parse_wikilinks("See [[SomePage|display text]].");
        assert_eq!(links[0].title, "SomePage");
        assert_eq!(links[0].display.as_deref(), Some("display text"));
    }

    #[test]
    fn test_wikilink_with_heading() {
        let links = parse_wikilinks("See [[SomePage#Introduction]].");
        assert_eq!(links[0].title, "SomePage");
        assert_eq!(links[0].heading.as_deref(), Some("Introduction"));
        assert!(links[0].display.is_none());
    }

    #[test]
    fn test_wikilink_with_heading_and_display() {
        let links = parse_wikilinks("[[SomePage#Section|click here]]");
        let l = &links[0];
        assert_eq!(l.title, "SomePage");
        assert_eq!(l.heading.as_deref(), Some("Section"));
        assert_eq!(l.display.as_deref(), Some("click here"));
    }

    #[test]
    fn test_wikilink_case_preserved() {
        let links = parse_wikilinks("[[My Page Title]]");
        assert_eq!(links[0].title, "My Page Title");
    }

    #[test]
    fn test_wikilink_empty_title_skipped() {
        let links = parse_wikilinks("[[]]");
        assert_eq!(links.len(), 0);
    }

    #[test]
    fn test_wikilink_malformed_no_close() {
        let links = parse_wikilinks("[[unclosed");
        assert_eq!(links.len(), 0);
    }

    #[test]
    fn test_wikilink_source_line() {
        let content = "intro\n\n[[Target Page]]\n";
        let links = parse_wikilinks(content);
        assert_eq!(links[0].source_line, 3);
    }

    #[test]
    fn test_multiple_links_same_content() {
        let content = "[F1](a.rs#L1) and [[Wiki]] and [F2](b.rs#L2)";
        let frags = parse_fragment_links(content);
        let wikis = parse_wikilinks(content);
        assert_eq!(frags.len(), 2);
        assert_eq!(wikis.len(), 1);
    }

    // ── Code block exclusion tests ────────────────────────────────────────────

    #[test]
    fn test_fenced_code_block_excluded() {
        let content = "before\n```\n[Link](src/file.rs#L1)\n```\nafter\n";
        let links = parse_fragment_links(content);
        assert_eq!(
            links.len(),
            0,
            "links inside fenced code blocks must not be extracted"
        );
    }

    #[test]
    fn test_fenced_code_block_with_lang_excluded() {
        let content = "before\n```rust\n[Link](src/file.rs#L1)\n```\nafter\n";
        let links = parse_fragment_links(content);
        assert_eq!(links.len(), 0);
    }

    #[test]
    fn test_tilde_fence_excluded() {
        let content = "before\n~~~\n[Link](src/file.rs#L1)\n~~~\nafter\n";
        let links = parse_fragment_links(content);
        assert_eq!(links.len(), 0);
    }

    #[test]
    fn test_link_outside_code_block_extracted() {
        let content =
            "[Before](before.rs#L1)\n```\n[Inside](inside.rs#L1)\n```\n[After](after.rs#L1)\n";
        let links = parse_fragment_links(content);
        assert_eq!(links.len(), 2);
        let paths: Vec<&str> = links.iter().map(|l| l.path.as_str()).collect();
        assert!(paths.contains(&"before.rs"));
        assert!(paths.contains(&"after.rs"));
    }

    #[test]
    fn test_link_with_backticks_in_text() {
        let content = "[`src/foo.rs`](src/foo.rs)";
        let links = parse_fragment_links(content);
        assert_eq!(links.len(), 1);
        // Scrubbed text has spaces inside backticks
        assert_eq!(links[0].text, "`          `");
        // original_text has the original content
        assert_eq!(links[0].original_text, "`src/foo.rs`");
    }

    #[test]
    fn test_inline_code_excluded() {
        let content = "See `[Link](src/file.rs#L1)` for details.";
        let links = parse_fragment_links(content);
        assert_eq!(
            links.len(),
            0,
            "links inside inline code must not be extracted"
        );
    }

    #[test]
    fn test_double_backtick_inline_code_excluded() {
        let content = "See ``[Link](src/file.rs#L1)`` for details.";
        let links = parse_fragment_links(content);
        assert_eq!(links.len(), 0);
    }

    #[test]
    fn test_html_comment_excluded() {
        let content = "<!-- [Link](src/file.rs#L1) -->\n[Real](real.rs#L1)";
        let links = parse_fragment_links(content);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].path, "real.rs");
    }

    #[test]
    fn test_multiline_html_comment_excluded() {
        let content = "<!--\n[Link](src/file.rs#L1)\n-->\n[Real](real.rs#L1)";
        let links = parse_fragment_links(content);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].path, "real.rs");
    }

    #[test]
    fn test_wikilinks_in_code_block_excluded() {
        let content = "text\n```\n[[InCode]]\n```\n[[OutsideCode]]\n";
        let links = parse_wikilinks(content);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].title, "OutsideCode");
    }

    #[test]
    fn test_wikilinks_in_inline_code_excluded() {
        let content = "See `[[InCode]]` and [[OutsideCode]].";
        let links = parse_wikilinks(content);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].title, "OutsideCode");
    }

    #[test]
    fn test_wikilinks_in_html_comment_excluded() {
        let content = "<!-- [[InComment]] -->\n[[Outside]]";
        let links = parse_wikilinks(content);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].title, "Outside");
    }

    #[test]
    fn test_unclosed_html_comment_scrubbed_to_eof() {
        // Link inside an unclosed HTML comment must NOT be extracted
        let content = "<!-- [Hidden](src/file.rs#L1)\n[AlsoHidden](other.rs#L1)";
        let links = parse_fragment_links(content);
        assert_eq!(
            links.len(),
            0,
            "links inside unclosed HTML comment must not be extracted"
        );
    }

    #[test]
    fn test_unclosed_html_comment_wikilink_not_extracted() {
        let content = "Real text\n<!-- [[HiddenWiki]]\n[[AlsoHidden]]";
        let links = parse_wikilinks(content);
        assert_eq!(
            links.len(),
            0,
            "wikilinks inside unclosed HTML comment must not be extracted"
        );
    }

    #[test]
    fn test_utf8_multibyte_in_code_block_no_panic() {
        // Multi-byte UTF-8 (emoji) inside a fenced code block — must not panic
        let content = "before\n```\n😀 🎉 [Link](src/file.rs#L1)\n```\nafter\n";
        let links = parse_fragment_links(content);
        assert_eq!(
            links.len(),
            0,
            "links inside code block with emoji must not be extracted"
        );
    }

    #[test]
    fn test_utf8_multibyte_in_html_comment_no_panic() {
        // Multi-byte UTF-8 inside an HTML comment
        let content = "<!-- 😀 [Hidden](src/file.rs#L1) -->\n[Real](real.rs#L1)";
        let links = parse_fragment_links(content);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].path, "real.rs");
    }

    // ── Namespaced wikilink tests (Phase 2 contract) ─────────────────────────

    #[test]
    #[ignore = "Phase 2: parse_wikilinks should detect [[ns:Article]] prefix"]
    fn test_wikilink_with_namespace_prefix() {
        let links = parse_wikilinks("[[foo:Article]]");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].namespace.as_deref(), Some("foo"));
        assert_eq!(links[0].title, "Article");
    }

    #[test]
    #[ignore = "Phase 2: bare wikilinks have namespace None"]
    fn test_wikilink_without_namespace() {
        let links = parse_wikilinks("[[Article]]");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].namespace, None);
        assert_eq!(links[0].title, "Article");
    }
}
