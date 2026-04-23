//! Git plumbing helpers.
//!
//! Thin wrappers around `gix` (and `git` as a subprocess where `gix`
//! does not yet have first-class support — e.g. `git log -L`). These
//! are the only place in the crate that talks to git directly; the
//! rest of the crate stays on typed results.

use crate::Result;

/// Read a git object as UTF-8 text (blob contents, commit messages,
/// etc). Returns an `Error::Parse` if the bytes are not valid UTF-8.
///
/// Mirrors `git cat-file -p <oid>`.
pub fn read_git_text(_repo: &gix::Repository, _oid: &str) -> Result<String> {
    todo!("git::read_git_text — read blob/commit text via gix")
}

/// Resolve a commit-ish (sha, ref name, `HEAD~2`, etc.) to a full
/// commit OID. Errors if the ref does not resolve to a commit.
pub fn resolve_commit(_repo: &gix::Repository, _commit_ish: &str) -> Result<String> {
    todo!("git::resolve_commit")
}

/// True if `ancestor` is an ancestor of `descendant` (or equal).
/// Mirrors `git merge-base --is-ancestor`.
pub fn is_ancestor(
    _repo: &gix::Repository,
    _ancestor: &str,
    _descendant: &str,
) -> Result<bool> {
    todo!("git::is_ancestor")
}

/// Read the blob OID of `path` at `commit_oid`'s tree. `Err(PathNotInTree)`
/// if absent.
pub fn path_blob_at(
    _repo: &gix::Repository,
    _commit_oid: &str,
    _path: &str,
) -> Result<String> {
    todo!("git::path_blob_at")
}

/// Read file bytes from the working tree, relative to the repo root.
pub fn read_worktree_bytes(_repo: &gix::Repository, _path: &str) -> Result<Vec<u8>> {
    todo!("git::read_worktree_bytes")
}

/// Line count of `blob_oid`. Used to validate `(start, end)` is in range.
pub fn blob_line_count(_repo: &gix::Repository, _blob_oid: &str) -> Result<u32> {
    todo!("git::blob_line_count")
}

/// Extract lines `[start, end]` (1-based, inclusive) from a blob.
pub fn extract_blob_lines(
    _repo: &gix::Repository,
    _blob_oid: &str,
    _start: u32,
    _end: u32,
) -> Result<Vec<u8>> {
    todo!("git::extract_blob_lines")
}

/// Run `git log -L <start>,<end>:<path> <anchor_sha>..HEAD` with the
/// flags implied by the mesh's config. Returns the current
/// `(path, start, end, blob)` of the range at HEAD, or `None` if the
/// range no longer exists.
pub fn log_l_resolve(
    _repo: &gix::Repository,
    _anchor_sha: &str,
    _path: &str,
    _start: u32,
    _end: u32,
    _copy_detection: crate::types::CopyDetection,
) -> Result<Option<(String, u32, u32, String)>> {
    todo!("git::log_l_resolve — §5.1")
}

/// Find the commit that introduced the most recent change affecting
/// `(path, start, end)` from `anchor_sha` forward. Used for culprit
/// attribution on `Changed` ranges (§10.4 stale output).
pub fn culprit_commit(
    _repo: &gix::Repository,
    _anchor_sha: &str,
    _path: &str,
    _start: u32,
    _end: u32,
) -> Result<Option<String>> {
    todo!("git::culprit_commit")
}

/// `git update-ref <ref> <new_oid> <expected_oid>` — atomic CAS.
/// `expected_oid = None` means "must not exist" (create-only).
pub fn update_ref_cas(
    _repo: &gix::Repository,
    _ref_name: &str,
    _new_oid: &str,
    _expected_oid: Option<&str>,
) -> Result<()> {
    todo!("git::update_ref_cas")
}

/// `git update-ref -d <ref>`.
pub fn delete_ref(_repo: &gix::Repository, _ref_name: &str) -> Result<()> {
    todo!("git::delete_ref")
}
