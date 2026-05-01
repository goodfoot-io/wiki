//! `wiki scaffold` end-to-end pipeline.
//!
//! Discover wiki files, parse their fragment links, and emit a markdown
//! document (or JSON) of `git mesh add` / `git mesh why` commands — one mesh
//! per link.
//!
//! Phase B: name generation is fully wired; whys are template-only because
//! [`why::extract_prose_why`] is a Phase B stub returning `None`. Phase C will
//! add the prose-why heuristics.

use std::fs;
use std::path::{Path, PathBuf};

use miette::{IntoDiagnostic, Result};
use regex::Regex;
use serde::Serialize;

use crate::commands::{discover_files, resolve_link_path};
use crate::parser::{LinkKind, parse_fragment_links};

// ── Parse-error types ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub(crate) enum ParseErrorKind {
    /// File does not start with `---\n`.
    NoFrontmatter,
    /// Frontmatter present, no `title:` key.
    MissingTitle,
    /// `title:` present, value empty/whitespace.
    EmptyTitle,
    /// IO error or invalid UTF-8 — message captured.
    Unreadable(String),
    /// Starts with `---` but regex rejected it (BOM, CRLF, no closing fence, etc.).
    Malformed,
}

#[derive(Debug, Clone)]
pub(crate) struct ParseError {
    pub(crate) path: String,
    pub(crate) kind: ParseErrorKind,
}

// ── JSON output types ─────────────────────────────────────────────────────────

/// Top-level JSON output for `wiki scaffold --format json`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ScaffoldOutput {
    schema_version: u32,
    parse_errors: Vec<ParseErrorJson>,
    pages: Vec<PageJson>,
}

/// JSON representation of a parse error.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ParseErrorJson {
    path: String,
    category: ParseErrorCategory,
    message: String,
}

/// Machine-stable parse-error category tags.
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum ParseErrorCategory {
    NoFrontmatter,
    MissingTitle,
    EmptyTitle,
    Unreadable,
    MalformedFrontmatter,
}

/// JSON representation of a per-page section.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PageJson {
    path: String,
    title: String,
    meshes: Vec<MeshJson>,
}

/// JSON representation of one mesh entry.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MeshJson {
    slug: String,
    heading_chain: Vec<String>,
    section_opening: Vec<String>,
    anchors: Vec<AnchorJson>,
}

/// JSON representation of a structured anchor.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AnchorJson {
    path: String,
    start_line: u32,
    end_line: u32,
}

impl ParseErrorCategory {
    fn from_kind(kind: &ParseErrorKind) -> Self {
        match kind {
            ParseErrorKind::NoFrontmatter => ParseErrorCategory::NoFrontmatter,
            ParseErrorKind::MissingTitle => ParseErrorCategory::MissingTitle,
            ParseErrorKind::EmptyTitle => ParseErrorCategory::EmptyTitle,
            ParseErrorKind::Unreadable(_) => ParseErrorCategory::Unreadable,
            ParseErrorKind::Malformed => ParseErrorCategory::MalformedFrontmatter,
        }
    }
}

impl ParseErrorKind {
    pub(crate) fn reason(&self) -> String {
        match self {
            ParseErrorKind::NoFrontmatter => {
                "no frontmatter block — file does not start with `---`".to_string()
            }
            ParseErrorKind::MissingTitle => {
                "frontmatter present but `title:` is missing".to_string()
            }
            ParseErrorKind::EmptyTitle => "frontmatter present but `title:` is empty".to_string(),
            ParseErrorKind::Unreadable(msg) => format!("file could not be read: {msg}"),
            ParseErrorKind::Malformed => {
                "malformed frontmatter — could not parse `title`".to_string()
            }
        }
    }
}

use super::augment::{AugmentedLink, augment};
use super::draft::{self, MeshDraft};
use super::group;
use super::render;

