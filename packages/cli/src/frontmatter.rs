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

    #[error("`links-reviewed` must be a scalar, not a collection.")]
    InvalidLinksReviewed { path: PathBuf },

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
pub const RESERVED_TITLES: &[&str] = &["check", "list", "summary", "mesh"];

// ── Raw deserialization helper ────────────────────────────────────────────────

/// Intermediate struct that accepts loose YAML types so we can validate manually.
#[derive(Debug, Deserialize)]
struct RawFrontmatter {
    title: Option<serde_yaml::Value>,
    aliases: Option<serde_yaml::Value>,
    tags: Option<serde_yaml::Value>,
    keywords: Option<serde_yaml::Value>,
    summary: Option<serde_yaml::Value>,
    #[serde(rename = "links-reviewed")]
    links_reviewed: Option<serde_yaml::Value>,
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
    /// The `links-reviewed:` certification value (card main-3), coerced to
    /// its string form — `None` when the field is absent.
    pub links_reviewed: Option<String>,
}

// ── Parsing ───────────────────────────────────────────────────────────────────

/// Returns `true` if `content` is a wiki-page candidate: it has a `---`
/// frontmatter fence whose YAML contains `title` or `summary` keys, or whose
/// YAML is unparseable (possibly a broken wiki page). Files with no fence, or
/// with a fence containing only non-wiki keys (`name`, `description`, `paths`,
/// etc.), are not wiki candidates.
pub fn has_wiki_frontmatter(content: &str) -> bool {
    let content = content.trim_start_matches('\n');
    if !content.starts_with("---") {
        return false;
    }
    // The opening "---" must be on its own line.
    let after = &content["---".len()..];
    if !after.is_empty() && !after.starts_with('\n') && !after.starts_with("\r\n") {
        return false;
    }
    // Extract the YAML block. If there's no closing fence, the YAML is
    // unparseable — treat it as a wiki candidate so the diagnostic loop
    // reports the missing fence.
    let Some(yaml) = extract_yaml_block(content) else {
        return true;
    };
    // Parseable YAML: include only if it has wiki-specific keys.  Skill files
    // (name/description) and config files (paths) are not wiki pages.
    // `links-reviewed` is a wiki-only certification field (card main-3), so
    // its presence is a third wiki marker.
    match serde_yaml::from_str::<serde_yaml::Value>(yaml) {
        Ok(value) => {
            value.get("title").is_some()
                || value.get("summary").is_some()
                || value.get("links-reviewed").is_some()
        }
        Err(_) => true, // Broken YAML — might be a malformed wiki page
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

    // Validate links-reviewed (card main-3): a scalar coerced to its string
    // form via the same coercion the drift engine's epoch comparison uses,
    // so page validation and certification can never disagree about a value.
    let links_reviewed = match raw.links_reviewed {
        None => None,
        Some(value) => Some(scalar_to_string(&value).ok_or_else(|| {
            FrontmatterError::InvalidLinksReviewed {
                path: path.to_path_buf(),
            }
        })?),
    };

    Ok(Some(Frontmatter {
        title,
        aliases,
        tags,
        keywords,
        summary,
        links_reviewed,
    }))
}

/// The string form of a YAML scalar: quoted strings unquote, numbers and
/// booleans render as written. `null`, sequences, and maps have no scalar
/// string form (fail-closed: the drift engine's change detection must never
/// silently collapse distinct values into one string).
pub(crate) fn scalar_to_string(value: &serde_yaml::Value) -> Option<String> {
    match value {
        serde_yaml::Value::String(s) => Some(s.clone()),
        serde_yaml::Value::Number(n) => Some(n.to_string()),
        serde_yaml::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// Byte bounds of the leading `---` YAML block in `content`: `(yaml_start,
/// yaml_end)` bracket the YAML text between the fences, and `close_fence` is
/// the byte offset where the closing `---` line begins. `None` when there is
/// no opening fence, the opening fence is not on its own line, or no closing
/// fence exists. Leading blank lines are skipped exactly like
/// [`extract_yaml_block`].
///
/// `pub(crate)` for the drift engine, which appends a field just before the
/// closing fence without disturbing the rest of the content byte-for-byte.
pub(crate) fn yaml_block_bounds(content: &str) -> Option<(usize, usize, usize)> {
    let trimmed = content.trim_start_matches('\n');
    let skipped = content.len() - trimmed.len();
    if !trimmed.starts_with("---") {
        return None;
    }
    // The opening fence must be on its own line.
    let after_fence = &trimmed["---".len()..];
    let (yaml_start, after_open) = if let Some(s) = after_fence.strip_prefix('\n') {
        (skipped + "---".len() + 1, s)
    } else {
        let s = after_fence.strip_prefix("\r\n")?;
        (skipped + "---".len() + 2, s)
    };
    let close = find_close_fence(after_open)?;
    Some((yaml_start, yaml_start + close, yaml_start + close))
}

/// Extract the raw YAML string from the leading `---` block.
fn extract_yaml_block(content: &str) -> Option<&str> {
    let (start, end, _) = yaml_block_bounds(content)?;
    Some(&content[start..end])
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

    /// Reproduction: RESERVED_TITLES must match the actual CLI subcommands.
    ///
    /// `mesh` is a real subcommand but is missing from the set;
    /// `pin`, `stale`, `links`, `print` are dead entries that should be removed.
    #[test]
    fn test_reserved_titles_matches_cli_surface() {
        let reserved: std::collections::BTreeSet<&str> =
            RESERVED_TITLES.iter().copied().collect();

        // mesh is a real subcommand — must be reserved
        assert!(
            reserved.contains("mesh"),
            "mesh is a CLI subcommand but is missing from RESERVED_TITLES"
        );

        // Dead entries that no longer correspond to any subcommand
        for dead in &["pin", "stale", "links", "print"] {
            assert!(
                !reserved.contains(dead),
                "'{dead}' is not a CLI subcommand but is still in RESERVED_TITLES"
            );
        }

        // Exact set must be the current subcommand names
        let expected: std::collections::BTreeSet<&str> =
            ["check", "list", "summary", "mesh"].into_iter().collect();
        assert_eq!(
            reserved, expected,
            "RESERVED_TITLES drifted from the CLI command surface"
        );
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
                links_reviewed: None,
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
                    links_reviewed: None,
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
                    links_reviewed: None,
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
                    links_reviewed: None,
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
                    links_reviewed: None,
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
                    links_reviewed: None,
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
                    links_reviewed: None,
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
    fn test_links_reviewed_scalar_forms_coerce_to_string() {
        for (yaml, expected) in [
            ("links-reviewed: 1", Some("1")),
            ("links-reviewed: 2", Some("2")),
            ("links-reviewed: \"quoted value\"", Some("quoted value")),
        ] {
            let content = format!("---\ntitle: Page\nsummary: S.\n{yaml}\n---\n");
            let fm = parse_frontmatter(&content, &p("page.md")).unwrap().unwrap();
            assert_eq!(fm.links_reviewed.as_deref(), expected);
        }
    }

    #[test]
    fn test_links_reviewed_absent_is_none() {
        let content = "---\ntitle: Page\nsummary: S.\n---\n";
        let fm = parse_frontmatter(content, &p("page.md")).unwrap().unwrap();
        assert_eq!(fm.links_reviewed, None);
    }

    #[test]
    fn test_links_reviewed_non_scalar_error() {
        // A sequence is not a scalar — fail closed with a page-level
        // validation error rather than inventing a value.
        let content = "---\ntitle: Page\nsummary: S.\nlinks-reviewed:\n  - 1\n  - 2\n---\n";
        let err = parse_frontmatter(content, &p("page.md")).unwrap_err();
        assert!(matches!(err, FrontmatterError::InvalidLinksReviewed { .. }));
    }

    #[test]
    fn test_links_reviewed_bare_null_reads_as_absent() {
        // serde_yaml deserializes a bare `links-reviewed:` (YAML null) into
        // `None` for an Option field — indistinguishable from absent. That is
        // still fail-closed: the drift engine reads the same null as no
        // epoch, so read-only checks hard-error until a human fixes the page.
        let content = "---\ntitle: Page\nsummary: S.\nlinks-reviewed:\n---\n";
        let fm = parse_frontmatter(content, &p("page.md")).unwrap().unwrap();
        assert_eq!(fm.links_reviewed, None);
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
                    links_reviewed: None,
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
                    links_reviewed: None,
                },
            ),
        ];
        let (idx, collisions) = build_index(&pages);
        assert!(collisions.is_empty());
        assert_eq!(idx.len(), 4);
    }
}
