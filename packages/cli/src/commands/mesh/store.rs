//! In-process `.wiki/<slug>` mesh storage.
//!
//! Owns the storage and hashing primitives for in-process mesh operations.
//! shell-outs. Anchors live under `repo_root/.wiki/<slug>` as plain text in the
//! `git-mesh-core` [`MeshFile`] format (byte-identical to the legacy `.mesh/`
//! format). Every anchor identity flows through [`git_mesh_core::cheap_fingerprint_with_extent`] (rk64)
//! so a stored hash means the same thing on both sides by construction.
//!
//! Greenfield: the location is fixed at `repo_root/.wiki` — no env or
//! git-config indirection, no `.mesh` handling, no fallback.
//!
//! Groups 1–3 complete the cutover: Group 1 landed the primitives, Group 2
//! wired the read path, and Group 3 wires the write path. All public items are
//! now used in production code.

use std::fs;
use std::path::{Path, PathBuf};

use miette::Result;
use walkdir::WalkDir;

use git_mesh_core::mesh_file::{AnchorRecord, MeshFile};
use git_mesh_core::{AnchorExtent, RK64_ALGORITHM, cheap_fingerprint_with_extent, rk64_to_hex};

/// The fixed mesh storage directory: `repo_root/.wiki`.
pub(crate) fn wiki_dir(repo_root: &Path) -> PathBuf {
    repo_root.join(".wiki")
}

/// Resolve the on-disk path of the mesh file for `slug`.
fn slug_path(repo_root: &Path, slug: &str) -> PathBuf {
    let mut path = wiki_dir(repo_root);
    for component in slug.split('/') {
        path.push(component);
    }
    path
}

/// Whether a filename under `.wiki/` is a non-mesh runtime artifact written by
/// the CLI (the index cache DB, its SQLite WAL/SHM/journal sidecars, the
/// refresh lock, and the perf log). These coexist with mesh files under
/// `.wiki/` but must never be parsed as meshes.
fn is_runtime_artifact(name: &str) -> bool {
    name == "wiki-index.sqlite"
        || name.starts_with("wiki-index.sqlite-")
        || name == "wiki-refresh.lock"
        || name == "wiki.log"
}

/// Slug for a file under `.wiki/`: the relative path with forward slashes.
fn slug_for(root: &Path, file: &Path) -> Option<String> {
    let rel = file.strip_prefix(root).ok()?;
    Some(rel.to_string_lossy().replace('\\', "/"))
}

/// Walk `.wiki/` recursively, parsing each file into a [`MeshFile`].
///
/// A missing `.wiki/` directory yields an empty `Vec` (not an error).
pub(crate) fn read_all(repo_root: &Path) -> Result<Vec<(String, MeshFile)>> {
    let root = wiki_dir(repo_root);
    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut out = Vec::new();
    for entry in WalkDir::new(&root).sort_by_file_name() {
        let entry = entry
            .map_err(|e| miette::miette!("failed to walk {}: {e}", root.display()))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        // Skip hidden/dotfiles (e.g. .DS_Store, editor swap files) and the
        // index cache DB / its sidecars / runtime files that live under `.wiki/`
        // alongside the mesh store. These are not meshes and must never be
        // parsed (MeshFile::parse fails closed on the sqlite binary).
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.starts_with('.') || is_runtime_artifact(n))
            .unwrap_or(false)
        {
            continue;
        }
        let Some(slug) = slug_for(&root, path) else {
            continue;
        };
        // Fail closed: any non-dotfile that cannot be read as UTF-8 or parsed
        // as a mesh is a hard error. A committed mesh file with git conflict
        // markers must surface loudly rather than silently drop coverage.
        let text = fs::read_to_string(path)
            .map_err(|e| miette::miette!("failed to read mesh `{}`: {e}", path.display()))?;
        let mesh = MeshFile::parse(&text)
            .map_err(|e| miette::miette!("malformed mesh `{}`: {e}", path.display()))?;
        out.push((slug, mesh));
    }
    Ok(out)
}

