//! Phase-1 draft model: one `MeshDraft` per fragment link, before grouping.
//!
//! The draft carries everything the renderer needs (page, slug, anchors,
//! section context) and is the unit grouping operates on.
//! Slug derivation lives here.

use std::path::{Path, PathBuf};

use super::augment::AugmentedLink;

/// One mesh proposal before grouping. `consolidated_count` is `1` until
/// `group` merges siblings into it.
#[derive(Debug, Clone)]
pub(crate) struct MeshDraft {
    /// Source wiki page, repo-root-relative with forward slashes.
    pub(crate) page_path: String,
    /// Generated slug, e.g. `wiki/perf/sync-detection`.
    pub(crate) slug: String,
    /// Ordered list of anchor strings (`page` first, then targets like
    /// `src/index.rs#L10-L20`). Render emits these as a single `git mesh add`.
    pub(crate) anchors: Vec<String>,
    /// Heading text rendered with ATX hashes (e.g. `## Sync detection`), or
    /// empty when the link sits before any heading.
    pub(crate) section_heading: String,
    /// First prose sentence under the heading, cleaned of markdown link syntax.
    pub(crate) section_opening: String,
    /// Number of identical-anchor-set siblings merged into this draft. Starts at 1.
    pub(crate) consolidated_count: usize,
}

/// Build one draft per augmented link. `page_path` is the source wiki page
/// (already repo-root-relative). `target_anchors` is the per-link list of
/// anchor strings that go after `page_path` on the `git mesh add` line.
pub(crate) fn build(
    page_path: &str,
    augmented: &[AugmentedLink],
    target_anchors: &[Vec<String>],
    repo_root: &Path,
) -> Vec<MeshDraft> {
    assert_eq!(
        augmented.len(),
        target_anchors.len(),
        "draft::build expects parallel slices"
    );
    augmented
        .iter()
        .zip(target_anchors.iter())
        .map(|(aug, targets)| {
            let slug = derive_slug(page_path, aug, repo_root);
            let mut anchors = Vec::with_capacity(1 + targets.len());
            anchors.push(page_path.to_string());
            anchors.extend(targets.iter().cloned());
            MeshDraft {
                page_path: page_path.to_string(),
                slug,
                anchors,
                section_heading: aug.section_heading.clone(),
                section_opening: aug.section_opening.clone(),
                consolidated_count: 1,
            }
        })
        .collect()
}

/// Slug = `<category>/<noun>`. Category is the page's parent directory; for
/// `wiki/<sub>/file.md` that's `wiki/<sub>`, for `wiki/file.md` it's `wiki`,
/// for any other path it's the first path segment. Noun is the deepest section
/// heading kebab-cased; falls back to the link label, then the target file stem.
fn derive_slug(page_path: &str, aug: &AugmentedLink, _repo_root: &Path) -> String {
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
    // Drop the filename, keep the parent dir(s) up to two deep.
    let dirs = &parts[..parts.len() - 1];
    dirs.join("/")
}

fn derive_noun(aug: &AugmentedLink) -> String {
    // Strip ATX hashes from `section_heading` if present.
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
    // Drop a leading numbered-list prefix (`3. `, `12) `) so headings like
    // `### 3. Incremental indexing` slug to `incremental-indexing` instead
    // of leaking the digit into the slug.
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
            surrounding_text: String::new(),
            line_text: String::new(),
            heading_chain: Vec::new(),
            section_heading: heading.to_string(),
            section_opening: String::new(),
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
        // Mid-string digits stay.
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
    fn build_produces_one_draft_per_link_with_anchors_prefixed_by_page() {
        let augs = vec![aug_with("## Sync detection", "build_index", "src/index.rs")];
        let targets = vec![vec!["src/index.rs#L10-L20".to_string()]];
        let drafts = build("wiki/perf/indexing.md", &augs, &targets, Path::new("/"));
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].slug, "wiki/perf/sync-detection");
        assert_eq!(
            drafts[0].anchors,
            vec![
                "wiki/perf/indexing.md".to_string(),
                "src/index.rs#L10-L20".to_string()
            ]
        );
        assert_eq!(drafts[0].consolidated_count, 1);
    }

    #[test]
    fn build_uses_label_when_no_heading() {
        let augs = vec![aug_with("", "bootstrap", "src/index.rs")];
        let targets = vec![vec!["src/index.rs#L1-L5".to_string()]];
        let drafts = build("wiki/perf/indexing.md", &augs, &targets, Path::new("/"));
        assert_eq!(drafts[0].slug, "wiki/perf/bootstrap");
    }
}
