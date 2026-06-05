//! In-process `.wiki/<slug>` mesh storage.
//!
//! Owns the storage and hashing primitives that replace the `git mesh` binary
//! shell-outs. Anchors live under `repo_root/.wiki/<slug>` as plain text in the
//! `git-mesh-core` [`MeshFile`] format (byte-identical to the legacy `.mesh/`
//! format). Every hash flows through [`git_mesh_core::hash_bytes_with_extent`]
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
use git_mesh_core::{AnchorExtent, hash_bytes_with_extent};

/// Algorithm name recorded in every [`AnchorRecord`] written by wiki.
const SHA256: &str = "sha256";

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
        let Some(slug) = slug_for(&root, path) else {
            continue;
        };
        let text = fs::read_to_string(path)
            .map_err(|e| miette::miette!("failed to read {}: {e}", path.display()))?;
        let mesh = MeshFile::parse(&text)
            .map_err(|e| miette::miette!("invalid mesh `{slug}`: {e}"))?;
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
/// Returns the bare lowercase-hex SHA-256 produced by
/// [`git_mesh_core::hash_bytes_with_extent`] (no `sha256:` prefix).
pub(crate) fn hash_anchor(
    repo_root: &Path,
    path: &str,
    extent: AnchorExtent,
) -> Result<String> {
    let abs = repo_root.join(path);
    let bytes = fs::read(&abs)
        .map_err(|e| miette::miette!("failed to read {}: {e}", abs.display()))?;
    Ok(hash_bytes_with_extent(&bytes, &extent))
}

/// Build an [`AnchorRecord`] from a path, extent, and bare-hex content hash.
///
/// The algorithm is fixed to `"sha256"`. Whole-file anchors use the
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
        algorithm: SHA256.to_string(),
        content_hash,
    }
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

    /// Frozen parity vector: a known buffer + line range must hash to this
    /// exact lowercase-hex digest. Pins the `git-mesh-core` hash contract so
    /// future crate drift is caught (acceptance signal #2).
    #[test]
    fn frozen_hash_parity_vector() {
        let buf = b"line one\nline two\nline three\nline four\nline five\n";

        let range = hash_bytes_with_extent(buf, &AnchorExtent::LineRange { start: 2, end: 4 });
        assert_eq!(
            range,
            "d0c948cc8b26ad880ae92259ebc2524dc21dee3116718adb59eae0828678f896",
            "range 2-4 hash drifted from frozen vector"
        );

        let whole = hash_bytes_with_extent(buf, &AnchorExtent::WholeFile);
        assert_eq!(
            whole,
            "1b0deaa0ac952c6dcc836234ce4270a8f0dba9e12f5ec4cf1d65108168a17843",
            "whole-file hash drifted from frozen vector"
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
        let expected = hash_bytes_with_extent(content, &AnchorExtent::WholeFile);
        assert_eq!(got, expected);
    }

    #[test]
    fn anchor_record_whole_file_uses_zero_sentinel() {
        let rec = anchor_record("p".to_string(), AnchorExtent::WholeFile, "h".to_string());
        assert_eq!((rec.start_line, rec.end_line), (0, 0));
        assert_eq!(rec.algorithm, "sha256");
    }
}
