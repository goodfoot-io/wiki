//! Fast triple gate: 3× stat on `.git/HEAD`, `.git/index`, repo root +
//! state-row read. No `gix::Repository` open on the fast path.

use std::fs;
use std::io::Read;
use std::path::Path;

#[allow(dead_code)]
pub struct FastTriple {
    pub head_oid: String,
    pub index_checksum: [u8; 20],
    pub worktree_generation: i64,
}

/// Stat-only fast gate: three stats + one state-row read; no gix open.
///
/// Returns `Ok(Some(triple))` when the on-disk triple matches the state row.
/// Returns `Ok(None)` when any input is missing or the state is stale and a
/// refresh is therefore required.
pub fn fast_gate(
    dot_git: &Path,
    conn: &rusqlite::Connection,
) -> anyhow::Result<Option<FastTriple>> {
    let head_path = dot_git.join("HEAD");
    let index_path = dot_git.join("index");
    let repo_root = dot_git.parent().unwrap_or(dot_git);

    if fs::metadata(&head_path).is_err()
        || fs::metadata(&index_path).is_err()
        || fs::metadata(repo_root).is_err()
    {
        return Ok(None);
    }

    let head_oid = match read_head_oid(dot_git) {
        Some(h) => h,
        None => return Ok(None),
    };

    let index_checksum = match read_index_trailer(&index_path) {
        Some(t) => t,
        None => return Ok(None),
    };

    let (state_head, state_checksum, state_generation): (String, Vec<u8>, i64) = match conn
        .query_row(
            "SELECT head_oid, index_checksum, worktree_generation FROM state WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ) {
        Ok(t) => t,
        Err(_) => return Ok(None),
    };

    if state_head != head_oid {
        return Ok(None);
    }
    if state_checksum.len() != 20 || state_checksum.as_slice() != index_checksum.as_slice() {
        return Ok(None);
    }

    // Also require state to be non-empty (a fresh DB has empty head_oid + zero
    // checksum). If everything is zero, force a refresh.
    if state_head.is_empty() {
        return Ok(None);
    }

    Ok(Some(FastTriple {
        head_oid,
        index_checksum,
        worktree_generation: state_generation,
    }))
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
