use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::mesh::store;

// ── Types ─────────────────────────────────────────────────────────────────────

/// All mesh data read in-process from `.wiki/<slug>` files.
pub(crate) struct MeshIndex {
    /// Normalized path → list of `(start_line, end_line, mesh_names)` entries.
    by_anchor: HashMap<PathBuf, Vec<(u32, u32, Vec<String>)>>,
    /// Mesh name → every path anchored by that mesh (any range).
    paths_by_mesh: HashMap<String, Vec<PathBuf>>,
}

impl MeshIndex {
    pub(crate) fn is_covered(
        &self,
        code_path: &Path,
        start: u32,
        end: u32,
        wiki_rel: &Path,
    ) -> bool {
        // A link `(path, start, end)` is covered when some anchor on the same
        // path — belonging to a mesh that also anchors the wiki file — either
        // is the whole-file sentinel (0-0) or contains the link range
        // (`anchor.start <= start && end <= anchor.end`). Containment (rather
        // than exact-range matching) keeps `wiki check --fix` from re-splitting
        // ranges that `git span drift --fix` has already coalesced.
        let normalized = normalize_path(code_path);
        let Some(entries) = self.by_anchor.get(&normalized) else {
            return false;
        };
        let wiki_normalized = normalize_path(wiki_rel);
        let anchors_wiki = |name: &String| {
            self.paths_by_mesh
                .get(name)
                .is_some_and(|paths| paths.iter().any(|p| p == &wiki_normalized))
        };
        entries.iter().any(|(a_start, a_end, names)| {
            let contains =
                (*a_start == 0 && *a_end == 0) || (*a_start <= start && end <= *a_end);
            contains && names.iter().any(&anchors_wiki)
        })
    }

    /// Return the name of an existing mesh that anchors the exact
    /// `(path, start, end)` triple, if one exists. When multiple meshes
    /// anchor the same triple the lexicographically first name wins so the
    /// choice is deterministic.
    pub(crate) fn owning_mesh_for_exact(&self, path: &Path, start: u32, end: u32) -> Option<&str> {
        let normalized = normalize_path(path);
        let entries = self.by_anchor.get(&normalized)?;
        entries
            .iter()
            .find(|(s, e, _)| *s == start && *e == end)
            .and_then(|(_, _, names)| names.iter().min().map(String::as_str))
    }

    /// Whether `mesh` contains an anchor on `path` whose range contains
    /// `(start, end)`, i.e. containment semantics (`anchor.start <= start &&
    /// end <= anchor.end`), or the whole-file `(path, 0, 0)` sentinel that
    /// covers any range.
    ///
    /// Using containment (rather than exact-range matching) keeps
    /// `apply_section_extension` from re-appending a code anchor that is
    /// already covered by a broader existing anchor — which would otherwise
    /// oscillate once post-pass coalescing re-merges the ranges.
    pub(crate) fn mesh_contains_anchor(
        &self,
        mesh: &str,
        path: &Path,
        start: u32,
        end: u32,
    ) -> bool {
        let normalized = normalize_path(path);
        let Some(entries) = self.by_anchor.get(&normalized) else {
            return false;
        };
        entries.iter().any(|(a_start, a_end, names)| {
            let contains = (*a_start == 0 && *a_end == 0)
                || (*a_start <= start && end <= *a_end);
            contains && names.iter().any(|n| n == mesh)
        })
    }
}

// ── Public surface ────────────────────────────────────────────────────────────