/// Run the `wiki scaffold` subcommand.
pub fn run(globs: &[String], json: bool, wiki_root: &Path, repo_root: &Path) -> Result<i32> {
    let discovered = discover_files(globs, wiki_root, repo_root)?;
    // Filter test fixtures: `wiki check` legitimately scans them, but a
    // scaffold run that materializes mesh commands for a test wiki would
    // pollute the repo's mesh state on the first commit. Scoped to scaffold.
    let files: Vec<PathBuf> = discovered
        .into_iter()
        .filter(|p| {
            let s = p.to_string_lossy();
            !s.contains("/tests/fixtures/") && !s.contains("\\tests\\fixtures\\")
        })
        .collect();

    let mut all_inputs: Vec<LinkInput> = Vec::new();
    for file in &files {
        let content = match fs::read_to_string(file) {
            Ok(s) => s,
            Err(e) => {
                if json {
                    // JSON mode must preserve baseline hard-error semantics.
                    return Err(miette::miette!("{}: {}", file.display(), e));
                }
                // Markdown mode: unreadable files are reported via parse_errors.
                continue;
            }
        };
        let raw_links = parse_fragment_links(&content);
        let augmented = augment(&raw_links, &content);
        // Filter to internal links with a parsed line range — mirrors the JS
        // which skips URL-scheme links and links lacking `#`.
        for aug in augmented {
            if aug.link.kind != LinkKind::Internal {
                continue;
            }
            if aug.link.start_line.is_none() {
                continue;
            }
            all_inputs.push(LinkInput {
                wiki_file: file.clone(),
                augmented: aug,
            });
        }
    }

    // Build per-source frontmatter map (title) keyed by absolute path,
    // and accumulate parse errors for source files.
    let mut wiki_meta_cache: std::collections::HashMap<PathBuf, FileMeta> =
        std::collections::HashMap::new();
    let mut parse_errors: Vec<ParseError> = Vec::new();
    for f in &files {
        let (meta, err_kind) = classify_frontmatter(f);
        if let Some(kind) = err_kind {
            let rel = path_relative_to(f, repo_root);
            parse_errors.push(ParseError { path: rel, kind });
        }
        wiki_meta_cache.insert(f.clone(), meta);
    }
    parse_errors.sort_by(|a, b| a.path.cmp(&b.path));

    // ── Unified build/group pipeline (both modes) ─────────────────────────
    let consolidated = build_meshes(&all_inputs, repo_root);

    // Build the page-title lookup keyed by the same repo-root-relative
    // `page_path` strings the drafts use.
    let mut page_titles: std::collections::HashMap<String, Option<String>> =
        std::collections::HashMap::new();
    for f in &files {
        let rel = path_relative_to(f, repo_root);
        let title = wiki_meta_cache.get(f).and_then(|m| m.title.clone());
        page_titles.insert(rel, title);
    }

    if json {
        if all_inputs.is_empty() {
            // Empty corpus — still emit structured output.
            let parse_errors_json: Vec<ParseErrorJson> = parse_errors
                .iter()
                .map(|e| ParseErrorJson {
                    path: e.path.clone(),
                    category: ParseErrorCategory::from_kind(&e.kind),
                    message: e.kind.reason(),
                })
                .collect();
            let output = ScaffoldOutput {
                schema_version: 1,
                parse_errors: parse_errors_json,
                pages: Vec::new(),
            };
            let s = serde_json::to_string_pretty(&output).into_diagnostic()?;
            println!("{s}");
            return Ok(0);
        }
        let parse_errors_json: Vec<ParseErrorJson> = parse_errors
            .iter()
            .map(|e| ParseErrorJson {
                path: e.path.clone(),
                category: ParseErrorCategory::from_kind(&e.kind),
                message: e.kind.reason(),
            })
            .collect();
        let pages = build_pages_json(&consolidated, &page_titles);
        let output = ScaffoldOutput {
            schema_version: 1,
            parse_errors: parse_errors_json,
            pages,
        };
        let s = serde_json::to_string_pretty(&output).into_diagnostic()?;
        println!("{s}");
        return Ok(0);
    }

    // ── Markdown mode ─────────────────────────────────────────────────────
    if all_inputs.is_empty() {
        print!("{}", render::render_empty_markdown(&parse_errors));
        return Ok(0);
    }

    let rendered = render::render_markdown(&consolidated, &page_titles, &parse_errors);
    print!("{rendered}");
    Ok(0)
}