/// Read and parse a single mesh by `slug`, or `None` if it does not exist.
pub(crate) fn read_one(repo_root: &Path, slug: &str) -> Result<Option<MeshFile>> {
    let path = slug_path(repo_root, slug);
    match fs::read_to_string(&path) {
        Ok(text) => {
            let mesh = MeshFile::parse(&text)
                .map_err(|e| miette::miette!("invalid mesh `{slug}`: {e}"))?;
            Ok(Some(mesh))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(miette::miette!("failed to read {}: {e}", path.display())),
    }
}

/// Whether a mesh file exists for `slug`.
pub(crate) fn exists(repo_root: &Path, slug: &str) -> bool {
    slug_path(repo_root, slug).is_file()
}

/// Serialize `mesh` and write it atomically to `.wiki/<slug>`.
///
/// Parent directories are created as needed. The write is atomic: a temp file
/// in the destination directory is written then renamed into place.
pub(crate) fn write(repo_root: &Path, slug: &str, mesh: &MeshFile) -> Result<()> {
    let path = slug_path(repo_root, slug);
    let parent = path
        .parent()
        .ok_or_else(|| miette::miette!("mesh slug `{slug}` has no parent directory"))?;
    fs::create_dir_all(parent)
        .map_err(|e| miette::miette!("failed to create {}: {e}", parent.display()))?;

    let mut tmp = tempfile::NamedTempFile::new_in(parent)
        .map_err(|e| miette::miette!("failed to create temp file in {}: {e}", parent.display()))?;
    use std::io::Write as _;
    tmp.write_all(mesh.serialize().as_bytes())
        .map_err(|e| miette::miette!("failed to write mesh `{slug}`: {e}"))?;
    tmp.persist(&path)
        .map_err(|e| miette::miette!("failed to persist mesh `{slug}`: {e}"))?;
    Ok(())
}

/// Delete the mesh file for `slug`. Missing file is not an error.
pub(crate) fn delete(repo_root: &Path, slug: &str) -> Result<()> {
    let path = slug_path(repo_root, slug);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(miette::miette!("failed to delete mesh `{slug}`: {e}")),
    }
}

/// Read the worktree file at `repo_root/path` and hash the named extent.
///
/// Returns the bare lowercase-hex rk64 fingerprint produced by
/// [`git_mesh_core::cheap_fingerprint_with_extent`] + [`git_mesh_core::rk64_to_hex`]
/// (16 hex digits, no `rk64:` prefix). rk64 is a non-cryptographic identity —
/// sound here because wiki anchors track documentation links, where a rare
/// wrong/missed match is self-correcting, not a data-integrity event.
pub(crate) fn hash_anchor(
    repo_root: &Path,
    path: &str,
    extent: AnchorExtent,
) -> Result<String> {
    let abs = repo_root.join(path);
    let bytes = fs::read(&abs)
        .map_err(|e| miette::miette!("failed to read {}: {e}", abs.display()))?;
    Ok(rk64_to_hex(cheap_fingerprint_with_extent(&bytes, &extent)))
}

/// Build an [`AnchorRecord`] from a path, extent, and bare-hex content hash.
///
/// The algorithm is fixed to [`RK64_ALGORITHM`]. Whole-file anchors use the
/// `start_line == 0 && end_line == 0` sentinel.
pub(crate) fn anchor_record(path: String, extent: AnchorExtent, content_hash: String) -> AnchorRecord {
    let (start_line, end_line) = match extent {
        AnchorExtent::WholeFile => (0, 0),
        AnchorExtent::LineRange { start, end } => (start, end),
    };
    AnchorRecord {
        path,
        start_line,
        end_line,
        algorithm: RK64_ALGORITHM.to_string(),
        content_hash,
    }
}

