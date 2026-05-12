//! Phase-1 draft model: one `MeshDraft` per *section* (deepest enclosing
//! heading, or paragraph fallback), built from the augmented links of a page.
//!
//! Each draft carries the page section anchor as the leading anchor and the
//! merged, deduplicated set of target anchors that the section's links point
//! at. The draft is the unit grouping operates on. Slug derivation lives here.

use std::path::{Path, PathBuf};

use super::augment::AugmentedLink;
use super::scaffold::PageNamespace;

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
    /// Kebab-cased noun derived for this draft (deepest section heading,
    /// link label, or file stem fallback). Retained so the collision
    /// resolver can rebuild the slug with extra qualifiers.
    pub(crate) noun: String,
    /// Snapshot of the page's namespace context at draft-build time. Used by
    /// the collision resolver to reapply [`build_slug_with_qualifiers`] when
    /// the base slug clashes with an existing mesh.
    pub(crate) page_ns: PageNamespace,
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
    page_ns: &PageNamespace,
) -> Vec<MeshDraft> {
    groups
        .iter()
        .map(|g| {
            let noun = derive_noun(g.leader);
            let slug = build_slug(page_ns, &noun);
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
                noun,
                page_ns: page_ns.clone(),
            }
        })
        .collect()
}

/// Compose the slug from a `PageNamespace` and a kebab-cased noun, applying
/// the no-repeat invariant on `wiki` and the namespace name.
///
/// Slug = `<prefix>/<inner-subdir>/<noun>`, where:
///   - `<prefix>` is `wiki` for default-namespace pages and `wiki/<ns>` for
///     pages in a named-namespace wiki;
///   - `<inner-subdir>` is the page's directory relative to its owning wiki
///     root (empty for pages at the wiki root, and empty for `.wiki.md`
///     floats that have no owning root);
///   - `<noun>` comes from the deepest section heading kebab-cased, falling
///     back to the link label, then the target file stem.
///
/// The `wiki/` and `wiki/<ns>/` prefixes are added exactly once and never
/// repeated inside the slug: any inner-subdir segment equal to `wiki` (or to
/// the namespace, when set) is stripped before concatenation, so that a page
/// at `wiki/wiki/foo.md` or `mesh/mesh/foo.md` does not produce duplicate
/// prefix segments.
pub(crate) fn build_slug(page_ns: &PageNamespace, noun: &str) -> String {
    build_slug_with_qualifiers(page_ns, &[], noun)
}

/// Like [`build_slug`], but inserts extra **qualifier** segments
/// (outer→inner) between the subdir and the final noun. Used by the
/// collision resolver to disambiguate by adding heading-chain or page-title
/// context when the base slug clashes with an existing mesh.
///
/// Qualifiers are subject to the same no-repeat invariant: any qualifier
/// segment equal to `wiki`, the namespace, or already present in `parts` is
/// silently dropped so we never emit `wiki/foo/foo/bar`.
pub(crate) fn build_slug_with_qualifiers(
    page_ns: &PageNamespace,
    qualifiers: &[String],
    noun: &str,
) -> String {
    let mut parts: Vec<String> = vec!["wiki".to_string()];
    if let Some(ns) = page_ns.namespace.as_deref() {
        parts.push(ns.to_string());
    }
    let mut reserved: std::collections::HashSet<String> = std::collections::HashSet::new();
    reserved.insert("wiki".to_string());
    if let Some(ns) = page_ns.namespace.as_deref() {
        reserved.insert(ns.to_string());
    }
    for seg in page_ns.subdir.split('/') {
        if seg.is_empty() || reserved.contains(seg) {
            continue;
        }
        parts.push(seg.to_string());
        reserved.insert(seg.to_string());
    }
    for q in qualifiers {
        if q.is_empty() || reserved.contains(q) {
            continue;
        }
        parts.push(q.clone());
        reserved.insert(q.clone());
    }
    parts.push(noun.to_string());
    parts.join("/")
}

pub(crate) fn derive_noun(aug: &AugmentedLink) -> String {
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

pub(crate) fn kebab(s: &str) -> String {
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

    fn ns(namespace: Option<&str>, subdir: &str) -> PageNamespace {
        PageNamespace {
            namespace: namespace.map(|s| s.to_string()),
            subdir: subdir.to_string(),
        }
    }

    #[test]
    fn slug_default_namespace_top_level() {
        assert_eq!(build_slug(&ns(None, ""), "billing"), "wiki/billing");
    }

    #[test]
    fn slug_default_namespace_with_subdir() {
        assert_eq!(
            build_slug(&ns(None, "perf"), "sync-detection"),
            "wiki/perf/sync-detection"
        );
    }

    #[test]
    fn slug_named_namespace_top_level() {
        assert_eq!(build_slug(&ns(Some("mesh"), ""), "foo"), "wiki/mesh/foo");
    }

    #[test]
    fn slug_named_namespace_with_subdir() {
        assert_eq!(
            build_slug(&ns(Some("mesh"), "sub"), "bar"),
            "wiki/mesh/sub/bar"
        );
    }

    #[test]
    fn slug_strips_repeated_wiki_segment() {
        // A page at wiki/wiki/foo.md in the default-ns wiki at wiki/ would
        // produce subdir "wiki" — drop it so the prefix is not repeated.
        assert_eq!(build_slug(&ns(None, "wiki"), "foo"), "wiki/foo");
    }

    #[test]
    fn slug_strips_repeated_namespace_segment() {
        // A page at mesh/mesh/foo.md in the mesh-ns wiki at mesh/ would
        // produce subdir "mesh" — drop it so the namespace is not repeated.
        assert_eq!(build_slug(&ns(Some("mesh"), "mesh"), "foo"), "wiki/mesh/foo");
    }

    #[test]
    fn slug_strips_repeats_anywhere_in_subdir() {
        assert_eq!(
            build_slug(&ns(Some("mesh"), "wiki/mesh/sub"), "leaf"),
            "wiki/mesh/sub/leaf"
        );
    }

    #[test]
    fn qualifiers_insert_between_subdir_and_noun() {
        assert_eq!(
            build_slug_with_qualifiers(
                &ns(None, "perf"),
                &["bootstrap".to_string()],
                "sync-detection"
            ),
            "wiki/perf/bootstrap/sync-detection"
        );
    }

    #[test]
    fn qualifiers_preserve_outer_to_inner_order() {
        assert_eq!(
            build_slug_with_qualifiers(
                &ns(None, ""),
                &["billing".to_string(), "checkout".to_string()],
                "charge-handler"
            ),
            "wiki/billing/checkout/charge-handler"
        );
    }

    #[test]
    fn qualifiers_drop_reserved_segments() {
        // `wiki` is reserved (prefix); `perf` is already in subdir.
        assert_eq!(
            build_slug_with_qualifiers(
                &ns(None, "perf"),
                &["wiki".to_string(), "perf".to_string(), "extra".to_string()],
                "leaf"
            ),
            "wiki/perf/extra/leaf"
        );
    }

    #[test]
    fn qualifiers_drop_namespace_repeats() {
        assert_eq!(
            build_slug_with_qualifiers(
                &ns(Some("mesh"), ""),
                &["mesh".to_string(), "inner".to_string()],
                "leaf"
            ),
            "wiki/mesh/inner/leaf"
        );
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
        let page_ns = ns(None, "perf");
        let drafts = build("wiki/perf/indexing.md", &[group], Path::new("/"), &page_ns);
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