/// Build the JSON page list from consolidated drafts.
fn build_pages_json(
    drafts: &[MeshDraft],
    page_titles: &std::collections::HashMap<String, Option<String>>,
) -> Vec<PageJson> {
    // Group by page in first-occurrence order.
    let mut page_order: Vec<String> = Vec::new();
    let mut by_page: std::collections::HashMap<String, Vec<&MeshDraft>> =
        std::collections::HashMap::new();
    for d in drafts {
        if !by_page.contains_key(&d.page_path) {
            page_order.push(d.page_path.clone());
        }
        by_page.entry(d.page_path.clone()).or_default().push(d);
    }

    page_order
        .into_iter()
        .map(|page_path| {
            let title = page_titles
                .get(&page_path)
                .and_then(|t| t.clone())
                .unwrap_or_default();
            let page_drafts = by_page.get(&page_path).expect("tracked");
            let meshes = page_drafts
                .iter()
                .map(|d| {
                    // Trim heading chain: drop leading entry if it matches page title
                    // (after normalization via extract_heading_text-style stripping).
                    let trimmed_chain = trim_heading_chain(&d.heading_chain, &title);
                    MeshJson {
                        slug: d.slug.clone(),
                        heading_chain: trimmed_chain,
                        section_opening: d.section_opening_lines.clone(),
                        anchors: d
                            .structured_anchors
                            .iter()
                            .map(|a| AnchorJson {
                                path: a.path.clone(),
                                start_line: a.start_line,
                                end_line: a.end_line,
                            })
                            .collect(),
                    }
                })
                .collect();
            PageJson {
                path: page_path,
                title,
                meshes,
            }
        })
        .collect()
}

/// Trim the leading entry of `heading_chain` when it matches the page's
/// frontmatter `title` after normalization (strip inline markup, collapse
/// whitespace, case-insensitive compare). Returns the trimmed chain.
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

