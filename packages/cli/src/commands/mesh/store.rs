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

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use miette::Result;
use walkdir::WalkDir;

use git_mesh_core::mesh_file::{AnchorRecord, MeshFile, has_conflict_markers};
use git_mesh_core::{AnchorExtent, RK64_ALGORITHM, cheap_fingerprint_with_extent, rk64_to_hex};

/// Per-run file-content cache that avoids re-reading the same file when
/// multiple anchors reference the same target. Keyed by absolute path.
///
/// A file referenced by K anchors is read once — not up to 2K times — during
/// mesh validation (bounds checking) and hashing.
pub(crate) struct FileContentCache {
    entries: HashMap<PathBuf, CachedFile>,
}

pub(crate) struct CachedFile {
    /// Raw file bytes. Hashing ([`cheap_fingerprint_with_extent`]) operates on
    /// these directly. Callers that need line counting use [`CachedFile::utf8`].
    bytes: Vec<u8>,
}

impl CachedFile {
    /// Validate and return the cached content as `&str`.
    ///
    /// Returns an error when the content is not valid UTF-8. Hashing via
    /// [`cheap_fingerprint_with_extent`] works on [`CachedFile::bytes`]
    /// regardless — this accessor exists only for line counting and bounds
    /// validation, which require text.
    pub(crate) fn utf8(&self) -> Result<&str> {
        std::str::from_utf8(&self.bytes).map_err(|e| {
            miette::miette!("cached file content is not valid UTF-8: {e}")
        })
    }
}

impl FileContentCache {
    pub(crate) fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Get cached content, reading from disk on first access.
    ///
    /// The returned [`CachedFile`] stores only raw bytes — no UTF-8 validation
    /// happens here. Callers that need line counting call [`CachedFile::utf8`].
    pub(crate) fn get_or_read(&mut self, abs_path: &Path) -> Result<&CachedFile> {
        Ok(match self.entries.entry(abs_path.to_path_buf()) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => {
                let bytes = fs::read(abs_path).map_err(|err| {
                    miette::miette!("failed to read {}: {err}", abs_path.display())
                })?;
                e.insert(CachedFile { bytes })
            }
        })
    }

    /// Insert pre-read content into the cache. Used when content was read from
    /// a non-filesystem source (e.g. git objects) so subsequent
    /// [`get_or_read`] calls find it.
    pub(crate) fn insert(&mut self, abs_path: PathBuf, bytes: Vec<u8>) {
        self.entries
            .entry(abs_path)
            .or_insert(CachedFile { bytes });
    }
}

/// The fixed mesh storage directory: `repo_root/.wiki`.
pub(crate) fn wiki_dir(repo_root: &Path) -> PathBuf {
    repo_root.join(".wiki")
}

/// Write `.wiki/.gitattributes` with `* text eol=lf` if missing or stale.
///
/// Mirrors `git_mesh::mesh::structural::ensure_mesh_dir` for the `.wiki/` store:
/// idempotent — writes only when the file is absent or its content differs from
/// the canonical form. This guarantees mesh files check out with LF line endings
/// on all platforms without per-developer `core.autocrlf` configuration, so rk64
/// anchor hashes mean the same thing everywhere.
fn ensure_wiki_dir(repo_root: &Path) -> Result<()> {
    let wiki = wiki_dir(repo_root);
    let ga_path = wiki.join(".gitattributes");
    let canonical = "* text eol=lf\n";
    let current = std::fs::read_to_string(&ga_path).unwrap_or_default();
    if current != canonical {
        std::fs::write(&ga_path, canonical)
            .map_err(|e| miette::miette!("failed to write {}: {e}", ga_path.display()))?;
    }
    Ok(())
}

