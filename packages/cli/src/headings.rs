use std::collections::HashMap;

// ── Public types ──────────────────────────────────────────────────────────────

/// A parsed heading from a markdown file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    /// The raw heading text (after the `#` markers).
    pub text: String,
    /// The GitHub-style slug for this heading (with duplicate suffixes applied).
    pub slug: String,
    /// 1-based line number in the source file.
    pub line: usize,
}

// ── Slug algorithm ────────────────────────────────────────────────────────────

/// Compute the GitHub heading slug for `text`.
///
/// Algorithm (matches GitHub's actual behavior):
/// 1. Lowercase the entire string.
/// 2. Remove all characters that are not Unicode letters, digits, hyphens, or spaces/tabs.
/// 3. Replace each space/tab with a hyphen (spaces are NOT collapsed — `"a  b"` → `"a--b"`).
/// 4. Trim leading and trailing hyphens.
pub fn github_slug(text: &str) -> String {
    let lower = text.to_lowercase();

    let mut result = String::new();
    for ch in lower.chars() {
        if ch.is_alphanumeric() || ch == '-' {
            result.push(ch);
        } else if ch == ' ' || ch == '\t' {
            result.push('-');
        }
        // All other characters (punctuation, symbols) are dropped
    }

    // Trim trailing hyphens
    let trimmed = result.trim_end_matches('-');
    // Trim leading hyphens
    trimmed.trim_start_matches('-').to_string()
}

// ── Heading extraction ────────────────────────────────────────────────────────

/// Parse all markdown headings from `content`, computing slugs with duplicate suffixes.
///
/// Returns headings in document order. The first occurrence of a slug has no suffix;
/// subsequent occurrences get `-1`, `-2`, etc. (matching GitHub behavior).
pub fn extract_headings(content: &str) -> Vec<Heading> {
    let mut slug_counts: HashMap<String, usize> = HashMap::new();
    let mut headings = Vec::new();

    for (line_idx, line) in content.lines().enumerate() {
        let line_num = line_idx + 1;
        let Some(text) = parse_heading_line(line) else {
            continue;
        };

        let base_slug = github_slug(text);
        let count = slug_counts.entry(base_slug.clone()).or_insert(0);
        let slug = if *count == 0 {
            base_slug.clone()
        } else {
            format!("{}-{}", base_slug, count)
        };
        *count += 1;

        headings.push(Heading {
            text: text.to_string(),
            slug,
            line: line_num,
        });
    }

    headings
}

/// Extract the heading text from a line starting with one or more `#` characters.
/// Returns `None` if the line is not a heading.
fn parse_heading_line(line: &str) -> Option<&str> {
    if !line.starts_with('#') {
        return None;
    }
    let rest = line.trim_start_matches('#');
    // Must be followed by a space (ATX heading syntax)
    if !rest.starts_with(' ') {
        return None;
    }
    Some(rest.trim())
}

// ── Heading resolution ────────────────────────────────────────────────────────

