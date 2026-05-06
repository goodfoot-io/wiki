//! Phase-1 draft model: one `MeshDraft` per *section* (deepest enclosing
//! heading, or paragraph fallback), built from the augmented links of a page.
//!
//! Each draft carries the page section anchor as the leading anchor and the
//! merged, deduplicated set of target anchors that the section's links point
//! at. The draft is the unit grouping operates on. Slug derivation lives here.

use std::path::{Path, PathBuf};

use super::augment::AugmentedLink;

/// Structured anchor triple — path and line range, before stringification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StructuredAnchor {
    /// Repo-root-relative path with forward slashes.
    pub(crate) path: String,
    pub(crate) start_line: u32,
    pub(crate) end_line: u32,
}

/// One mesh proposal before grouping. `consolidated_count` is `1` until
/// `group` merges siblings into it.
#[derive(Debug, Clone)]
pub(crate) struct MeshDraft {
    /// Source wiki page, repo-root-relative with forward slashes.
    pub(crate) page_path: String,
    /// Generated slug, e.g. `wiki/perf/sync-detection`.
    pub(crate) slug: String,
    /// Ordered list of anchor strings (page section anchor first, then targets
    /// like `src/index.rs#L10-L20`). Render emits these as a single
    /// `git mesh add`.
    pub(crate) anchors: Vec<String>,
    /// Structured anchor triples, parallel to `anchors`. The first entry is the
    /// page section anchor; the rest are targets in document order.
    pub(crate) structured_anchors: Vec<StructuredAnchor>,
    /// Full ancestor heading chain above the first link (verbatim text from
    /// `parse_atx_heading`).
    pub(crate) heading_chain: Vec<String>,
    /// Number of identical-anchor-set siblings merged into this draft. Starts
    /// at 1.
    pub(crate) consolidated_count: usize,
}

/// One section's worth of input: the leader (used to derive slug and
/// heading_chain) and the merged target anchors in document order.
pub(crate) struct SectionGroup<'a> {
    pub(crate) leader: &'a AugmentedLink,
    pub(crate) section_start: u32,
    pub(crate) section_end: u32,
    pub(crate) target_anchors: Vec<String>,
    pub(crate) structured_targets: Vec<StructuredAnchor>,
}