/// Resolve the on-disk path of the mesh file for `slug`.
pub(crate) fn slug_path(repo_root: &Path, slug: &str) -> PathBuf {
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
///
/// Fail-closes on any file that cannot be parsed (e.g. git conflict markers).
/// Production code now uses [`read_all_tolerant`] instead; this strict variant
/// is retained for unit tests that verify the fail-closed contract.
#[allow(dead_code)]
pub(crate) fn read_all(repo_root: &Path) -> Result<Vec<(String, MeshFile)>> {
    let root = wiki_dir(repo_root);
    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut out = Vec::new();
    for entry in WalkDir::new(&root).sort_by_file_name() {
        let entry = entry.map_err(|e| miette::miette!("failed to walk {}: {e}", root.display()))?;
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
///
/// Anchors are written in canonical order: sorted by `(path, start_line,
/// end_line)` so that every write produces a deterministic serialization
/// regardless of the order in which anchors were added programmatically.
pub(crate) fn write(repo_root: &Path, slug: &str, mesh: &MeshFile) -> Result<()> {
    let path = slug_path(repo_root, slug);
    let parent = path
        .parent()
        .ok_or_else(|| miette::miette!("mesh slug `{slug}` has no parent directory"))?;
    fs::create_dir_all(parent)
        .map_err(|e| miette::miette!("failed to create {}: {e}", parent.display()))?;
    ensure_wiki_dir(repo_root)?;

    let mut tmp = tempfile::NamedTempFile::new_in(parent)
        .map_err(|e| miette::miette!("failed to create temp file in {}: {e}", parent.display()))?;
    use std::io::Write as _;

    // Canonical ordering: sort by (path, start_line, end_line) before serializing.
    let mut sorted = mesh.clone();
    sorted.anchors.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then(a.start_line.cmp(&b.start_line))
            .then(a.end_line.cmp(&b.end_line))
    });

    tmp.write_all(sorted.serialize().as_bytes())
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
/// Uses `cache` to avoid re-reading a file already read earlier in the same
/// run — `get_or_read` hits disk only on the first access for each path.
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
    cache: &mut FileContentCache,
) -> Result<String> {
    let abs = repo_root.join(path);
    let cached = cache.get_or_read(&abs)?;
    Ok(rk64_to_hex(cheap_fingerprint_with_extent(
        &cached.bytes,
        &extent,
    )))
}

/// Read all meshes, skipping files that fail to parse (e.g. due to git conflict
/// markers). Returns the parsed meshes alongside a list of skipped slugs so the
/// caller can print a single consolidated warning.
///
/// Skips dotfiles and runtime artifacts (same filter as [`read_all`]). A missing
/// `.wiki/` directory returns an empty vector and an empty skip list.
#[allow(clippy::type_complexity)]
pub(crate) fn read_all_tolerant(
    repo_root: &Path,
) -> Result<(Vec<(String, MeshFile)>, Vec<String>)> {
    let root = wiki_dir(repo_root);
    if !root.exists() {
        return Ok((Vec::new(), Vec::new()));
    }

    let mut out = Vec::new();
    let mut skipped = Vec::new();
    for entry in WalkDir::new(&root).sort_by_file_name() {
        let entry = entry.map_err(|e| miette::miette!("failed to walk {}: {e}", root.display()))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
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
        let text = match fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                skipped.push(format!(
                    "failed to read mesh `{}`: {e}",
                    path.display()
                ));
                continue;
            }
        };
        match MeshFile::parse(&text) {
            Ok(mesh) => out.push((slug, mesh)),
            Err(e) => {
                skipped.push(format!(
                    "skipping unparseable mesh `{slug}`: {e}"
                ));
            }
        }
    }
    Ok((out, skipped))
}

/// Walk `.wiki/` and return `(slug, raw_text)` for every mesh file whose content
/// contains git conflict markers.
///
/// Skips dotfiles and runtime artifacts (same filter as [`read_all`]). A missing
/// `.wiki/` directory returns an empty `Vec` (not an error).
pub(crate) fn find_conflicted_meshes(repo_root: &Path) -> Result<Vec<(String, String)>> {
    let root = wiki_dir(repo_root);
    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut out = Vec::new();
    for entry in WalkDir::new(&root).sort_by_file_name() {
        let entry =
            entry.map_err(|e| miette::miette!("failed to walk {}: {e}", root.display()))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
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
        let text = fs::read_to_string(path)
            .map_err(|e| miette::miette!("failed to read mesh `{}`: {e}", path.display()))?;
        if has_conflict_markers(&text) {
            out.push((slug, text));
        }
    }
    Ok(out)
}

