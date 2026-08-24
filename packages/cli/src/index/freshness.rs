//! Canonical freshness inputs: HEAD oid, `.git/index` trailer, and the
//! worktree `(path, mtime_ns)` walk feeding [`crate::index::generations::
//! worktree_signature`]. The gate hashes these into a
//! `StateFingerprint` and looks the digest up in the merged store — no
//! state row, no stored triple comparison.

use std::fs;
use std::io::Read;
use std::path::Path;

use crate::index::generations::{
    StateFingerprint, UNBORN_HEAD_OID, ZERO_INDEX_CHECKSUM, worktree_signature,
};

/// Compute the canonical fingerprint of the repository's current state, or
/// `None` when a leg is unreadable — which the gate treats as a miss
/// (fail-open toward rehash). `repo` may be supplied by callers that
/// already hold an open `gix::Repository` (the refresh path); otherwise one
/// is opened for the head-tree leg.
///
/// The result must be byte-stable across call sites: whatever this returns
/// on unchanged state is exactly what the refresh path publishes.
pub(crate) fn current_fingerprint(
    repo_root: &Path,
    dot_git: &Path,
    repo: Option<&gix::Repository>,
    wikiignore_hash: &[u8; 20],
) -> anyhow::Result<Option<StateFingerprint>> {
    // HEAD oid: file-based resolution matches the refresh path's gix
    // emission byte-for-byte, including the unborn-HEAD sentinel.
    let Some(head_oid) = read_head_oid(dot_git) else {
        return Ok(None);
    };

    let Some(index_checksum) = read_index_trailer(&dot_git.join("index")) else {
        return Ok(None);
    };

    // Head tree: peel HEAD through gix (an unborn HEAD diffs against the
    // empty tree). A failed open degrades to a gate miss rather than an
    // error — consistent with every other unreadable-input leg.
    let head_tree_oid = match repo {
        Some(repo) => repo.head_tree_id().ok().map(|id| id.to_hex().to_string()),
        None => match gix::open(repo_root) {
            Ok(opened) => opened.head_tree_id().ok().map(|id| id.to_hex().to_string()),
            Err(_) => return Ok(None),
        },
    }
    .unwrap_or_default(); // unborn HEAD diffs against the empty tree

    let mut wikiignore = [0u8; 20];
    wikiignore.copy_from_slice(wikiignore_hash);

    let pairs = collect_worktree_pairs(repo_root);
    Ok(Some(StateFingerprint {
        head_oid,
        head_tree_oid,
        index_checksum,
        wikiignore_hash: wikiignore,
        worktree_sig: worktree_signature(&pairs),
    }))
}

/// The publish-side twin of [`current_fingerprint`]: same canonical inputs,
/// but every leg falls back to its sentinel instead of degrading — a fresh
/// clone may have no `.git/index` yet and an unborn HEAD has no tree, and
/// the refresh must still publish a generation for that state.
pub(crate) fn published_fingerprint(
    repo: &gix::Repository,
    repo_root: &Path,
    dot_git: &Path,
    wikiignore_hash: &[u8; 20],
) -> StateFingerprint {
    let head_oid = repo
        .head_id()
        .ok()
        .map(|id| id.to_hex().to_string())
        .unwrap_or_else(|| UNBORN_HEAD_OID.to_string());
    let head_tree_oid = repo
        .head_tree_id()
        .ok()
        .map(|id| id.to_hex().to_string())
        .unwrap_or_default();
    let index_checksum =
        read_index_trailer(&dot_git.join("index")).unwrap_or(ZERO_INDEX_CHECKSUM);
    let mut wikiignore = [0u8; 20];
    wikiignore.copy_from_slice(wikiignore_hash);
    let pairs = collect_worktree_pairs(repo_root);
    StateFingerprint {
        head_oid,
        head_tree_oid,
        index_checksum,
        wikiignore_hash: wikiignore,
        worktree_sig: worktree_signature(&pairs),
    }
}