/// Build a `MeshIndex` by reading `.wiki/<slug>` files in-process via
/// [`store::read_all_tolerant`] (skips files that fail to parse, e.g. due to
/// git conflict markers).
///
/// Always returns `Ok(Some(_))` — coverage is always computed, there is no
/// external binary to be unavailable. Callers in `check_fix.rs` and
/// `scaffold.rs` that previously handled `None` (binary missing) should treat
/// `Some` as the only variant.
///
/// Files that fail to parse are skipped and logged to stderr.
pub(crate) fn build_mesh_index(
    repo_root: &Path,
    _files: &[PathBuf],
) -> Result<Option<MeshIndex>, miette::Error> {
    let (meshes, skipped) = store::read_all_tolerant(repo_root)?;
    if !skipped.is_empty() {
        eprintln!(
            "wiki: skipped {} unparseable mesh(es) during index build",
            skipped.len()
        );
    }

    let mut by_anchor: HashMap<PathBuf, Vec<(u32, u32, Vec<String>)>> = HashMap::new();
    let mut paths_by_mesh: HashMap<String, Vec<PathBuf>> = HashMap::new();

    for (slug, mesh_file) in meshes {
        for anchor in &mesh_file.anchors {
            let raw_path = PathBuf::from(&anchor.path);
            let normalized_path = normalize_path(&raw_path);

            // Group entries by normalized path; dedup slugs with same (start, end).
            let entries = by_anchor.entry(normalized_path.clone()).or_default();
            if let Some(existing) =
                entries.iter_mut().find(|(s, e, _)| *s == anchor.start_line && *e == anchor.end_line)
            {
                if !existing.2.contains(&slug) {
                    existing.2.push(slug.clone());
                }
            } else {
                entries.push((anchor.start_line, anchor.end_line, vec![slug.clone()]));
            }

            // Track path per mesh, dedup by normalized path.
            let mesh_paths = paths_by_mesh.entry(slug.clone()).or_default();
            if !mesh_paths.iter().any(|p| p == &normalized_path) {
                mesh_paths.push(normalized_path);
            }
        }
    }

    Ok(Some(MeshIndex {
        by_anchor,
        paths_by_mesh,
    }))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Normalize a path by stripping leading `./` components into a `PathBuf`
/// suitable for use as a HashMap key.
fn normalize_path(path: &Path) -> PathBuf {
    path.components()
        .filter(|c| !matches!(c, std::path::Component::CurDir))
        .collect()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use git_mesh_core::mesh_file::{AnchorRecord, MeshFile};
    use tempfile::TempDir;

    fn make_anchor(path: &str, start: u32, end: u32) -> AnchorRecord {
        AnchorRecord {
            path: path.to_string(),
            start_line: start,
            end_line: end,
            algorithm: "sha256".to_string(),
            content_hash: "deadbeef".to_string(),
        }
    }

    fn make_mesh(anchors: Vec<AnchorRecord>) -> MeshFile {
        MeshFile {
            anchors,
            why: "test".to_string(),
        }
    }

    fn build_index_from_store(dir: &TempDir) -> MeshIndex {
        build_mesh_index(dir.path(), &[])
            .expect("build_mesh_index")
            .expect("always Some")
    }

    #[test]
    fn empty_store_yields_empty_index() {
        let dir = tempfile::tempdir().unwrap();
        let idx = build_index_from_store(&dir);
        assert!(idx.by_anchor.is_empty());
        assert!(idx.paths_by_mesh.is_empty());
    }

    #[test]
    fn whole_file_anchor_covers_any_range() {
        let dir = tempfile::tempdir().unwrap();
        let mesh = make_mesh(vec![
            make_anchor("src/code.rs", 0, 0),
            make_anchor("wiki/page.md", 1, 1),
        ]);
        store::write(dir.path(), "test-mesh", &mesh).unwrap();

        let idx = build_index_from_store(&dir);
        assert!(idx.is_covered(Path::new("src/code.rs"), 1, 1, Path::new("wiki/page.md")));
        assert!(idx.is_covered(Path::new("src/code.rs"), 10, 20, Path::new("wiki/page.md")));
    }

    #[test]
    fn coverage_requires_both_anchors_in_same_mesh() {
        let dir = tempfile::tempdir().unwrap();
        // mesh-a has code + wiki
        let mesh_a = make_mesh(vec![
            make_anchor("src/code.rs", 1, 10),
            make_anchor("wiki/page.md", 1, 1),
        ]);
        store::write(dir.path(), "mesh-a", &mesh_a).unwrap();
        // mesh-b has code only
        let mesh_b = make_mesh(vec![make_anchor("src/code.rs", 1, 10)]);
        store::write(dir.path(), "mesh-b", &mesh_b).unwrap();

        let idx = build_index_from_store(&dir);
        assert!(idx.is_covered(Path::new("src/code.rs"), 1, 10, Path::new("wiki/page.md")));
    }

    #[test]
    fn coverage_fails_when_no_mesh_has_wiki_file() {
        let dir = tempfile::tempdir().unwrap();
        let mesh = make_mesh(vec![make_anchor("src/code.rs", 1, 10)]);
        store::write(dir.path(), "mesh-a", &mesh).unwrap();

        let idx = build_index_from_store(&dir);
        assert!(!idx.is_covered(Path::new("src/code.rs"), 1, 10, Path::new("wiki/page.md")));
    }

    #[test]
    fn broad_anchor_covers_contained_narrow_range() {
        let dir = tempfile::tempdir().unwrap();
        let mesh = make_mesh(vec![
            make_anchor("src/code.rs", 1, 100),
            make_anchor("wiki/page.md", 1, 1),
        ]);
        store::write(dir.path(), "broad-mesh", &mesh).unwrap();

        let idx = build_index_from_store(&dir);
        assert!(idx.is_covered(Path::new("src/code.rs"), 50, 60, Path::new("wiki/page.md")));
    }

    #[test]
    fn slug_without_tabs_is_used_as_mesh_name() {
        let dir = tempfile::tempdir().unwrap();
        let mesh = make_mesh(vec![make_anchor("src/code.rs", 1, 10)]);
        store::write(dir.path(), "my-slug", &mesh).unwrap();

        let idx = build_index_from_store(&dir);
        assert!(idx.paths_by_mesh.contains_key("my-slug"));
    }

    #[test]
    fn nested_slug_is_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let mesh = make_mesh(vec![make_anchor("src/code.rs", 1, 5)]);
        store::write(dir.path(), "subsystem/feature/detail", &mesh).unwrap();

        let idx = build_index_from_store(&dir);
        assert!(idx.paths_by_mesh.contains_key("subsystem/feature/detail"));
    }

    #[test]
    fn normalize_path_strips_curdir() {
        assert_eq!(normalize_path(Path::new("./foo/bar")), PathBuf::from("foo/bar"));
        assert_eq!(normalize_path(Path::new("foo/bar")), PathBuf::from("foo/bar"));
        assert_eq!(normalize_path(Path::new("./foo/./bar")), PathBuf::from("foo/bar"));
    }

    /// Reproduction for exact-vs-containment anchor filtering disagreement.
    ///
    /// `is_covered` uses containment (`a_start <= start && end <= a_end`) so a
    /// narrow range inside a broader anchor is considered covered. But
    /// `mesh_contains_anchor` checks only the **exact** `(path, start, end)`
    /// triple. When `apply_section_extension` calls `mesh_contains_anchor` to
    /// decide whether to filter a code anchor from an extension draft, a
    /// contained range (e.g. L5-L10 inside existing L5-L20) passes through
    /// unfiltered → `wiki check --fix` re-appends it → `check` still sees it
    /// as covered → permanent oscillation once range coalescing re-merges.
    ///
    /// This test **fails** until `mesh_contains_anchor` (or its call site in
    /// `apply_section_extension`) adopts containment semantics.
    #[test]
    fn mesh_contains_anchor_uses_containment_not_exact_match() {
        let dir = tempfile::tempdir().unwrap();
        let mesh = make_mesh(vec![make_anchor("src/code.rs", 5, 20)]);
        store::write(dir.path(), "test-mesh", &mesh).unwrap();

        let idx = build_index_from_store(&dir);

        // Under containment semantics, L5-L10 is inside L5-L20 so
        // mesh_contains_anchor should return true. It currently returns false
        // (exact-match only).
        assert!(
            idx.mesh_contains_anchor("test-mesh", Path::new("src/code.rs"), 5, 10),
            "mesh_contains_anchor must use containment: (5,10) is inside existing anchor (5,20)"
        );
    }
}