/// Build an [`AnchorRecord`] from a path, extent, and bare-hex content hash.
///
/// The algorithm is fixed to [`RK64_ALGORITHM`]. Whole-file anchors use the
/// `start_line == 0 && end_line == 0` sentinel.
pub(crate) fn anchor_record(
    path: String,
    extent: AnchorExtent,
    content_hash: String,
) -> AnchorRecord {
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

/// The outcome of [`upsert_anchor`]: distinguishes first-create from extend/refresh.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) enum UpsertOutcome {
    /// A new mesh file was created with this anchor as its sole entry.
    Created,
    /// The anchor was added to an existing mesh that did not contain it.
    Extended,
    /// An existing anchor with the same `(path, start, end)` triple had its
    /// hash refreshed in place (no duplicate was added).
    Refreshed,
}

/// The result of applying one or more anchors / a rationale update to a mesh.
///
/// `why_overwritten` carries the *previous* non-empty rationale when `--why`
/// replaced an existing curated rationale, so the caller can surface an explicit
/// notice rather than silently clobbering it (F4). It is `None` when no overwrite
/// occurred (mesh created, `--why` omitted, or the new rationale is identical).
#[derive(Debug)]
pub(crate) struct ApplyOutcome {
    /// Per-anchor outcome, in the same order as the input anchors.
    pub(crate) anchors: Vec<(String, UpsertOutcome)>,
    /// Previous rationale, when `--why` overwrote a non-empty existing one.
    pub(crate) why_overwritten: Option<String>,
    /// True when this invocation only updated the rationale (no anchors).
    pub(crate) why_only: bool,
}

/// Insert or update a single anchor in the named mesh.
///
/// Thin convenience wrapper over [`upsert_anchors`] for the one-anchor case
/// (preserves the `why`/bounds/upsert semantics documented there).
#[cfg(test)]
pub(crate) fn upsert_anchor(
    repo_root: &Path,
    slug: &str,
    why: Option<&str>,
    path: &str,
    extent: AnchorExtent,
) -> Result<UpsertOutcome> {
    let label = match extent {
        AnchorExtent::WholeFile => path.to_string(),
        AnchorExtent::LineRange { start, end } => format!("{path}#L{start}-L{end}"),
    };
    let outcome = upsert_anchors(repo_root, slug, why, &[(path.to_string(), extent, label)])?;
    Ok(outcome.anchors[0].1)
}