/// Normalize heading or title text for comparison: strip inline markup chars
/// (`*`, `_`, `` ` ``, `[`, `]`), collapse whitespace, trim.
/// Mirrors the behavior of `extract_heading_text` in the legacy path.
fn normalize_heading_text(s: &str) -> String {
    let stripped: String = s.chars().filter(|c| !"`*_[]".contains(*c)).collect();
    // Collapse whitespace
    stripped.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Three-stage build/group/annotate pipeline that produces the final list of
/// meshes (in per-page declaration order) ready for shell rendering.
fn build_meshes(inputs: &[LinkInput], repo_root: &Path) -> Vec<MeshDraft> {
    // Group inputs by source page (preserving discovery order).
    let mut page_order: Vec<PathBuf> = Vec::new();
    let mut by_page: std::collections::HashMap<PathBuf, Vec<&LinkInput>> =
        std::collections::HashMap::new();
    for input in inputs {
        if !by_page.contains_key(&input.wiki_file) {
            page_order.push(input.wiki_file.clone());
        }
        by_page
            .entry(input.wiki_file.clone())
            .or_default()
            .push(input);
    }

    // Stage 1: build drafts per page.
    let mut all_drafts: Vec<MeshDraft> = Vec::new();
    let mut page_spans: Vec<(usize, usize)> = Vec::with_capacity(page_order.len());
    for page in &page_order {
        let entries = by_page.get(page).expect("page tracked in order");
        let page_rel = path_relative_to(page, repo_root);
        let augs: Vec<AugmentedLink> = entries.iter().map(|i| i.augmented.clone()).collect();
        let target_anchors: Vec<Vec<String>> = entries
            .iter()
            .map(|i| {
                let link = &i.augmented.link;
                let resolved = resolve_link_path(&link.path, &i.wiki_file, repo_root);
                let anchor_rel = path_relative_to(&resolved, repo_root);
                let anchor_rel =
                    locate_existing_suffix(&anchor_rel, repo_root).unwrap_or(anchor_rel);
                let start = link.start_line.unwrap_or(0);
                let end = link.end_line.unwrap_or(start);
                vec![format!("{anchor_rel}#L{start}-L{end}")]
            })
            .collect();
        let structured_anchors: Vec<Vec<draft::StructuredAnchor>> = entries
            .iter()
            .map(|i| {
                let link = &i.augmented.link;
                let resolved = resolve_link_path(&link.path, &i.wiki_file, repo_root);
                let anchor_rel = path_relative_to(&resolved, repo_root);
                let anchor_rel =
                    locate_existing_suffix(&anchor_rel, repo_root).unwrap_or(anchor_rel);
                let start = link.start_line.unwrap_or(0);
                let end = link.end_line.unwrap_or(start);
                vec![draft::StructuredAnchor {
                    path: anchor_rel,
                    start_line: start,
                    end_line: end,
                }]
            })
            .collect();
        let drafts = draft::build(&page_rel, &augs, &target_anchors, &structured_anchors, repo_root);
        let start = all_drafts.len();
        all_drafts.extend(drafts);
        page_spans.push((start, all_drafts.len()));
    }

    // Stage 2a: per-page consolidation FIRST. Identical-anchor-set siblings
    // collapse into one survivor; only then does the global dedup pass see
    // contiguous slug counts. Doing this in the reverse order leaks suffix
    // gaps (`foo`, `foo-3`, no `foo-2`) into the footer's `git mesh commit`
    // lines whenever consolidation prunes a duplicate the dedup already
    // suffixed.
    let mut consolidated: Vec<MeshDraft> = Vec::new();
    for (start, end) in page_spans {
        let page_drafts: Vec<MeshDraft> = all_drafts[start..end].to_vec();
        consolidated.extend(group::consolidate_within_page(page_drafts));
    }

    // Stage 2b: global slug dedup over the merge survivors.
    dedup_slugs(&mut consolidated);

    consolidated
}

/// First occurrence keeps the original slug; subsequent duplicates get
/// `-2`, `-3`, … suffixes.
fn dedup_slugs(drafts: &mut [MeshDraft]) {
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for d in drafts.iter_mut() {
        let count = seen.entry(d.slug.clone()).or_insert(0);
        *count += 1;
        if *count > 1 {
            d.slug = format!("{}-{}", d.slug, *count);
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

struct LinkInput {
    wiki_file: PathBuf,
    augmented: AugmentedLink,
}

#[derive(Debug, Clone, Default)]
struct FileMeta {
    title: Option<String>,
}

/// Classify the frontmatter of a file, returning both the `FileMeta` and an
/// optional `ParseErrorKind` if the file's `title` could not be extracted.
fn classify_frontmatter(path: &Path) -> (FileMeta, Option<ParseErrorKind>) {
    // Step 1: read raw bytes; if IO fails or bytes are not valid UTF-8 → Unreadable.
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            return (
                FileMeta::default(),
                Some(ParseErrorKind::Unreadable(e.to_string())),
            );
        }
    };
    let text = match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => {
            return (
                FileMeta::default(),
                Some(ParseErrorKind::Unreadable(e.to_string())),
            );
        }
    };

    // Step 2: must start with `---\n` or `---\r\n`.
    if !text.starts_with("---\n") && !text.starts_with("---\r\n") {
        return (FileMeta::default(), Some(ParseErrorKind::NoFrontmatter));
    }

    // Step 3: locate closing `---` fence.
    let after_open = text
        .strip_prefix("---\r\n")
        .or_else(|| text.strip_prefix("---\n"))
        .unwrap_or(&text[4..]);
    let has_closing_fence = after_open
        .lines()
        .any(|l| l.trim_end_matches('\r') == "---");
    if !has_closing_fence {
        return (FileMeta::default(), Some(ParseErrorKind::Malformed));
    }

    // Step 4: look for `title:` line inside the fenced block.
    // Collect lines between the two `---` fences.
    let lines: Vec<&str> = after_open.lines().collect();
    let closing_idx = lines.iter().position(|l| l.trim_end_matches('\r') == "---");
    let fm_lines = match closing_idx {
        Some(i) => &lines[..i],
        None => &lines[..],
    };

    let title_line = fm_lines
        .iter()
        .find(|l| l.starts_with("title:") || l.starts_with("title :"));

    if title_line.is_none() {
        return (FileMeta::default(), Some(ParseErrorKind::MissingTitle));
    }

    // Check if the value is empty/whitespace.
    let raw_value = title_line
        .unwrap()
        .split_once(':')
        .map(|(_, v)| v)
        .unwrap_or("")
        .trim();
    let stripped_value = raw_value
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| {
            raw_value
                .strip_prefix('\'')
                .and_then(|s| s.strip_suffix('\''))
        })
        .unwrap_or(raw_value)
        .trim();

    if stripped_value.is_empty() {
        return (FileMeta::default(), Some(ParseErrorKind::EmptyTitle));
    }

    // Step 5: run parse_frontmatter_field — if it returns None despite a
    // non-empty title line, the frontmatter is malformed (BOM, CRLF, etc.).
    let title = parse_frontmatter_field(&text, "title");
    if title.is_none() {
        return (FileMeta::default(), Some(ParseErrorKind::Malformed));
    }

    let meta = FileMeta { title };
    (meta, None)
}