/// Relocate an anchor in place within the `.wiki/<slug>` mesh file.
///
/// Finds the [`AnchorRecord`] matching the old `(old_path, old_start, old_end)`
/// triple, replaces its path/coordinates with the new location, recomputes its
/// `content_hash` by re-reading the worktree bytes at the new location, and
/// writes the mesh back. The whole-file sentinel (`0/0`) is handled by reading
/// the new extent directly from the new coordinates.
///
/// This is the move-follow writeback: it keeps the stored anchor pointing at the
/// content's current location so it is not re-detected as stale on the next run,
/// and so coverage queries reflect the new range. Returns `Ok(false)` when no
/// matching anchor or mesh is found (nothing relocated).
#[allow(clippy::too_many_arguments)]
pub(crate) fn relocate_anchor(
    repo_root: &Path,
    slug: &str,
    old_path: &str,
    old_start: u32,
    old_end: u32,
    new_path: &str,
    new_start: u32,
    new_end: u32,
) -> Result<bool> {
    let Some(mut mesh) = read_one(repo_root, slug)? else {
        return Ok(false);
    };

    let Some(anchor) = mesh.anchors.iter_mut().find(|a| {
        a.path == old_path && a.start_line == old_start && a.end_line == old_end
    }) else {
        return Ok(false);
    };

    let new_extent = if new_start == 0 && new_end == 0 {
        AnchorExtent::WholeFile
    } else {
        AnchorExtent::LineRange {
            start: new_start,
            end: new_end,
        }
    };
    let new_hash = hash_anchor(repo_root, new_path, new_extent)?;

    anchor.path = new_path.to_string();
    anchor.start_line = new_start;
    anchor.end_line = new_end;
    anchor.content_hash = new_hash;

    write(repo_root, slug, &mesh)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_mesh() -> MeshFile {
        MeshFile {
            anchors: vec![
                anchor_record(
                    "src/foo.rs".to_string(),
                    AnchorExtent::LineRange { start: 2, end: 4 },
                    "deadbeef".to_string(),
                ),
                anchor_record(
                    "src/bar.rs".to_string(),
                    AnchorExtent::WholeFile,
                    "cafef00d".to_string(),
                ),
            ],
            why: "Sample subsystem spanning foo and bar.".to_string(),
        }
    }

    /// Frozen parity vector: a known buffer + line range must fingerprint to
    /// this exact lowercase 16-hex rk64 value. Pins the `git-mesh-core` rk64
    /// canonicalization so future crate drift is caught (acceptance signal #2).
    #[test]
    fn frozen_hash_parity_vector() {
        let buf = b"line one\nline two\nline three\nline four\nline five\n";

        let range = rk64_to_hex(cheap_fingerprint_with_extent(
            buf,
            &AnchorExtent::LineRange { start: 2, end: 4 },
        ));
        assert_eq!(
            range, "f27948ccff0093a1",
            "range 2-4 fingerprint drifted from frozen vector"
        );

        let whole = rk64_to_hex(cheap_fingerprint_with_extent(buf, &AnchorExtent::WholeFile));
        assert_eq!(
            whole, "4616a82389f999f7",
            "whole-file fingerprint drifted from frozen vector"
        );
    }

    #[test]
    fn write_read_one_round_trip_nested_slug() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mesh = sample_mesh();

        assert!(!exists(root, "wiki/foo/bar"));
        write(root, "wiki/foo/bar", &mesh).unwrap();
        assert!(exists(root, "wiki/foo/bar"));

        let read = read_one(root, "wiki/foo/bar").unwrap().unwrap();
        assert_eq!(read, mesh);

        // Nested parent dirs were created under .wiki/.
        assert!(wiki_dir(root).join("wiki").join("foo").is_dir());
    }

    #[test]
    fn read_one_missing_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_one(dir.path(), "nope").unwrap().is_none());
    }

    #[test]
    fn read_all_missing_dir_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_all(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn read_all_recurses_and_yields_slugs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "top", &sample_mesh()).unwrap();
        write(root, "wiki/foo/bar", &sample_mesh()).unwrap();

        let all = read_all(root).unwrap();
        let slugs: Vec<&str> = all.iter().map(|(s, _)| s.as_str()).collect();
        assert!(slugs.contains(&"top"), "missing top slug: {slugs:?}");
        assert!(slugs.contains(&"wiki/foo/bar"), "missing nested slug: {slugs:?}");
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn delete_removes_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "gone", &sample_mesh()).unwrap();
        assert!(exists(root, "gone"));
        delete(root, "gone").unwrap();
        assert!(!exists(root, "gone"));
        // Deleting a missing slug is not an error.
        delete(root, "gone").unwrap();
    }

    #[test]
    fn hash_anchor_reads_worktree_and_matches_core() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let content = b"alpha\nbravo\ncharlie\n";
        fs::write(root.join("file.txt"), content).unwrap();

        let got = hash_anchor(root, "file.txt", AnchorExtent::WholeFile).unwrap();
        let expected = rk64_to_hex(cheap_fingerprint_with_extent(content, &AnchorExtent::WholeFile));
        assert_eq!(got, expected);
    }

    #[test]
    fn read_all_skips_stray_non_mesh_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        // Write a valid mesh.
        write(root, "goodslug", &sample_mesh()).unwrap();

        // Seed dotfiles with junk — these must be silently skipped, not errored.
        let wiki = wiki_dir(root);
        fs::write(wiki.join(".DS_Store"), b"\x00\x01\x02\xff\xfe binary junk").unwrap();
        fs::write(wiki.join(".foo.swp"), b"editor swap content").unwrap();

        // read_all must return only the valid mesh and must not error.
        let all = read_all(root).unwrap();
        assert_eq!(all.len(), 1, "expected only the valid mesh, got: {all:?}");
        assert_eq!(all[0].0, "goodslug");
    }

    #[test]
    fn read_all_skips_runtime_artifacts() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        write(root, "goodslug", &sample_mesh()).unwrap();

        // The index cache DB, its SQLite sidecars, the refresh lock, and the
        // perf log coexist under `.wiki/` but must never be parsed as meshes —
        // even though they are not dotfiles and the DB is binary.
        let wiki = wiki_dir(root);
        fs::write(wiki.join("wiki-index.sqlite"), b"SQLite format 3\x00\xff\xfe").unwrap();
        fs::write(wiki.join("wiki-index.sqlite-wal"), b"\x00\x01wal").unwrap();
        fs::write(wiki.join("wiki-index.sqlite-shm"), b"\x00\x01shm").unwrap();
        fs::write(wiki.join("wiki-refresh.lock"), b"").unwrap();
        fs::write(wiki.join("wiki.log"), b"{\"event\":\"x\"}\n").unwrap();

        let all = read_all(root).unwrap();
        assert_eq!(all.len(), 1, "expected only the valid mesh, got: {all:?}");
        assert_eq!(all[0].0, "goodslug");
    }

    #[test]
    fn read_all_fails_closed_on_conflict_markers() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        // Write a valid mesh alongside the corrupt one.
        write(root, "goodslug", &sample_mesh()).unwrap();

        // Seed a non-dotfile with git conflict markers — must error, not skip.
        let wiki = wiki_dir(root);
        fs::write(
            wiki.join("conflicted"),
            b"<<<<<<< HEAD\nfoo\n=======\nbar\n>>>>>>> other\n",
        )
        .unwrap();

        let result = read_all(root);
        assert!(
            result.is_err(),
            "read_all must fail closed on a conflict-markered mesh file"
        );
        let msg = format!("{:?}", result.unwrap_err());
        assert!(
            msg.contains("conflicted"),
            "error message must name the offending file: {msg}"
        );
    }

    #[test]
    fn read_all_fails_closed_on_non_dot_prose_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        write(root, "goodslug", &sample_mesh()).unwrap();

        // A non-dot file with arbitrary prose (not a mesh) must also error.
        let wiki = wiki_dir(root);
        fs::write(wiki.join("README"), b"This is documentation, not a mesh.\n").unwrap();

        let result = read_all(root);
        assert!(
            result.is_err(),
            "read_all must fail closed on a non-dot unparseable file"
        );
        let msg = format!("{:?}", result.unwrap_err());
        assert!(
            msg.contains("README"),
            "error message must name the offending file: {msg}"
        );
    }

    #[test]
    fn relocate_anchor_in_place_refreshes_hash_no_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        // Seed source + mesh anchoring line 1.
        fs::write(root.join("code.rs"), b"fn a() {}\n").unwrap();
        let h1 = hash_anchor(root, "code.rs", AnchorExtent::LineRange { start: 1, end: 1 }).unwrap();
        write(
            root,
            "m",
            &MeshFile {
                anchors: vec![anchor_record(
                    "code.rs".to_string(),
                    AnchorExtent::LineRange { start: 1, end: 1 },
                    h1,
                )],
                why: "w.".to_string(),
            },
        )
        .unwrap();

        // The block drifts to line 2.
        fs::write(root.join("code.rs"), b"// pre\nfn a() {}\n").unwrap();

        let moved = relocate_anchor(root, "m", "code.rs", 1, 1, "code.rs", 2, 2).unwrap();
        assert!(moved);

        let mesh = read_one(root, "m").unwrap().unwrap();
        assert_eq!(mesh.anchors.len(), 1, "must relocate in place, not append");
        let a = &mesh.anchors[0];
        assert_eq!((a.start_line, a.end_line), (2, 2));
        let expected = hash_anchor(root, "code.rs", AnchorExtent::LineRange { start: 2, end: 2 }).unwrap();
        assert_eq!(a.content_hash, expected, "hash must match the NEW range");
    }

    #[test]
    fn relocate_anchor_cross_file_and_whole_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("a.rs"), b"x\n").unwrap();
        fs::write(root.join("b.rs"), b"y\nx\n").unwrap();
        write(
            root,
            "m",
            &MeshFile {
                anchors: vec![anchor_record(
                    "a.rs".to_string(),
                    AnchorExtent::WholeFile,
                    "stale".to_string(),
                )],
                why: "w.".to_string(),
            },
        )
        .unwrap();

        // Whole-file anchor relocates to b.rs whole-file (0/0 handled).
        let moved = relocate_anchor(root, "m", "a.rs", 0, 0, "b.rs", 0, 0).unwrap();
        assert!(moved);
        let mesh = read_one(root, "m").unwrap().unwrap();
        assert_eq!(mesh.anchors.len(), 1);
        let a = &mesh.anchors[0];
        assert_eq!(a.path, "b.rs");
        assert_eq!((a.start_line, a.end_line), (0, 0));
        let expected = hash_anchor(root, "b.rs", AnchorExtent::WholeFile).unwrap();
        assert_eq!(a.content_hash, expected);
    }

    #[test]
    fn relocate_anchor_no_match_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("a.rs"), b"x\n").unwrap();
        write(root, "m", &sample_mesh()).unwrap();
        // No anchor matches (1,1) in foo.rs (sample has 2-4).
        let moved = relocate_anchor(root, "m", "src/foo.rs", 1, 1, "src/foo.rs", 9, 9).unwrap();
        assert!(!moved);
        // Missing mesh.
        let moved = relocate_anchor(root, "absent", "p", 1, 1, "p", 2, 2).unwrap();
        assert!(!moved);
    }

    #[test]
    fn anchor_record_whole_file_uses_zero_sentinel() {
        let rec = anchor_record("p".to_string(), AnchorExtent::WholeFile, "h".to_string());
        assert_eq!((rec.start_line, rec.end_line), (0, 0));
        assert_eq!(rec.algorithm, "rk64");
    }
}