/// Atomically upsert a batch of anchors (and/or a rationale) into one mesh.
///
/// Every anchor is parsed, bounds-checked, and hashed BEFORE any write, so a
/// single malformed/out-of-bounds anchor leaves the store untouched (F5: no
/// partial state). The label paired with each outcome is the caller-supplied
/// human label for that anchor.
///
/// `why` handling (F4/F6):
/// - When the mesh does not exist, `why` **must** be `Some(..)` (fail closed),
///   and at least one anchor is required (there is nothing to anchor otherwise).
/// - An empty `anchors` slice is a rationale-only update: the mesh must already
///   exist and `why` must be `Some(..)`; otherwise it is an error.
/// - When `why = Some(new)` replaces an existing non-empty rationale that
///   differs, the previous rationale is returned in
///   [`ApplyOutcome::why_overwritten`] so the caller can warn.
pub(crate) fn upsert_anchors(
    repo_root: &Path,
    slug: &str,
    why: Option<&str>,
    anchors: &[(String, AnchorExtent, String)],
) -> Result<ApplyOutcome> {
    let existing = read_one(repo_root, slug)?;

    // Rationale-only update (no anchors): mesh must exist; --why required.
    if anchors.is_empty() {
        let Some(mut mesh) = existing else {
            return Err(miette::miette!(
                "mesh `{slug}` does not exist; nothing to curate \
                 (provide at least one anchor to create it)"
            ));
        };
        let new_why = why.ok_or_else(|| {
            miette::miette!(
                "mesh `{slug}` already exists; provide an anchor to extend it \
                 or --why <text> to update its rationale"
            )
        })?;
        let prev = mesh.why.clone();
        let why_overwritten = (!prev.is_empty() && prev != new_why).then_some(prev);
        mesh.why = new_why.to_string();
        write(repo_root, slug, &mesh)?;
        return Ok(ApplyOutcome {
            anchors: Vec::new(),
            why_overwritten,
            why_only: true,
        });
    }

    // Determine the rationale up front so a missing --why on create fails before
    // any bounds work or writes.
    let (mesh_why, why_overwritten) = match &existing {
        None => {
            let w = why.ok_or_else(|| {
                miette::miette!(
                    "mesh `{slug}` does not exist; --why <text> is required when creating a new mesh"
                )
            })?;
            (w.to_string(), None)
        }
        Some(existing_mesh) => match why {
            Some(w) => {
                let prev = existing_mesh.why.clone();
                let overwritten = (!prev.is_empty() && prev != w).then_some(prev);
                (w.to_string(), overwritten)
            }
            None => (existing_mesh.why.clone(), None),
        },
    };

    // Phase 1: validate + hash every anchor. Any failure aborts before writing.
    // A per-run cache avoids re-reading the same file for bounds + hashing when
    // multiple anchors reference the same target.
    let mut cache = FileContentCache::new();
    let mut records: Vec<(AnchorRecord, String)> = Vec::with_capacity(anchors.len());
    for (path, extent, label) in anchors {
        if let AnchorExtent::LineRange { start, end } = *extent {
            let abs = repo_root.join(path);
            let cached = cache.get_or_read(&abs)?;
            let line_count = cached.utf8()?.lines().count() as u32;
            if end > line_count {
                return Err(miette::miette!(
                    "anchor `{path}#L{start}-L{end}` is out of bounds: file has {line_count} line(s)"
                ));
            }
        }
        let content_hash = hash_anchor(repo_root, path, *extent, &mut cache)?;
        let record = anchor_record(path.clone(), *extent, content_hash);
        records.push((record, label.clone()));
    }

    // Phase 2: apply all validated records to an in-memory mesh, then write once.
    let mesh_created = existing.is_none();
    let mut mesh = existing.unwrap_or_else(|| MeshFile {
        anchors: Vec::new(),
        why: String::new(),
    });
    mesh.why = mesh_why;

    let mut outcomes = Vec::with_capacity(records.len());
    for (record, label) in records {
        let pos = mesh.anchors.iter().position(|a| {
            a.path == record.path
                && a.start_line == record.start_line
                && a.end_line == record.end_line
        });
        let outcome = match pos {
            Some(i) => {
                mesh.anchors[i] = record;
                UpsertOutcome::Refreshed
            }
            None => {
                mesh.anchors.push(record);
                if mesh_created && outcomes.is_empty() {
                    UpsertOutcome::Created
                } else {
                    UpsertOutcome::Extended
                }
            }
        };
        outcomes.push((label, outcome));
    }

    write(repo_root, slug, &mesh)?;
    Ok(ApplyOutcome {
        anchors: outcomes,
        why_overwritten,
        why_only: false,
    })
}