fn parse_frontmatter_field(content: &str, field: &str) -> Option<String> {
    // Only parse if the file starts with `---\n`. JS uses /^---\s*\n(?:.*\n)*?title:\s*(.+?)\s*\n/.
    // Anchor to file start (\A) so a thematic-break `---` later in the body does not
    // match — that was the JS prototype's intent.
    let pat = format!(r"\A---\s*\n(?:.*\n)*?{field}:\s*(.+?)\s*\n");
    let re = Regex::new(&pat).ok()?;
    let cap = re.captures(content)?;
    let raw = cap.get(1)?.as_str().trim();
    let stripped = raw
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| raw.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
        .unwrap_or(raw);
    Some(stripped.trim().to_string())
}

fn path_relative_to(path: &Path, repo_root: &Path) -> String {
    let rel = path.strip_prefix(repo_root).unwrap_or(path);
    rel.to_string_lossy().replace('\\', "/")
}

fn locate_existing_suffix(rel_path: &str, repo_root: &Path) -> Option<String> {
    if repo_root.join(rel_path).exists() {
        return Some(rel_path.to_string());
    }
    let parts: Vec<&str> = rel_path.split('/').collect();
    for start in 1..parts.len() {
        let candidate = parts[start..].join("/");
        if candidate.is_empty() {
            continue;
        }
        if repo_root.join(&candidate).exists() {
            return Some(candidate);
        }
    }
    None
}