/// Resolve `.git/HEAD` to a 40-char hex OID, following a single symref hop.
/// Falls back to `.git/packed-refs` when the loose ref file is absent.
/// Returns `Some(zero_oid)` for unborn HEAD (ref not found anywhere).
/// Stat-only-ish (small reads of HEAD, the ref file, packed-refs); no gix open.
fn read_head_oid(dot_git: &Path) -> Option<String> {
    const ZERO_OID: &str = "0000000000000000000000000000000000000000";

    let head = fs::read_to_string(dot_git.join("HEAD")).ok()?;
    let trimmed = head.trim();
    if let Some(rest) = trimmed.strip_prefix("ref:") {
        let ref_name = rest.trim();
        let ref_path = dot_git.join(ref_name);
        // Try loose ref first.
        if let Ok(resolved) = fs::read_to_string(&ref_path) {
            let oid = resolved.trim().to_string();
            if oid.len() == 40 {
                return Some(oid);
            }
        }
        // Fall back to packed-refs.
        let packed = dot_git.join("packed-refs");
        if let Ok(contents) = fs::read_to_string(&packed) {
            for line in contents.lines() {
                if line.starts_with('#') || line.starts_with('^') {
                    continue;
                }
                let mut parts = line.splitn(2, ' ');
                let oid_part = parts.next().unwrap_or("");
                let ref_part = parts.next().unwrap_or("").trim();
                if ref_part == ref_name && oid_part.len() == 40 {
                    return Some(oid_part.to_string());
                }
            }
        }
        // Ref not found: unborn HEAD.
        Some(ZERO_OID.to_string())
    } else if trimmed.len() == 40 {
        // Detached HEAD: the file contains the OID directly.
        Some(trimmed.to_string())
    } else {
        None
    }
}

/// Return the last 20 bytes of `.git/index` (the SHA-1 trailer git writes).
fn read_index_trailer(index_path: &Path) -> Option<[u8; 20]> {
    let meta = fs::metadata(index_path).ok()?;
    let len = meta.len();
    if len < 20 {
        return None;
    }
    let mut file = fs::File::open(index_path).ok()?;
    // Seek to end-20.
    use std::io::Seek;
    file.seek(std::io::SeekFrom::Start(len - 20)).ok()?;
    let mut buf = [0u8; 20];
    file.read_exact(&mut buf).ok()?;
    Some(buf)
}

/// Collect the sorted `(repo-relative path, mtime_ns)` pairs of every
/// directory and markdown file under `repo_root`, feeding
/// [`worktree_signature`]. Directory mtimes catch file creation and
/// deletion; file mtimes catch in-place edits (which do not update the
/// parent directory's mtime on Linux).
///
/// The walk is gitignore-aware (a superset of what the index ingests) and
/// prunes `.git` at any depth. The repo-root `.wikiignore` mtime pair folds
/// in so any edit to the ignore list changes the signature even for
/// content-only pattern rewrites.
pub(crate) fn collect_worktree_pairs(repo_root: &Path) -> Vec<(String, i64)> {
    // The parallel walker overlaps the per-entry stat round-trips across
    // threads, which dominates latency on a hostile (fuseblk) filesystem.
    // Threads push kept pairs into a shared Mutex<Vec>; the work is
    // stat-bound, not lock-bound, so a per-entry lock-and-push is fine.
    // Pairs are sorted after the walk, so collection order never leaks
    // into the signature (worktree_signature re-sorts defensively anyway).
    let pairs = std::sync::Mutex::new(Vec::<(std::path::PathBuf, i64)>::new());
    let walker = ignore::WalkBuilder::new(repo_root)
        .standard_filters(true)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .ignore(true)
        .parents(true)
        .filter_entry(|e| {
            let name = e.file_name();
            name != ".git"
        })
        .build_parallel();
    walker.run(|| {
        let pairs = &pairs;
        Box::new(
            move |entry: Result<ignore::DirEntry, ignore::Error>| -> ignore::WalkState {
                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => return ignore::WalkState::Continue,
                };
                let rel = match entry.path().strip_prefix(repo_root) {
                    Ok(r) => r.to_path_buf(),
                    Err(_) => return ignore::WalkState::Continue,
                };

                // Cheap readdir-backed filter first (no stat): only
                // directories and markdown files are kept. `file_type()`
                // comes from readdir's d_type on Linux, so this discards
                // `.rs`/`.ts`/`.json`/etc. without a stat.
                let is_dir = entry.file_type().is_some_and(|ft| ft.is_dir());
                let is_file = entry.file_type().is_some_and(|ft| ft.is_file());
                if !(is_dir || (is_file && is_markdown(&rel))) {
                    return ignore::WalkState::Continue;
                }

                // Stat only the entries we keep. `entry.metadata()` may
                // reuse a stat already performed by the walker.
                let mtime_ns = match entry
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_nanos() as i64)
                {
                    Some(m) => m,
                    None => return ignore::WalkState::Continue,
                };

                pairs.lock().unwrap().push((rel, mtime_ns));
                ignore::WalkState::Continue
            },
        )
    });

    // Fold the repo-root .wikiignore mtime into the signature so any edit
    // to the ignore list busts the gate. Only a stat — no WikiIgnore load
    // here (this cannot fail closed).
    {
        let wikiignore_path = repo_root.join(super::WIKIIGNORE_RELPATH);
        if let Ok(meta) = std::fs::metadata(&wikiignore_path)
            && let Ok(mtime) = meta.modified()
            && let Ok(dur) = mtime.duration_since(std::time::UNIX_EPOCH)
        {
            pairs.lock().unwrap().push((
                std::path::PathBuf::from(super::WIKIIGNORE_RELPATH),
                dur.as_nanos() as i64,
            ));
        }
    }

    let mut pairs = pairs.into_inner().unwrap();
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    pairs
        .into_iter()
        .map(|(p, m)| (p.to_string_lossy().to_string(), m))
        .collect()
}

