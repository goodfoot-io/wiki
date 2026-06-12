use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::commands::resolve_link_path;
use crate::parser::{LinkKind, parse_fragment_links};

use super::check::{CheckDiagnostic, ContentCache};
use super::mesh::scaffold::locate_existing_suffix;
use super::mesh::store;

// ── Types ─────────────────────────────────────────────────────────────────────

/// All mesh data read in-process from `.wiki/<slug>` files.
pub(crate) struct MeshIndex {
    /// Code anchor `(path, start, end)` → names of every mesh containing it.
    by_anchor: HashMap<(PathBuf, u32, u32), Vec<String>>,
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
        // ranges that `git mesh stale --fix` has already coalesced.
        let anchors_wiki = |name: &String| {
            self.paths_by_mesh
                .get(name)
                .is_some_and(|paths| paths.iter().any(|p| paths_equal(p, wiki_rel)))
        };
        self.by_anchor
            .iter()
            .filter(|((path, _, _), _)| paths_equal(path, code_path))
            .any(|((_, a_start, a_end), names)| {
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
        let key = (path.to_path_buf(), start, end);
        let names = self.by_anchor.get(&key)?;
        names.iter().min().map(String::as_str)
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
        self.by_anchor
            .iter()
            .filter(|((ap, _, _), _)| paths_equal(ap, path))
            .any(|((_, a_start, a_end), names)| {
                let contains = (*a_start == 0 && *a_end == 0)
                    || (*a_start <= start && end <= *a_end);
                contains && names.iter().any(|n| n == mesh)
            })
    }
}

// ── Public surface ────────────────────────────────────────────────────────────

/// Collect `mesh_uncovered` diagnostics for the given wiki files.
///
/// Reads `.wiki/<slug>` files in-process via [`store::read_all_tolerant`]
/// (skipping files that fail to parse), then performs all coverage lookups
/// in memory. Coverage is always computed — there is no external binary to
/// be unavailable.
pub(super) fn collect_mesh_diagnostics(
    files: &[PathBuf],
    repo_root: &Path,
    content_cache: &mut ContentCache,
) -> Result<Vec<CheckDiagnostic>, miette::Error> {
    let mut out: Vec<CheckDiagnostic> = Vec::new();

    if files.is_empty() {
        return Ok(out);
    }

    let rel_paths: Vec<PathBuf> = files
        .iter()
        .map(|p| {
            p.strip_prefix(repo_root)
                .map(Path::to_path_buf)
                .unwrap_or_else(|_| p.clone())
        })
        .collect();

    let index = build_mesh_index(repo_root, &rel_paths)?.expect("always Some");

    // Candidate `mesh_uncovered` diagnostics, deferred until after the
    // gitignore filter is applied below.
    struct Pending {
        file: String,
        line: usize,
        link_path: String,
        target: String,
        start: u32,
        end: u32,
    }
    let mut pending: Vec<Pending> = Vec::new();

    for wiki_path in files {
        let content = match content_cache
            .get_or_try_read(wiki_path, || std::fs::read_to_string(wiki_path))
        {
            Ok(c) => c,
            Err(_) => continue,
        };

        let wiki_rel = wiki_path
            .strip_prefix(repo_root)
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|_| wiki_path.clone());

        for link in parse_fragment_links(content) {
            if link.kind == LinkKind::External {
                continue;
            }
            let Some(start) = link.start_line else {
                continue;
            };
            let end = link.end_line.unwrap_or(start);
            let target = resolve_link_path(&link.path, wiki_path, repo_root);

            // Apply locate_existing_suffix salvage to match the same path
            // resolution that scaffold uses: when the resolved path doesn't
            // exist on disk, peel back directory components to find an
            // existing file whose suffix matches (e.g. the link path
            // `deep/path/src/code.rs` resolves to `src/code.rs`).
            let target = {
                let target_str = target.to_string_lossy().replace('\\', "/");
                locate_existing_suffix(&target_str, repo_root)
                    .map_or(target, PathBuf::from)
            };

            // Skip if target is a directory (consistent with the missing_file check)
            let abs_target = repo_root.join(&target);
            if abs_target.is_dir() {
                continue;
            }

            if !index.is_covered(&target, start, end, &wiki_rel) {
                pending.push(Pending {
                    file: wiki_path.display().to_string(),
                    line: link.source_line,
                    link_path: link.path.clone(),
                    target: target.to_string_lossy().replace('\\', "/"),
                    start,
                    end,
                });
            }
        }
    }

    // A fragment link into a gitignored path (a generated build artifact) is
    // exempt from the mesh-coverage contract: a path git never sees cannot be
    // anchored, so demanding coverage would be unsatisfiable and `wiki check`
    // would fail closed forever. Mirrors the
    // existing exemptions for external links and links without a line range.
    // Untracked-but-not-ignored targets are NOT exempt — they resolve once
    // committed, so a missing mesh for them is a real, fixable finding.
    let candidate_targets: Vec<String> = pending.iter().map(|p| p.target.clone()).collect();
    let ignored = crate::git::ignored_paths(repo_root, &candidate_targets)?;

    for p in pending {
        if ignored.contains(&p.target) {
            continue;
        }
        out.push(CheckDiagnostic {
            kind: "mesh_uncovered".into(),
            file: p.file,
            line: p.line,
            message: format!(
                "fragment link `{}#L{}-L{}` has no covering mesh",
                p.link_path, p.start, p.end
            ),
        });
    }

    Ok(out)
}

/// Build a `MeshIndex` by reading `.wiki/<slug>` files in-process via
/// [`store::read_all_tolerant`] (skips files that fail to parse, e.g. due to
/// git conflict markers).
///
/// Always returns `Ok(Some(_))` — coverage is always computed, there is no
/// external binary to be unavailable. Callers in `check_fix.rs` and
/// `scaffold.rs` that previously handled `None` (binary missing) should treat
/// `Some` as the only variant.
///
/// Files that fail to parse are skipped and logged to stderr. In non-fix
/// mode, `check.rs` independently detects and reports conflict-markered
/// meshes so the fail-closed guarantee is preserved at the caller level.
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

    let mut by_anchor: HashMap<(PathBuf, u32, u32), Vec<String>> = HashMap::new();
    let mut paths_by_mesh: HashMap<String, Vec<PathBuf>> = HashMap::new();

    for (slug, mesh_file) in meshes {
        for anchor in &mesh_file.anchors {
            let path = PathBuf::from(&anchor.path);
            let key = (path.clone(), anchor.start_line, anchor.end_line);
            by_anchor.entry(key).or_default().push(slug.clone());

            let mesh_paths = paths_by_mesh.entry(slug.clone()).or_default();
            if !mesh_paths.iter().any(|p| paths_equal(p, &path)) {
                mesh_paths.push(path);
            }
        }
    }

    Ok(Some(MeshIndex {
        by_anchor,
        paths_by_mesh,
    }))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Normalize-compare two paths by components, handling leading `./`.
fn paths_equal(a: &Path, b: &Path) -> bool {
    use std::path::Component;
    let a_components: Vec<_> = a
        .components()
        .filter(|c| !matches!(c, Component::CurDir))
        .collect();
    let b_components: Vec<_> = b
        .components()
        .filter(|c| !matches!(c, Component::CurDir))
        .collect();
    a_components == b_components
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
    fn paths_equal_ignores_leading_dotslash() {
        assert!(paths_equal(Path::new("./foo/bar"), Path::new("foo/bar")));
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