/// Extract the text content of a markdown heading (without the # symbols, whitespace, and markdown formatting).
#[allow(dead_code)]
fn extract_heading_text(line: &str) -> String {
    let trimmed = line.trim_start();
    let text = trimmed
        .trim_start_matches('#')
        .trim_start()
        .trim_end()
        .to_string();
    // Remove markdown formatting: backticks, bold, italic, etc.
    let re = Regex::new(r"[`*_~\[\]]").expect("valid regex");
    re.replace_all(&text, "").trim().to_string()
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_frontmatter_extracts_title_and_summary() {
        let c = "---\ntitle: Hello World\nsummary: A page summary.\n---\nbody";
        assert_eq!(
            parse_frontmatter_field(c, "title"),
            Some("Hello World".into())
        );
        assert_eq!(
            parse_frontmatter_field(c, "summary"),
            Some("A page summary.".into())
        );
    }

    #[test]
    fn parse_frontmatter_handles_quoted_values() {
        let c = "---\ntitle: \"Quoted Title\"\n---\n";
        assert_eq!(
            parse_frontmatter_field(c, "title"),
            Some("Quoted Title".into())
        );
    }

    #[test]
    fn parse_frontmatter_returns_none_when_absent() {
        assert!(parse_frontmatter_field("no frontmatter here", "title").is_none());
    }

    #[test]
    fn parse_frontmatter_ignores_thematic_break_in_body() {
        // Body contains a `---` separator followed by a `title:` line — must NOT match.
        let c = "# Heading\n\nbody text\n\n---\ntitle: Spurious\n\nmore body\n";
        assert_eq!(parse_frontmatter_field(c, "title"), None);
    }

    // ── heading-chain trim ────────────────────────────────────────────────────

    #[test]
    fn trim_chain_drops_leading_when_equals_page_title() {
        let chain = vec!["Billing".to_string(), "Charge handler".to_string()];
        let trimmed = trim_heading_chain(&chain, "Billing");
        assert_eq!(trimmed, vec!["Charge handler"]);
    }

    #[test]
    fn trim_chain_keeps_chain_when_top_differs() {
        let chain = vec!["Charge handler".to_string()];
        let trimmed = trim_heading_chain(&chain, "Billing");
        assert_eq!(trimmed, vec!["Charge handler"]);
    }

    #[test]
    fn trim_chain_empties_to_nothing_when_single_equals_title() {
        let chain = vec!["Incremental indexing".to_string()];
        let trimmed = trim_heading_chain(&chain, "Incremental indexing");
        assert!(trimmed.is_empty());
    }

    #[test]
    fn extract_heading_text_strips_formatting() {
        assert_eq!(extract_heading_text("# My Heading"), "My Heading");
        assert_eq!(extract_heading_text("## `Code Title`"), "Code Title");
        assert_eq!(
            extract_heading_text("### **Bold** and *italic*"),
            "Bold and italic"
        );
        assert_eq!(
            extract_heading_text("### `git-mesh ls <anchor> --porcelain`"),
            "git-mesh ls <anchor> --porcelain"
        );
    }

    // ── classify_frontmatter unit tests ──────────────────────────────────────

    fn classify_str(text: &str) -> Option<ParseErrorKind> {
        // Write to a tempfile and run the classifier.
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), text.as_bytes()).unwrap();
        let (_, kind) = classify_frontmatter(tmp.path());
        kind
    }

    #[test]
    fn classify_no_frontmatter() {
        let kind = classify_str("# Just a body.\n");
        assert!(
            matches!(kind, Some(ParseErrorKind::NoFrontmatter)),
            "expected NoFrontmatter, got {kind:?}"
        );
    }

    #[test]
    fn classify_missing_title() {
        let kind = classify_str("---\nsummary: x\n---\n\nbody\n");
        assert!(
            matches!(kind, Some(ParseErrorKind::MissingTitle)),
            "expected MissingTitle, got {kind:?}"
        );
    }

    #[test]
    fn classify_empty_title() {
        let kind = classify_str("---\ntitle:\nsummary: x\n---\n\nbody\n");
        assert!(
            matches!(kind, Some(ParseErrorKind::EmptyTitle)),
            "expected EmptyTitle, got {kind:?}"
        );
    }

    #[test]
    fn classify_malformed_bom() {
        // BOM-prefixed frontmatter — parse_frontmatter_field will return None.
        let kind = classify_str("\u{FEFF}---\ntitle: x\nsummary: y\n---\n");
        assert!(
            matches!(
                kind,
                Some(ParseErrorKind::Malformed | ParseErrorKind::NoFrontmatter)
            ),
            "expected Malformed or NoFrontmatter, got {kind:?}"
        );
    }

    #[test]
    fn classify_unreadable_non_utf8() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), [0xFF_u8, 0xFE, 0x00]).unwrap();
        let (_, kind) = classify_frontmatter(tmp.path());
        assert!(
            matches!(kind, Some(ParseErrorKind::Unreadable(_))),
            "expected Unreadable, got {kind:?}"
        );
    }

    #[test]
    fn classify_clean_file() {
        let kind = classify_str("---\ntitle: Hello\nsummary: World\n---\n\nbody\n");
        assert!(kind.is_none(), "expected no error, got {kind:?}");
    }
}
