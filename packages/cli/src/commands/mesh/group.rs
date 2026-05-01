//! Within-page anchor grouping:
//!
//! Identical-anchor-set siblings on the same page are merged into one mesh.
//! All other drafts stay separate.
//!
//! Operates on drafts from a *single* page. The caller chunks by page first.

use std::collections::BTreeMap;

use super::draft::MeshDraft;

/// Merge identical-anchor-set siblings. Returns drafts in the order their
/// first occurrence appeared in the input.
pub(crate) fn consolidate_within_page(drafts: Vec<MeshDraft>) -> Vec<MeshDraft> {
    // ── Phase 1: identical-anchor-set merge ────────────────────────────────
    // Key: the full anchor list, joined with `\n` (anchors carry no newlines).
    let mut order: Vec<String> = Vec::new();
    let mut by_key: BTreeMap<String, Vec<MeshDraft>> = BTreeMap::new();
    for d in drafts {
        let key = d.anchors.join("\n");
        if !by_key.contains_key(&key) {
            order.push(key.clone());
        }
        by_key.entry(key).or_default().push(d);
    }
    let mut merged: Vec<MeshDraft> = Vec::with_capacity(order.len());
    for key in order {
        let group = by_key.remove(&key).expect("key tracked in order");
        let count = group.len();
        let mut first = group.into_iter().next().expect("non-empty group");
        if count > 1 {
            first.consolidated_count = count;
        }
        merged.push(first);
    }
    merged
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(slug: &str, _heading: &str, anchors: &[&str]) -> MeshDraft {
        MeshDraft {
            page_path: anchors[0].to_string(),
            slug: slug.to_string(),
            anchors: anchors.iter().map(|s| s.to_string()).collect(),
            structured_anchors: Vec::new(),
            section_opening: String::new(),
            heading_chain: Vec::new(),
            section_opening_lines: Vec::new(),
            consolidated_count: 1,
        }
    }

    #[test]
    fn identical_anchor_sets_merge() {
        let drafts = vec![
            draft(
                "wiki/cli/parser",
                "# CLI parser",
                &["wiki/cli/parser.md", "src/parser.rs#L2-L4"],
            ),
            draft(
                "wiki/cli/parser-2",
                "# CLI parser",
                &["wiki/cli/parser.md", "src/parser.rs#L2-L4"],
            ),
        ];
        let out = consolidate_within_page(drafts);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].consolidated_count, 2);
    }

    #[test]
    fn different_anchor_sets_do_not_merge() {
        let drafts = vec![
            draft(
                "wiki/perf/sync-detection",
                "## Sync detection",
                &["wiki/perf/indexing.md", "src/index.rs#L10-L20"],
            ),
            draft(
                "wiki/perf/apply-phase",
                "## Apply phase",
                &["wiki/perf/indexing.md", "src/index.rs#L25-L40"],
            ),
        ];
        let out = consolidate_within_page(drafts);
        assert_eq!(out.len(), 2);
        for d in &out {
            assert_eq!(d.consolidated_count, 1);
        }
    }
}
