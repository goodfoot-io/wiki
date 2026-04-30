//! Within-page anchor grouping per the card's anchor-grouping table:
//!
//! | Anchors share | Default | Hint |
//! |---|---|---|
//! | Identical target set within one page | Merge into one mesh | `Consolidated { count }` |
//! | Section heading **and** target file | Stay separate | `ConsiderMerge { other_slug }` (both sides) |
//! | Section heading only | Stay separate | none |
//! | Target file only | Stay separate | none |
//! | Different pages | Stay separate | none |
//!
//! Operates on drafts from a *single* page. The caller chunks by page first.

use std::collections::BTreeMap;

use super::draft::MeshDraft;
use super::hints::Hint;

/// Merge identical-anchor-set siblings, then annotate heading-and-file
/// overlaps with `ConsiderMerge` hints (both directions). Returns drafts in
/// the order their first occurrence appeared in the input.
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
            first.hints.push(Hint::Consolidated { count });
        }
        merged.push(first);
    }
    merged
}

/// Attach `ConsiderMerge` hints to drafts that share a section heading AND a
/// target file. Run AFTER global slug dedup so the recorded `other_slug`
/// references the post-dedup name. Operates on the full deduped list and
/// only flags pairs from the same page.
pub(crate) fn attach_consider_merge(drafts: &mut [MeshDraft]) {
    let n = drafts.len();
    for i in 0..n {
        for j in (i + 1)..n {
            if drafts[i].page_path != drafts[j].page_path {
                continue;
            }
            if !shares_heading_and_file(&drafts[i], &drafts[j]) {
                continue;
            }
            let other_j = drafts[j].slug.clone();
            let other_i = drafts[i].slug.clone();
            drafts[i].hints.push(Hint::ConsiderMerge { other_slug: other_j });
            drafts[j].hints.push(Hint::ConsiderMerge { other_slug: other_i });
        }
    }
}

fn shares_heading_and_file(a: &MeshDraft, b: &MeshDraft) -> bool {
    if a.section_heading.is_empty() || a.section_heading != b.section_heading {
        return false;
    }
    let files_a = target_files(a);
    let files_b = target_files(b);
    files_a.iter().any(|f| files_b.contains(f))
}

fn target_files(d: &MeshDraft) -> Vec<String> {
    d.anchors
        .iter()
        .skip(1) // anchors[0] is the page itself
        .map(|a| a.split('#').next().unwrap_or(a).to_string())
        .collect()
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(slug: &str, heading: &str, anchors: &[&str]) -> MeshDraft {
        MeshDraft {
            page_path: anchors[0].to_string(),
            slug: slug.to_string(),
            anchors: anchors.iter().map(|s| s.to_string()).collect(),
            section_heading: heading.to_string(),
            section_opening: String::new(),
            hints: Vec::new(),
            consolidated_count: 1,
            section_opening_degenerate: false,
            had_code_span_lead: false,
        }
    }

    #[test]
    fn identical_anchor_sets_merge_with_consolidated_hint() {
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
        assert!(matches!(
            out[0].hints.first(),
            Some(Hint::Consolidated { count: 2 })
        ));
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
            assert!(d.hints.is_empty());
        }
    }

    #[test]
    fn shared_heading_and_file_emits_consider_merge_both_sides() {
        let mut drafts = vec![
            draft(
                "wiki/perf/apply-phase",
                "## Apply phase",
                &["wiki/perf/indexing.md", "src/index.rs#L25-L40"],
            ),
            draft(
                "wiki/perf/apply-phase-2",
                "## Apply phase",
                &["wiki/perf/indexing.md", "src/index.rs#L45-L60"],
            ),
        ];
        attach_consider_merge(&mut drafts);
        assert_eq!(drafts.len(), 2);
        assert!(matches!(
            drafts[0].hints.first(),
            Some(Hint::ConsiderMerge { other_slug }) if other_slug == "wiki/perf/apply-phase-2"
        ));
        assert!(matches!(
            drafts[1].hints.first(),
            Some(Hint::ConsiderMerge { other_slug }) if other_slug == "wiki/perf/apply-phase"
        ));
    }

    #[test]
    fn shared_heading_only_emits_no_hint() {
        let mut drafts = vec![
            draft(
                "wiki/perf/h-a",
                "## Same",
                &["wiki/perf/p.md", "src/a.rs#L1-L2"],
            ),
            draft(
                "wiki/perf/h-b",
                "## Same",
                &["wiki/perf/p.md", "src/b.rs#L1-L2"],
            ),
        ];
        attach_consider_merge(&mut drafts);
        for d in &drafts {
            assert!(d.hints.is_empty(), "got: {:?}", d.hints);
        }
    }

    #[test]
    fn shared_file_only_emits_no_hint() {
        let mut drafts = vec![
            draft(
                "wiki/perf/one",
                "## One",
                &["wiki/perf/p.md", "src/a.rs#L1-L2"],
            ),
            draft(
                "wiki/perf/two",
                "## Two",
                &["wiki/perf/p.md", "src/a.rs#L5-L9"],
            ),
        ];
        attach_consider_merge(&mut drafts);
        for d in &drafts {
            assert!(d.hints.is_empty(), "got: {:?}", d.hints);
        }
    }
}
