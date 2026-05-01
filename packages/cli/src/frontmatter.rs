use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

// ── Error types ───────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum FrontmatterError {
    #[error("Add a `title` field.")]
    MissingTitle { path: PathBuf },

    #[error("`title` must be a non-empty string.")]
    EmptyTitle { path: PathBuf },

    #[error("`title` must be a string.")]
    InvalidTitleType { path: PathBuf },

    #[error("`aliases` must be an array of non-empty strings.")]
    InvalidAliases { path: PathBuf },

    #[error("`tags` must be an array of non-empty strings.")]
    InvalidTags { path: PathBuf },

    #[error("`keywords` must be an array of non-empty strings.")]
    InvalidKeywords { path: PathBuf },

    #[error("Add a `summary` field — a one-line description of the page.")]
    MissingSummary { path: PathBuf },

    #[error("`summary` must be a non-empty string.")]
    EmptySummary { path: PathBuf },

    #[error("`summary` must be a string.")]
    InvalidSummaryType { path: PathBuf },

    #[error("Fix the YAML syntax error: {message}")]
    YamlParse { path: PathBuf, message: String },

    #[error("`{title}` is a reserved command name. Choose a different title.")]
    ReservedTitle { path: PathBuf, title: String },
}

/// Command names that cannot be used as page titles or aliases.
///
/// These are reserved to prevent ambiguity with `wiki <title>` default dispatch.
pub const RESERVED_TITLES: &[&str] = &[
    "check", "pin", "stale", "links", "list", "summary", "print",
];

// ── Raw deserialization helper ────────────────────────────────────────────────

/// Intermediate struct that accepts loose YAML types so we can validate manually.
#[derive(Debug, Deserialize)]
struct RawFrontmatter {
    title: Option<serde_yaml::Value>,
    aliases: Option<serde_yaml::Value>,
    tags: Option<serde_yaml::Value>,
    keywords: Option<serde_yaml::Value>,
    summary: Option<serde_yaml::Value>,
    namespace: Option<serde_yaml::Value>,
}

// ── Public types ──────────────────────────────────────────────────────────────

/// Validated, parsed frontmatter for a single wiki page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frontmatter {
    pub title: String,
    pub aliases: Vec<String>,
    pub tags: Vec<String>,
    pub keywords: Vec<String>,
    pub summary: String,
    /// Optional namespace assignment, only meaningful for `*.wiki.md` files.
    pub namespace: Option<String>,
}

// ── Parsing ───────────────────────────────────────────────────────────────────

/// Extract only the title from frontmatter, without full validation.
///
/// Returns `None` if there is no frontmatter block, the YAML is unparseable,
/// or the title field is absent, empty, or not a string.
pub fn parse_title(content: &str) -> Option<String> {
    let yaml = extract_yaml_block(content)?;
    let raw: RawFrontmatter = serde_yaml::from_str(yaml).ok()?;
    match raw.title? {
        serde_yaml::Value::String(s) if !s.is_empty() => Some(s),
        _ => None,
    }
}

