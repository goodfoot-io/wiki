//! Markdown rendering for the build-then-render pipeline.
//!
//! Consumes deduplicated `MeshDraft`s and emits a review-ready markdown
//! document described by `tests/fixtures/mesh-scaffold/expected.md`.

use std::collections::HashMap;

use super::draft::MeshDraft;

/// Render `meshes` (already grouped per-page in declaration order) and the
/// per-page titles (frontmatter `title` keyed by `page_path`, `None` when
/// absent) into a markdown document.
pub(crate) fn render_markdown(
    meshes: &[MeshDraft],
    page_titles: &HashMap<String, Option<String>>,
) -> String {
    let mut out = String::new();

    // Group by page in first-occurrence order.
    let mut by_page: Vec<(String, Vec<&MeshDraft>)> = Vec::new();
    for m in meshes {
        if let Some(entry) = by_page.iter_mut().find(|(k, _)| *k == m.page_path) {
            entry.1.push(m);
        } else {
            by_page.push((m.page_path.clone(), vec![m]));
        }
    }

    for (page, page_meshes) in &by_page {
        let title = page_titles.get(page).and_then(|t| t.as_deref());
        let header = match title {
            Some(t) if !t.is_empty() => format!("# {t} • {page}"),
            _ => format!("# {page}"),
        };
        let _ = std::fmt::Write::write_fmt(&mut out, format_args!("{header}\n"));
        out.push('\n');
        for m in page_meshes {
            render_mesh_block(&mut out, m);
        }
    }

    out
}

fn render_mesh_block(out: &mut String, m: &MeshDraft) {
    use std::fmt::Write as _;
    let heading_text = strip_atx_hashes(&m.section_heading);
    let heading = if heading_text.is_empty() {
        "(top of file)".to_string()
    } else {
        heading_text
    };
    let _ = writeln!(out, "## {heading}");
    let _ = writeln!(out, "> {}", m.section_opening);
    out.push('\n');

    let _ = writeln!(out, "```bash");
    let _ = writeln!(out, "git mesh add {} \\", m.slug);
    let last = m.anchors.len().saturating_sub(1);
    for (i, a) in m.anchors.iter().enumerate() {
        if i == last {
            let _ = writeln!(out, "  {a}");
        } else {
            let _ = writeln!(out, "  {a} \\");
        }
    }
    let _ = writeln!(out, "git mesh why {} -m \"[why]\"", m.slug);
    let _ = writeln!(out, "```");
    out.push('\n');
}

fn strip_atx_hashes(s: &str) -> String {
    s.trim_start_matches(|c: char| c == '#' || c.is_whitespace())
        .trim()
        .to_string()
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_atx_hashes_drops_leading_hashes() {
        assert_eq!(strip_atx_hashes("## Sync detection"), "Sync detection");
        assert_eq!(
            strip_atx_hashes("# Charge handler notes"),
            "Charge handler notes"
        );
        assert_eq!(strip_atx_hashes(""), "");
    }
}
