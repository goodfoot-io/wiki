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
/// Stat-only-ish (small reads of HEAD and the ref file); no gix open.
fn read_head_oid(dot_git: &Path) -> Option<String> {
    let head = fs::read_to_string(dot_git.join("HEAD")).ok()?;
    let trimmed = head.trim();
    if let Some(rest) = trimmed.strip_prefix("ref:") {
        let ref_name = rest.trim();
        let ref_path = dot_git.join(ref_name);
        let resolved = fs::read_to_string(&ref_path).ok()?;
        let oid = resolved.trim().to_string();
        if oid.len() == 40 { Some(oid) } else { None }
    } else if trimmed.len() == 40 {
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