/// Check whether `fragment` matches any heading in `headings`.
///
/// The fragment is slugified before comparison, so both the slug form
/// (`my-section`) and the raw heading text (`My Section`) resolve to the
/// same heading. Duplicate-suffix slugs (`foo-1`) survive slugification
/// unchanged and continue to match.
pub fn resolve_heading(fragment: &str, headings: &[Heading]) -> bool {
    let fragment_slug = github_slug(fragment);
    headings.iter().any(|h| h.slug == fragment_slug)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Slug tests ────────────────────────────────────────────────────────────

    #[test]
    fn test_slug_simple() {
        assert_eq!(github_slug("Hello World"), "hello-world");
    }

    #[test]
    fn test_slug_lowercase() {
        assert_eq!(github_slug("Hello"), "hello");
    }

    #[test]
    fn test_slug_punctuation_stripped() {
        assert_eq!(github_slug("Hello, World!"), "hello-world");
    }

    #[test]
    fn test_slug_spaces_each_become_hyphen() {
        // Spaces are not collapsed; each space becomes its own hyphen
        assert_eq!(github_slug("A B C"), "a-b-c");
    }

    #[test]
    fn test_slug_leading_trailing_hyphens_trimmed() {
        // A leading '#' would normally be stripped by parse_heading_line,
        // but test the slug function directly with edge cases.
        assert_eq!(github_slug("-foo-"), "foo");
        assert_eq!(github_slug("foo-"), "foo");
        assert_eq!(github_slug("-foo"), "foo");
    }

    #[test]
    fn test_slug_unicode_letters_preserved() {
        // Unicode letters should be kept and lowercased
        assert_eq!(github_slug("Ångström"), "ångström");
        assert_eq!(github_slug("Café"), "café");
        assert_eq!(github_slug("日本語"), "日本語");
    }

    #[test]
    fn test_slug_hyphen_preserved() {
        assert_eq!(github_slug("foo-bar"), "foo-bar");
    }

    #[test]
    fn test_slug_digits_preserved() {
        assert_eq!(github_slug("Step 1 Setup"), "step-1-setup");
    }

    #[test]
    fn test_slug_all_punctuation() {
        // All punctuation → empty string
        assert_eq!(github_slug("!!!"), "");
    }

    #[test]
    fn test_slug_empty() {
        assert_eq!(github_slug(""), "");
    }

    #[test]
    fn test_slug_github_examples() {
        // Verified against GitHub's actual heading slug behavior
        assert_eq!(github_slug("Contributing"), "contributing");
        assert_eq!(github_slug("Table of Contents"), "table-of-contents");
        assert_eq!(github_slug("C++ example"), "c-example");
        // '&' is stripped; the two surrounding spaces each become a hyphen
        assert_eq!(github_slug("foo & bar"), "foo--bar");
    }

    #[test]
    fn test_slug_multiple_spaces_not_collapsed() {
        // GitHub does NOT collapse multiple spaces — each space becomes a hyphen
        assert_eq!(github_slug("A  B"), "a--b");
    }

    // ── Extraction tests ──────────────────────────────────────────────────────

    #[test]
    fn test_extract_simple_headings() {
        let content = "# H1\n\n## H2\n\n### H3\n";
        let headings = extract_headings(content);
        assert_eq!(headings.len(), 3);
        assert_eq!(headings[0].text, "H1");
        assert_eq!(headings[0].slug, "h1");
        assert_eq!(headings[0].line, 1);
        assert_eq!(headings[1].text, "H2");
        assert_eq!(headings[1].slug, "h2");
        assert_eq!(headings[1].line, 3);
        assert_eq!(headings[2].text, "H3");
        assert_eq!(headings[2].slug, "h3");
        assert_eq!(headings[2].line, 5);
    }

    #[test]
    fn test_extract_no_headings() {
        let content = "Just text.\n\nAnother paragraph.\n";
        assert!(extract_headings(content).is_empty());
    }

    #[test]
    fn test_extract_invalid_heading_no_space() {
        // `#NoSpace` is NOT a valid ATX heading
        let content = "#NoSpace\n# Valid Heading\n";
        let headings = extract_headings(content);
        assert_eq!(headings.len(), 1);
        assert_eq!(headings[0].text, "Valid Heading");
    }

    // ── Duplicate suffix tests ────────────────────────────────────────────────

    #[test]
    fn test_duplicate_headings_get_suffixes() {
        let content = "## Introduction\n\n## Introduction\n\n## Introduction\n";
        let headings = extract_headings(content);
        assert_eq!(headings.len(), 3);
        assert_eq!(headings[0].slug, "introduction");
        assert_eq!(headings[1].slug, "introduction-1");
        assert_eq!(headings[2].slug, "introduction-2");
    }

    #[test]
    fn test_duplicate_suffix_unique_headings_not_affected() {
        let content = "## Alpha\n\n## Beta\n\n## Alpha\n";
        let headings = extract_headings(content);
        assert_eq!(headings[0].slug, "alpha");
        assert_eq!(headings[1].slug, "beta");
        assert_eq!(headings[2].slug, "alpha-1");
    }

    // ── Unicode tests ─────────────────────────────────────────────────────────

    #[test]
    fn test_unicode_heading_preserved() {
        let content = "## Ångström Units\n";
        let headings = extract_headings(content);
        assert_eq!(headings[0].slug, "ångström-units");
    }

    #[test]
    fn test_cjk_heading_preserved() {
        let content = "## 日本語\n";
        let headings = extract_headings(content);
        assert_eq!(headings[0].slug, "日本語");
    }

    // ── Resolution tests ──────────────────────────────────────────────────────

    #[test]
    fn test_resolve_heading_found() {
        let content = "## Introduction\n";
        let headings = extract_headings(content);
        assert!(resolve_heading("introduction", &headings));
    }

    #[test]
    fn test_resolve_heading_not_found() {
        let content = "## Introduction\n";
        let headings = extract_headings(content);
        assert!(!resolve_heading("conclusion", &headings));
    }

    #[test]
    fn test_resolve_heading_case_insensitive() {
        let content = "## My Section\n";
        let headings = extract_headings(content);
        assert!(resolve_heading("my-section", &headings));
    }

    #[test]
    fn test_resolve_heading_raw_text_accepted() {
        // Raw heading text — what the `Markdown Link To Wiki` suggester emits —
        // must resolve, not just the slug form.
        let content = "### Code layout: six bounded-context packages\n";
        let headings = extract_headings(content);
        assert!(resolve_heading("Code layout: six bounded-context packages", &headings));
        assert!(resolve_heading("code-layout-six-bounded-context-packages", &headings));
        // The colon-retained form also resolves: the colon is stripped during
        // slugification, leaving the same `code-layout-six-...` slug.
        assert!(resolve_heading("code-layout:-six-bounded-context-packages", &headings));
    }

    #[test]
    fn test_resolve_heading_plain_raw_text() {
        let content = "## Plain heading\n";
        let headings = extract_headings(content);
        assert!(resolve_heading("Plain heading", &headings));
        assert!(resolve_heading("plain-heading", &headings));
    }

    #[test]
    fn test_resolve_duplicate_heading_with_suffix() {
        let content = "## Foo\n\n## Foo\n";
        let headings = extract_headings(content);
        assert!(resolve_heading("foo", &headings));
        assert!(resolve_heading("foo-1", &headings));
        assert!(!resolve_heading("foo-2", &headings));
    }
}