/// True when `path` has a `.md` extension (case-insensitive).
fn is_markdown(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .map(|s| s.eq_ignore_ascii_case("md"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn make_dot_git() -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("refs/heads")).unwrap();
        dir
    }

    #[test]
    fn loose_ref_present() {
        let tmp = make_dot_git();
        let dot_git = tmp.path();
        let oid = "aabbccddeeff00112233445566778899aabbccdd";
        fs::write(dot_git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(dot_git.join("refs/heads/main"), format!("{oid}\n")).unwrap();
        assert_eq!(read_head_oid(dot_git), Some(oid.to_string()));
    }

    #[test]
    fn packed_refs_fallback() {
        let tmp = make_dot_git();
        let dot_git = tmp.path();
        let oid = "1122334455667788990011223344556677889900";
        fs::write(dot_git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        // No loose ref file — only packed-refs.
        fs::write(
            dot_git.join("packed-refs"),
            format!("# pack-refs with: peeled fully-peeled\n{oid} refs/heads/main\n"),
        )
        .unwrap();
        assert_eq!(read_head_oid(dot_git), Some(oid.to_string()));
    }

    #[test]
    fn unborn_head_returns_zero_oid() {
        let tmp = make_dot_git();
        let dot_git = tmp.path();
        // Neither loose ref nor packed-refs entry.
        fs::write(dot_git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        assert_eq!(
            read_head_oid(dot_git),
            Some("0000000000000000000000000000000000000000".to_string())
        );
    }

    #[test]
    fn detached_head() {
        let tmp = make_dot_git();
        let dot_git = tmp.path();
        let oid = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
        fs::write(dot_git.join("HEAD"), format!("{oid}\n")).unwrap();
        assert_eq!(read_head_oid(dot_git), Some(oid.to_string()));
    }

    fn set_mtime_ns(path: &std::path::Path, mtime_ns: i64) {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let c = CString::new(path.as_os_str().as_bytes()).unwrap();
        let times = [
            libc::timespec {
                tv_sec: (mtime_ns / 1_000_000_000) as libc::time_t,
                tv_nsec: (mtime_ns % 1_000_000_000) as _,
            },
            libc::timespec {
                tv_sec: (mtime_ns / 1_000_000_000) as libc::time_t,
                tv_nsec: (mtime_ns % 1_000_000_000) as _,
            },
        ];
        let rc = unsafe { libc::utimensat(libc::AT_FDCWD, c.as_ptr(), times.as_ptr(), 0) };
        assert_eq!(rc, 0, "utimensat failed");
    }

    #[test]
    fn worktree_signature_changes_on_wikiignore_content_change() {
        // Editing the repo-root `.wikiignore` must bust freshness — its
        // mtime pair folds into the collected pairs behind the signature.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let ignore = root.join(crate::index::WIKIIGNORE_RELPATH);

        // Absent → present must change the signature.
        let absent = worktree_signature(&collect_worktree_pairs(root));
        fs::write(&ignore, "drafts/\n").unwrap();
        let present = worktree_signature(&collect_worktree_pairs(root));
        assert_ne!(
            absent, present,
            "creating .wikiignore must change the worktree signature"
        );

        // A content edit that advances the mtime must also change it.
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
        let later_ns = later
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        fs::write(&ignore, "secrets/\n").unwrap();
        set_mtime_ns(&ignore, later_ns as i64);
        let edited = worktree_signature(&collect_worktree_pairs(root));
        assert_ne!(
            present, edited,
            "editing .wikiignore must change the worktree signature"
        );
    }
}