/// Build one draft per section group. The page section anchor is prepended to
/// both `anchors` and `structured_anchors`.
pub(crate) fn build(
    page_path: &str,
    groups: &[SectionGroup<'_>],
    _repo_root: &Path,
) -> Vec<MeshDraft> {
    groups
        .iter()
        .map(|g| {
            let slug = derive_slug(page_path, g.leader);
            let page_anchor_str = format!(
                "{page_path}#L{start}-L{end}",
                start = g.section_start,
                end = g.section_end
            );
            let page_anchor = StructuredAnchor {
                path: page_path.to_string(),
                start_line: g.section_start,
                end_line: g.section_end,
            };
            let mut anchors = Vec::with_capacity(1 + g.target_anchors.len());
            anchors.push(page_anchor_str);
            anchors.extend(g.target_anchors.iter().cloned());
            let mut structured_anchors = Vec::with_capacity(1 + g.structured_targets.len());
            structured_anchors.push(page_anchor);
            structured_anchors.extend(g.structured_targets.iter().cloned());
            MeshDraft {
                page_path: page_path.to_string(),
                slug,
                anchors,
                structured_anchors,
                heading_chain: g.leader.heading_chain.clone(),
                consolidated_count: 1,
            }
        })
        .collect()
}

/// Slug = `<category>/<noun>`. Category is the page's parent directory; for
/// `wiki/<sub>/file.md` that's `wiki/<sub>`, for `wiki/file.md` it's `wiki`,
/// for any other path it's the first path segment. Noun is the deepest section
/// heading kebab-cased; falls back to the link label, then the target file stem.
fn derive_slug(page_path: &str, aug: &AugmentedLink) -> String {
    let category = derive_category(page_path);
    let noun = derive_noun(aug);
    if category.is_empty() {
        noun
    } else {
        format!("{category}/{noun}")
    }
}

fn derive_category(page_path: &str) -> String {
    let parts: Vec<&str> = page_path.split('/').collect();
    if parts.len() <= 1 {
        return String::new();
    }
    let dirs = &parts[..parts.len() - 1];
    dirs.join("/")
}

fn derive_noun(aug: &AugmentedLink) -> String {
    let heading = aug
        .section_heading
        .trim_start_matches(|c: char| c == '#' || c.is_whitespace())
        .trim();
    if !heading.is_empty() {
        let slug = kebab(heading);
        if !slug.is_empty() {
            return slug;
        }
    }
    let label = aug.link.original_text.trim();
    if !label.is_empty() {
        let slug = kebab(label);
        if !slug.is_empty() {
            return slug;
        }
    }
    let stem = file_stem_of(&aug.link.path);
    let slug = kebab(&stem);
    if slug.is_empty() {
        "anchor".to_string()
    } else {
        slug
    }
}

fn strip_leading_ordinal(s: &str) -> &str {
    let t = s.trim_start();
    let bytes = t.as_bytes();
    let mut k = 0;
    while k < bytes.len() && bytes[k].is_ascii_digit() {
        k += 1;
    }
    if k == 0 || k >= bytes.len() {
        return s;
    }
    if bytes[k] != b'.' && bytes[k] != b')' {
        return s;
    }
    let after = &t[k + 1..];
    after.strip_prefix(' ').unwrap_or(s)
}

fn file_stem_of(p: &str) -> String {
    let last = p.rsplit('/').next().unwrap_or(p);
    let last = last.split('#').next().unwrap_or(last);
    PathBuf::from(last)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn kebab(s: &str) -> String {
    let s = strip_leading_ordinal(s);
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = true;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{FragmentLink, LinkKind};

    fn aug_with(heading: &str, label: &str, path: &str) -> AugmentedLink {
        AugmentedLink {
            link: FragmentLink {
                kind: LinkKind::Internal,
                path: path.to_string(),
                start_line: Some(1),
                end_line: Some(2),
                text: label.to_string(),
                original_text: label.to_string(),
                original_href: path.to_string(),
                source_line: 1,
            },
            heading_chain: Vec::new(),
            section_heading: heading.to_string(),
            section_start_line: 1,
            section_end_line: 1,
        }
    }

    #[test]
    fn category_for_nested_wiki_page() {
        assert_eq!(derive_category("wiki/perf/indexing.md"), "wiki/perf");
    }

    #[test]
    fn category_for_top_level_wiki_page() {
        assert_eq!(derive_category("wiki/billing.md"), "wiki");
    }

    #[test]
    fn category_for_non_wiki_page() {
        assert_eq!(derive_category("src/notes.wiki.md"), "src");
    }

    #[test]
    fn category_empty_for_bare_filename() {
        assert_eq!(derive_category("README.md"), "");
    }

    #[test]
    fn kebab_drops_leading_ordinal_marker() {
        assert_eq!(kebab("3. Incremental Indexing"), "incremental-indexing");
        assert_eq!(kebab("12) Apply Phase"), "apply-phase");
        assert_eq!(kebab("Phase 3 details"), "phase-3-details");
    }

    #[test]
    fn noun_from_heading_is_kebabbed() {
        let a = aug_with("## Sync detection", "build_index", "src/index.rs");
        assert_eq!(derive_noun(&a), "sync-detection");
    }

    #[test]
    fn noun_falls_back_to_label() {
        let a = aug_with("", "bootstrap", "src/index.rs");
        assert_eq!(derive_noun(&a), "bootstrap");
    }

    #[test]
    fn noun_falls_back_to_file_stem() {
        let a = aug_with("", "", "src/index.rs");
        assert_eq!(derive_noun(&a), "index");
    }

    #[test]
    fn build_emits_page_section_anchor_then_targets() {
        let aug = aug_with("## Sync detection", "build_index", "src/index.rs");
        let group = SectionGroup {
            leader: &aug,
            section_start: 10,
            section_end: 20,
            target_anchors: vec!["src/index.rs#L10-L20".to_string()],
            structured_targets: vec![StructuredAnchor {
                path: "src/index.rs".to_string(),
                start_line: 10,
                end_line: 20,
            }],
        };
        let drafts = build("wiki/perf/indexing.md", &[group], Path::new("/"));
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].slug, "wiki/perf/sync-detection");
        assert_eq!(
            drafts[0].anchors,
            vec![
                "wiki/perf/indexing.md#L10-L20".to_string(),
                "src/index.rs#L10-L20".to_string()
            ]
        );
        assert_eq!(drafts[0].structured_anchors.len(), 2);
        assert_eq!(drafts[0].structured_anchors[0].path, "wiki/perf/indexing.md");
        assert_eq!(drafts[0].structured_anchors[0].start_line, 10);
        assert_eq!(drafts[0].structured_anchors[0].end_line, 20);
        assert_eq!(drafts[0].structured_anchors[1].path, "src/index.rs");
    }
}
