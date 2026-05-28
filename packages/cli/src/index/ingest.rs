//! Parse frontmatter from blob bytes via [`crate::frontmatter`]; reject
//! non-wiki markdown (missing or empty `title`/`summary`).

use std::path::Path;

use crate::frontmatter::parse_frontmatter;

/// Indexable fields lifted out of a wiki page's frontmatter and body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WikiBlobFields {
    pub title: String,
    pub summary: String,
    pub body: String,
    /// Aliases joined by a single space — fed directly to FTS5.
    pub aliases_text: String,
    pub tags_text: String,
    pub keywords_text: String,
}

/// Parse `bytes` as a wiki page. Returns `None` when the file is not a
/// wiki page (missing/empty `title` or `summary`, unparseable YAML, or no
/// frontmatter fence at all).
pub fn parse_blob(bytes: &[u8]) -> Option<WikiBlobFields> {
    let content = std::str::from_utf8(bytes).ok()?;
    // Use an empty path as a placeholder — errors are mapped to `None` and
    // the path is only used in error messages we discard.
    let placeholder = Path::new("");
    let fm = parse_frontmatter(content, placeholder).ok()??;

    let body = strip_frontmatter(content).to_string();

    Some(WikiBlobFields {
        title: fm.title,
        summary: fm.summary,
        body,
        aliases_text: fm.aliases.join(" "),
        tags_text: fm.tags.join(" "),
        keywords_text: fm.keywords.join(" "),
    })
}

/// Return the slice of `content` after the closing `---` fence, or the
/// whole input if no fence is present.
fn strip_frontmatter(content: &str) -> &str {
    let trimmed = content.trim_start_matches('\n');
    if !trimmed.starts_with("---") {
        return content;
    }
    let after_open = &trimmed["---".len()..];
    let after_open = after_open
        .strip_prefix('\n')
        .or_else(|| after_open.strip_prefix("\r\n"));
    let Some(after_open) = after_open else {
        return content;
    };
    // Find closing fence line
    let mut offset = 0usize;
    while offset < after_open.len() {
        let line_end = after_open[offset..]
            .find('\n')
            .map(|rel| offset + rel)
            .unwrap_or(after_open.len());
        let raw = &after_open[offset..line_end];
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line == "---" {
            let next = if line_end < after_open.len() {
                line_end + 1
            } else {
                line_end
            };
            return &after_open[next..];
        }
        offset = if line_end < after_open.len() {
            line_end + 1
        } else {
            after_open.len()
        };
    }
    content
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_wiki_page() {
        let bytes = b"---\ntitle: Foo\nsummary: A foo page.\naliases: [Bar, Baz]\ntags: [t1]\nkeywords: [k1, k2]\n---\n\nBody text.\n";
        let f = parse_blob(bytes).unwrap();
        assert_eq!(f.title, "Foo");
        assert_eq!(f.summary, "A foo page.");
        assert_eq!(f.aliases_text, "Bar Baz");
        assert_eq!(f.tags_text, "t1");
        assert_eq!(f.keywords_text, "k1 k2");
        assert!(f.body.contains("Body text."));
        assert!(!f.body.contains("title:"));
    }

    #[test]
    fn rejects_missing_summary() {
        let bytes = b"---\ntitle: Foo\n---\n\nBody.\n";
        assert!(parse_blob(bytes).is_none());
    }

    #[test]
    fn rejects_missing_title() {
        let bytes = b"---\nsummary: A summary.\n---\n\nBody.\n";
        assert!(parse_blob(bytes).is_none());
    }

    #[test]
    fn rejects_no_frontmatter() {
        let bytes = b"# Just a heading\n\nBody.\n";
        assert!(parse_blob(bytes).is_none());
    }
}