/// Extract and parse YAML frontmatter from `content`.
///
/// Returns `None` if the content does not begin with a `---` fence.
/// Returns `Err` on YAML parse failure or field validation failure.
pub fn parse_frontmatter(
    content: &str,
    path: &Path,
) -> Result<Option<Frontmatter>, FrontmatterError> {
    let Some(yaml) = extract_yaml_block(content) else {
        return Ok(None);
    };

    let raw: RawFrontmatter =
        serde_yaml::from_str(yaml).map_err(|e| FrontmatterError::YamlParse {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;

    // Validate title
    let title = match raw.title {
        None => {
            return Err(FrontmatterError::MissingTitle {
                path: path.to_path_buf(),
            });
        }
        Some(serde_yaml::Value::String(s)) => {
            if s.is_empty() {
                return Err(FrontmatterError::EmptyTitle {
                    path: path.to_path_buf(),
                });
            }
            s
        }
        Some(_) => {
            return Err(FrontmatterError::InvalidTitleType {
                path: path.to_path_buf(),
            });
        }
    };

    if RESERVED_TITLES.contains(&title.to_lowercase().as_str()) {
        return Err(FrontmatterError::ReservedTitle {
            path: path.to_path_buf(),
            title,
        });
    }

    // Validate aliases
    let aliases = match raw.aliases {
        None => vec![],
        Some(serde_yaml::Value::Sequence(seq)) => {
            let mut result = Vec::with_capacity(seq.len());
            for v in seq {
                match v {
                    serde_yaml::Value::String(s) if !s.is_empty() => result.push(s),
                    _ => {
                        return Err(FrontmatterError::InvalidAliases {
                            path: path.to_path_buf(),
                        });
                    }
                }
            }
            result
        }
        Some(_) => {
            return Err(FrontmatterError::InvalidAliases {
                path: path.to_path_buf(),
            });
        }
    };

    // Validate tags
    let tags = match raw.tags {
        None => vec![],
        Some(serde_yaml::Value::Sequence(seq)) => {
            let mut result = Vec::with_capacity(seq.len());
            for v in seq {
                match v {
                    serde_yaml::Value::String(s) if !s.is_empty() => result.push(s),
                    _ => {
                        return Err(FrontmatterError::InvalidTags {
                            path: path.to_path_buf(),
                        });
                    }
                }
            }
            result
        }
        Some(_) => {
            return Err(FrontmatterError::InvalidTags {
                path: path.to_path_buf(),
            });
        }
    };

    // Validate keywords
    let keywords = match raw.keywords {
        None => vec![],
        Some(serde_yaml::Value::Sequence(seq)) => {
            let mut result = Vec::with_capacity(seq.len());
            for v in seq {
                match v {
                    serde_yaml::Value::String(s) if !s.is_empty() => result.push(s),
                    _ => {
                        return Err(FrontmatterError::InvalidKeywords {
                            path: path.to_path_buf(),
                        });
                    }
                }
            }
            result
        }
        Some(_) => {
            return Err(FrontmatterError::InvalidKeywords {
                path: path.to_path_buf(),
            });
        }
    };

    // Validate summary
    let summary = match raw.summary {
        None => {
            return Err(FrontmatterError::MissingSummary {
                path: path.to_path_buf(),
            });
        }
        Some(serde_yaml::Value::String(s)) => {
            if s.is_empty() {
                return Err(FrontmatterError::EmptySummary {
                    path: path.to_path_buf(),
                });
            }
            s
        }
        Some(_) => {
            return Err(FrontmatterError::InvalidSummaryType {
                path: path.to_path_buf(),
            });
        }
    };

    let namespace = match raw.namespace {
        None => None,
        Some(serde_yaml::Value::String(s)) if !s.is_empty() => Some(s),
        Some(serde_yaml::Value::String(_)) => None,
        Some(_) => None,
    };

    Ok(Some(Frontmatter {
        title,
        aliases,
        tags,
        keywords,
        summary,
        namespace,
    }))
}

/// Extract the raw YAML string from the leading `---` block.
fn extract_yaml_block(content: &str) -> Option<&str> {
    let content = content.trim_start_matches('\n');
    if !content.starts_with("---") {
        return None;
    }
    // Move past opening "---" line
    let after_open = &content["---".len()..];
    // The opening fence may be followed by a newline
    let after_open = if let Some(s) = after_open.strip_prefix('\n') {
        s
    } else {
        // "---" must be on its own line
        after_open.strip_prefix("\r\n")?
    };

    // Find closing "---" on its own line
    let close = find_close_fence(after_open)?;
    Some(&after_open[..close])
}

/// Find the byte offset of the start of a `---` close fence within `s`.
///
/// Handles both LF (`\n`) and CRLF (`\r\n`) line endings by computing byte
/// offsets directly from the original string rather than from `lines()` lengths.
fn find_close_fence(s: &str) -> Option<usize> {
    let mut offset = 0;
    while offset < s.len() {
        // Find the end of the current line
        let line_end = s[offset..].find('\n').map(|rel| offset + rel);
        let (line_content, next_offset) = match line_end {
            Some(newline_pos) => {
                // The line content excluding the newline (and any \r before it)
                let raw = &s[offset..newline_pos];
                let line = raw.strip_suffix('\r').unwrap_or(raw);
                (line, newline_pos + 1) // +1 to skip the '\n'
            }
            None => {
                // Last line with no trailing newline
                (&s[offset..], s.len())
            }
        };
        if line_content == "---" {
            return Some(offset);
        }
        offset = next_offset;
    }
    None
}

// ── Title/alias index ─────────────────────────────────────────────────────────

/// A collision error describing which page defines a conflicting alias (or title).
#[derive(Debug, PartialEq, Eq)]
pub struct CollisionError {
    /// The normalized (case-folded) key that collides.
    pub key: String,
    /// The page that is defining the conflicting key.
    pub offending_path: PathBuf,
    /// The page that already holds the key.
    pub existing_path: PathBuf,
}

/// Build a case-insensitive title/alias index from a list of `(path, frontmatter)` pairs.
///
/// Returns `(index, collisions)`. The index maps case-folded keys to file paths.
/// Collisions are reported on the page defining the conflicting alias/title, not the holder.
pub fn build_index(
    pages: &[(PathBuf, Frontmatter)],
) -> (HashMap<String, PathBuf>, Vec<CollisionError>) {
    let mut index: HashMap<String, PathBuf> = HashMap::new();
    let mut collisions: Vec<CollisionError> = Vec::new();

    for (path, fm) in pages {
        // Insert title
        let title_key = fm.title.to_lowercase();
        insert_key(&mut index, &mut collisions, title_key, path);

        // Insert aliases
        for alias in &fm.aliases {
            let alias_key = alias.to_lowercase();
            insert_key(&mut index, &mut collisions, alias_key, path);
        }
    }

    (index, collisions)
}

fn insert_key(
    index: &mut HashMap<String, PathBuf>,
    collisions: &mut Vec<CollisionError>,
    key: String,
    path: &Path,
) {
    if let Some(existing) = index.get(&key) {
        if existing != path {
            collisions.push(CollisionError {
                key: key.clone(),
                offending_path: path.to_path_buf(),
                existing_path: existing.clone(),
            });
            // Do not overwrite — keep the first holder in the index
        }
    } else {
        index.insert(key, path.to_path_buf());
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    // ── Parsing tests ─────────────────────────────────────────────────────────

    #[test]
    fn test_valid_full_frontmatter() {
        let content = "---\ntitle: My Page\naliases:\n  - alias one\n  - alias two\ntags:\n  - tag1\n  - tag2\nsummary: A summary.\n---\nbody\n";
        let fm = parse_frontmatter(content, &p("page.md")).unwrap().unwrap();
        assert_eq!(fm.title, "My Page");
        assert_eq!(fm.aliases, vec!["alias one", "alias two"]);
        assert_eq!(fm.tags, vec!["tag1", "tag2"]);
        assert_eq!(fm.summary, "A summary.");
    }

    #[test]
    fn test_valid_title_only() {
        let content = "---\ntitle: Simple\nsummary: A simple page.\n---\nbody\n";
        let fm = parse_frontmatter(content, &p("page.md")).unwrap().unwrap();
        assert_eq!(fm.title, "Simple");
        assert!(fm.aliases.is_empty());
        assert!(fm.tags.is_empty());
        assert_eq!(fm.summary, "A simple page.");
    }

    #[test]
    fn test_no_frontmatter_returns_none() {
        let content = "# Just a heading\n\nSome text.";
        let result = parse_frontmatter(content, &p("page.md")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_missing_title_error() {
        let content = "---\naliases:\n  - alias\nsummary: A summary.\n---\nbody\n";
        let err = parse_frontmatter(content, &p("page.md")).unwrap_err();
        assert!(matches!(err, FrontmatterError::MissingTitle { .. }));
    }

    #[test]
    fn test_empty_title_error() {
        let content = "---\ntitle: ''\n---\nbody\n";
        let err = parse_frontmatter(content, &p("page.md")).unwrap_err();
        assert!(matches!(err, FrontmatterError::EmptyTitle { .. }));
    }

    #[test]
    fn test_non_string_title_error() {
        let content = "---\ntitle: 42\n---\nbody\n";
        let err = parse_frontmatter(content, &p("page.md")).unwrap_err();
        assert!(matches!(err, FrontmatterError::InvalidTitleType { .. }));
    }

    #[test]
    fn test_invalid_yaml_error() {
        let content = "---\ntitle: [unclosed\n---\nbody\n";
        let err = parse_frontmatter(content, &p("page.md")).unwrap_err();
        assert!(matches!(err, FrontmatterError::YamlParse { .. }));
    }

    #[test]
    fn test_aliases_not_array_error() {
        let content = "---\ntitle: Page\naliases: not-an-array\n---\nbody\n";
        let err = parse_frontmatter(content, &p("page.md")).unwrap_err();
        assert!(matches!(err, FrontmatterError::InvalidAliases { .. }));
    }

    #[test]
    fn test_aliases_empty_string_error() {
        let content = "---\ntitle: Page\naliases:\n  - ''\n---\nbody\n";
        let err = parse_frontmatter(content, &p("page.md")).unwrap_err();
        assert!(matches!(err, FrontmatterError::InvalidAliases { .. }));
    }

    #[test]
    fn test_tags_not_array_error() {
        let content = "---\ntitle: Page\ntags: not-an-array\n---\nbody\n";
        let err = parse_frontmatter(content, &p("page.md")).unwrap_err();
        assert!(matches!(err, FrontmatterError::InvalidTags { .. }));
    }

    #[test]
    fn test_tags_empty_string_error() {
        let content = "---\ntitle: Page\ntags:\n  - ''\n---\nbody\n";
        let err = parse_frontmatter(content, &p("page.md")).unwrap_err();
        assert!(matches!(err, FrontmatterError::InvalidTags { .. }));
    }

    #[test]
    fn test_missing_summary_error() {
        let content = "---\ntitle: Page\n---\nbody\n";
        let err = parse_frontmatter(content, &p("page.md")).unwrap_err();
        assert!(matches!(err, FrontmatterError::MissingSummary { .. }));
    }

    #[test]
    fn test_empty_summary_error() {
        let content = "---\ntitle: Page\nsummary: ''\n---\nbody\n";
        let err = parse_frontmatter(content, &p("page.md")).unwrap_err();
        assert!(matches!(err, FrontmatterError::EmptySummary { .. }));
    }

    #[test]
    fn test_non_string_summary_error() {
        let content = "---\ntitle: Page\nsummary: 42\n---\nbody\n";
        let err = parse_frontmatter(content, &p("page.md")).unwrap_err();
        assert!(matches!(err, FrontmatterError::InvalidSummaryType { .. }));
    }

    #[test]
    fn test_reserved_title_error() {
        for reserved in RESERVED_TITLES {
            let content = format!("---\ntitle: {reserved}\nsummary: A summary.\n---\n");
            let err = parse_frontmatter(&content, &p("page.md")).unwrap_err();
            assert!(
                matches!(err, FrontmatterError::ReservedTitle { .. }),
                "expected ReservedTitle for '{reserved}', got: {err}"
            );
        }
    }

    #[test]
    fn test_reserved_title_case_insensitive() {
        let content = "---\ntitle: CHECK\nsummary: A summary.\n---\n";
        let err = parse_frontmatter(content, &p("page.md")).unwrap_err();
        assert!(matches!(err, FrontmatterError::ReservedTitle { .. }));
    }

    // ── Index and collision tests ─────────────────────────────────────────────

    #[test]
    fn test_index_basic() {
        let pages = vec![(
            p("a.md"),
            Frontmatter {
                title: "Alpha".into(),
                aliases: vec!["β".into()],
                tags: vec![],
                keywords: vec![],
                summary: "Summary.".into(),
            namespace: None,
            },
        )];
        let (idx, collisions) = build_index(&pages);
        assert!(collisions.is_empty());
        assert_eq!(idx.get("alpha"), Some(&p("a.md")));
        assert_eq!(idx.get("β"), Some(&p("a.md")));
    }

    #[test]
    fn test_title_collision_case_insensitive() {
        let pages = vec![
            (
                p("a.md"),
                Frontmatter {
                    title: "Alpha".into(),
                    aliases: vec![],
                    tags: vec![],
                    keywords: vec![],
                    summary: "Summary.".into(),
                namespace: None,
                },
            ),
            (
                p("b.md"),
                Frontmatter {
                    title: "alpha".into(),
                    aliases: vec![],
                    tags: vec![],
                    keywords: vec![],
                    summary: "Summary.".into(),
                namespace: None,
                },
            ),
        ];
        let (_, collisions) = build_index(&pages);
        assert_eq!(collisions.len(), 1);
        assert_eq!(collisions[0].key, "alpha");
        // Collision reported on the second definer
        assert_eq!(collisions[0].offending_path, p("b.md"));
        assert_eq!(collisions[0].existing_path, p("a.md"));
    }

    #[test]
    fn test_alias_collides_with_title() {
        let pages = vec![
            (
                p("a.md"),
                Frontmatter {
                    title: "Shared".into(),
                    aliases: vec![],
                    tags: vec![],
                    keywords: vec![],
                    summary: "Summary.".into(),
                namespace: None,
                },
            ),
            (
                p("b.md"),
                Frontmatter {
                    title: "Other".into(),
                    aliases: vec!["Shared".into()],
                    tags: vec![],
                    keywords: vec![],
                    summary: "Summary.".into(),
                namespace: None,
                },
            ),
        ];
        let (_, collisions) = build_index(&pages);
        assert_eq!(collisions.len(), 1);
        // Error reported on alias definer (b.md), not title holder (a.md)
        assert_eq!(collisions[0].offending_path, p("b.md"));
        assert_eq!(collisions[0].existing_path, p("a.md"));
    }

    #[test]
    fn test_alias_collides_with_alias() {
        let pages = vec![
            (
                p("a.md"),
                Frontmatter {
                    title: "A".into(),
                    aliases: vec!["shared-alias".into()],
                    tags: vec![],
                    keywords: vec![],
                    summary: "Summary.".into(),
                namespace: None,
                },
            ),
            (
                p("b.md"),
                Frontmatter {
                    title: "B".into(),
                    aliases: vec!["shared-alias".into()],
                    tags: vec![],
                    keywords: vec![],
                    summary: "Summary.".into(),
                namespace: None,
                },
            ),
        ];
        let (_, collisions) = build_index(&pages);
        assert_eq!(collisions.len(), 1);
        assert_eq!(collisions[0].offending_path, p("b.md"));
    }

    #[test]
    fn test_crlf_frontmatter_parsed_correctly() {
        // Frontmatter with CRLF line endings must parse correctly
        let content = "---\r\ntitle: My Page\r\naliases:\r\n  - alias\r\nsummary: A summary.\r\n---\r\nbody\r\n";
        let fm = parse_frontmatter(content, &p("page.md")).unwrap().unwrap();
        assert_eq!(fm.title, "My Page");
        assert_eq!(fm.aliases, vec!["alias"]);
        assert_eq!(fm.summary, "A summary.");
    }

    #[test]
    fn test_valid_keywords() {
        let content = "---\ntitle: Page\nsummary: Summary.\nkeywords:\n  - cards-create\n  - CardsCreatePanel\n---\nbody\n";
        let fm = parse_frontmatter(content, &p("page.md")).unwrap().unwrap();
        assert_eq!(fm.keywords, vec!["cards-create", "CardsCreatePanel"]);
    }

    #[test]
    fn test_missing_keywords_defaults_to_empty() {
        let content = "---\ntitle: Page\nsummary: Summary.\n---\nbody\n";
        let fm = parse_frontmatter(content, &p("page.md")).unwrap().unwrap();
        assert!(fm.keywords.is_empty());
    }

    #[test]
    fn test_keywords_empty_string_error() {
        let content = "---\ntitle: Page\nsummary: Summary.\nkeywords:\n  - ''\n---\nbody\n";
        let err = parse_frontmatter(content, &p("page.md")).unwrap_err();
        assert!(matches!(err, FrontmatterError::InvalidKeywords { .. }));
    }

    #[test]
    fn test_no_collision_unique_keys() {
        let pages = vec![
            (
                p("a.md"),
                Frontmatter {
                    title: "Alpha".into(),
                    aliases: vec!["a".into()],
                    tags: vec![],
                    keywords: vec![],
                    summary: "Summary.".into(),
                namespace: None,
                },
            ),
            (
                p("b.md"),
                Frontmatter {
                    title: "Beta".into(),
                    aliases: vec!["b".into()],
                    tags: vec![],
                    keywords: vec![],
                    summary: "Summary.".into(),
                namespace: None,
                },
            ),
        ];
        let (idx, collisions) = build_index(&pages);
        assert!(collisions.is_empty());
        assert_eq!(idx.len(), 4);
    }
}