/// Remove a single anchor from the named mesh.
///
/// Finds the [`AnchorRecord`] matching `(path, start_line, end_line)` and
/// removes it. If the mesh becomes empty after removal the mesh file is
/// deleted. Returns `true` when an anchor was removed, `false` when no
/// matching anchor or mesh was found.
pub(crate) fn remove_anchor(
    repo_root: &Path,
    slug: &str,
    path: &str,
    start: u32,
    end: u32,
) -> Result<bool> {
    let Some(mut mesh) = read_one(repo_root, slug)? else {
        return Ok(false);
    };

    let before = mesh.anchors.len();
    mesh.anchors
        .retain(|a| !(a.path == path && a.start_line == start && a.end_line == end));

    if mesh.anchors.len() == before {
        // No anchor matched.
        return Ok(false);
    }

    if mesh.anchors.is_empty() {
        delete(repo_root, slug)?;
    } else {
        write(repo_root, slug, &mesh)?;
    }
    Ok(true)
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

    let Some(anchor) = mesh
        .anchors
        .iter_mut()
        .find(|a| a.path == old_path && a.start_line == old_start && a.end_line == old_end)
    else {
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
    let new_hash = hash_anchor(repo_root, new_path, new_extent, &mut FileContentCache::new())?;

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
        // Anchors are in canonical (path, start_line, end_line) order so the
        // write+read_one round-trip test passes: store::write sorts before
        // serializing.
        MeshFile {
            anchors: vec![
                anchor_record(
                    "src/bar.rs".to_string(),
                    AnchorExtent::WholeFile,
                    "cafef00d".to_string(),
                ),
                anchor_record(
                    "src/foo.rs".to_string(),
                    AnchorExtent::LineRange { start: 2, end: 4 },
                    "deadbeef".to_string(),
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
        assert!(
            slugs.contains(&"wiki/foo/bar"),
            "missing nested slug: {slugs:?}"
        );
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

        let got = hash_anchor(
            root,
            "file.txt",
            AnchorExtent::WholeFile,
            &mut FileContentCache::new(),
        )
        .unwrap();
        let expected = rk64_to_hex(cheap_fingerprint_with_extent(
            content,
            &AnchorExtent::WholeFile,
        ));
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
        fs::write(
            wiki.join("wiki-index.sqlite"),
            b"SQLite format 3\x00\xff\xfe",
        )
        .unwrap();
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
    fn find_conflicted_meshes_returns_conflicted_slug_and_raw_text() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        // Write a clean mesh (no conflict markers).
        write(root, "goodslug", &sample_mesh()).unwrap();

        // Write a mesh with git conflict markers.
        let wiki = wiki_dir(root);
        let conflicted_text = "<<<<<<< HEAD\nsrc/a.rs rk64:abc123\n=======\nsrc/a.rs rk64:def456\n>>>>>>> other\n\nwhy";
        fs::write(wiki.join("conflicted"), conflicted_text.as_bytes()).unwrap();

        // find_conflicted_meshes must return only the conflicted file.
        let results = find_conflicted_meshes(root).unwrap();
        assert_eq!(results.len(), 1, "expected exactly one conflicted mesh");
        assert_eq!(results[0].0, "conflicted");
        assert_eq!(results[0].1, conflicted_text);
    }

    #[test]
    fn find_conflicted_meshes_missing_dir_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let results = find_conflicted_meshes(dir.path()).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn find_conflicted_meshes_skips_clean_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "clean1", &sample_mesh()).unwrap();
        write(root, "clean2", &sample_mesh()).unwrap();
        let results = find_conflicted_meshes(root).unwrap();
        assert!(results.is_empty(), "expected no conflicted meshes");
    }

    #[test]
    fn find_conflicted_meshes_skips_dotfiles_and_runtime_artifacts() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let wiki = wiki_dir(root);

        // Create the .wiki directory.
        fs::create_dir_all(&wiki).unwrap();

        // Write a runtime artifact with marker-looking content — must be skipped.
        fs::write(wiki.join("wiki-index.sqlite"), b"<<<<<<< HEAD\nsqlite junk\n=======\nmore junk\n>>>>>>> other\n").unwrap();
        fs::write(wiki.join(".DS_Store"), b"<<<<<<< HEAD\n").unwrap();

        let results = find_conflicted_meshes(root).unwrap();
        assert!(results.is_empty(), "expected no conflicted meshes from dotfiles/artifacts");
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
        let h1 = hash_anchor(
            root,
            "code.rs",
            AnchorExtent::LineRange { start: 1, end: 1 },
            &mut FileContentCache::new(),
        )
        .unwrap();
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
        let expected = hash_anchor(
            root,
            "code.rs",
            AnchorExtent::LineRange { start: 2, end: 2 },
            &mut FileContentCache::new(),
        )
        .unwrap();
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
        let expected = hash_anchor(
            root,
            "b.rs",
            AnchorExtent::WholeFile,
            &mut FileContentCache::new(),
        )
        .unwrap();
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

    // ── upsert_anchor tests ───────────────────────────────────────────────────

    #[test]
    fn upsert_anchor_creates_new_mesh_with_why() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("a.rs"), b"fn main() {}\n").unwrap();

        let outcome = upsert_anchor(
            root,
            "my/slug",
            Some("The auth subsystem."),
            "a.rs",
            AnchorExtent::WholeFile,
        )
        .unwrap();
        assert_eq!(outcome, UpsertOutcome::Created);

        let mesh = read_one(root, "my/slug").unwrap().unwrap();
        assert_eq!(mesh.why, "The auth subsystem.");
        assert_eq!(mesh.anchors.len(), 1);
        assert_eq!(mesh.anchors[0].path, "a.rs");
    }

    #[test]
    fn upsert_anchor_extends_existing_mesh() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("a.rs"), b"fn a() {}\n").unwrap();
        fs::write(root.join("b.rs"), b"fn b() {}\n").unwrap();

        upsert_anchor(root, "sl", Some("Why."), "a.rs", AnchorExtent::WholeFile).unwrap();
        let outcome = upsert_anchor(root, "sl", None, "b.rs", AnchorExtent::WholeFile).unwrap();
        assert_eq!(outcome, UpsertOutcome::Extended);

        let mesh = read_one(root, "sl").unwrap().unwrap();
        assert_eq!(mesh.anchors.len(), 2);
        // why preserved.
        assert_eq!(mesh.why, "Why.");
    }

    #[test]
    fn upsert_anchor_refreshes_existing_anchor_no_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("a.rs"), b"fn a() {}\nfn b() {}\nfn c() {}\n").unwrap();

        upsert_anchor(
            root,
            "sl",
            Some("Why."),
            "a.rs",
            AnchorExtent::LineRange { start: 1, end: 2 },
        )
        .unwrap();

        // Rewrite the file (content changes, range unchanged).
        fs::write(root.join("a.rs"), b"fn x() {}\nfn y() {}\nfn z() {}\n").unwrap();

        let outcome = upsert_anchor(
            root,
            "sl",
            None,
            "a.rs",
            AnchorExtent::LineRange { start: 1, end: 2 },
        )
        .unwrap();
        assert_eq!(outcome, UpsertOutcome::Refreshed);

        let mesh = read_one(root, "sl").unwrap().unwrap();
        assert_eq!(mesh.anchors.len(), 1, "must refresh in place, not append");
        let expected_hash = hash_anchor(
            root,
            "a.rs",
            AnchorExtent::LineRange { start: 1, end: 2 },
            &mut FileContentCache::new(),
        )
        .unwrap();
        assert_eq!(mesh.anchors[0].content_hash, expected_hash);
    }

    #[test]
    fn upsert_anchor_fails_closed_without_why_on_create() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("a.rs"), b"fn a() {}\n").unwrap();

        let result = upsert_anchor(root, "sl", None, "a.rs", AnchorExtent::WholeFile);
        assert!(
            result.is_err(),
            "must fail closed when why is absent on first create"
        );
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("--why"), "error must mention --why: {msg}");
    }

    #[test]
    fn upsert_anchor_rejects_out_of_bounds_line_range() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // File has 3 lines.
        fs::write(root.join("a.rs"), b"fn a() {}\nfn b() {}\nfn c() {}\n").unwrap();

        let result = upsert_anchor(
            root,
            "sl",
            Some("Why."),
            "a.rs",
            AnchorExtent::LineRange { start: 2, end: 5 }, // out of bounds
        );
        assert!(result.is_err(), "must fail closed on out-of-bounds range");
        let msg = format!("{:?}", result.unwrap_err());
        assert!(
            msg.contains("out of bounds"),
            "error must say out of bounds: {msg}"
        );
    }

    // ── remove_anchor tests ───────────────────────────────────────────────────

    #[test]
    fn remove_anchor_drops_one_anchor() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("a.rs"), b"x\n").unwrap();
        fs::write(root.join("b.rs"), b"y\n").unwrap();

        upsert_anchor(root, "sl", Some("Why."), "a.rs", AnchorExtent::WholeFile).unwrap();
        upsert_anchor(root, "sl", None, "b.rs", AnchorExtent::WholeFile).unwrap();

        let removed = remove_anchor(root, "sl", "a.rs", 0, 0).unwrap();
        assert!(removed);

        let mesh = read_one(root, "sl").unwrap().unwrap();
        assert_eq!(mesh.anchors.len(), 1);
        assert_eq!(mesh.anchors[0].path, "b.rs");
    }

    #[test]
    fn remove_anchor_deletes_mesh_file_when_last_anchor_removed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("a.rs"), b"x\n").unwrap();

        upsert_anchor(root, "sl", Some("Why."), "a.rs", AnchorExtent::WholeFile).unwrap();
        assert!(exists(root, "sl"));

        let removed = remove_anchor(root, "sl", "a.rs", 0, 0).unwrap();
        assert!(removed);
        assert!(!exists(root, "sl"), "mesh file must be deleted when empty");
    }

    #[test]
    fn remove_anchor_returns_false_for_no_match() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("a.rs"), b"x\n").unwrap();

        upsert_anchor(root, "sl", Some("Why."), "a.rs", AnchorExtent::WholeFile).unwrap();

        // Wrong anchor (line range instead of whole file).
        let removed = remove_anchor(root, "sl", "a.rs", 1, 1).unwrap();
        assert!(!removed, "must return false for a non-matching anchor");

        // Missing mesh.
        let removed = remove_anchor(root, "no-such-slug", "a.rs", 0, 0).unwrap();
        assert!(!removed, "must return false for a missing mesh");
    }
}
