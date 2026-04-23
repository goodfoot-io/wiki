//! Structural mesh operations — §6.8.

use crate::Result;

/// `git mesh delete <name>` — `update-ref -d refs/meshes/v1/<name>`.
/// Reachable commits remain in the object db until `git gc`.
pub fn delete_mesh(_repo: &gix::Repository, _name: &str) -> Result<()> {
    todo!("mesh::structural::delete_mesh — §6.8")
}

/// `git mesh mv <old> <new>` — atomic rename: create new ref at the
/// old tip, then delete the old ref. Errors if `<new>` already exists
/// or `<new>` is reserved.
pub fn rename_mesh(_repo: &gix::Repository, _old: &str, _new: &str) -> Result<()> {
    todo!("mesh::structural::rename_mesh")
}

/// `git mesh restore <name>` — delete every
/// `.git/mesh/staging/<name>*` file. Analogous to `git restore --staged`
/// on all files for a single mesh (§6.8). Does not touch the ref.
pub fn restore_mesh(_repo: &gix::Repository, _name: &str) -> Result<()> {
    todo!("mesh::structural::restore_mesh")
}

/// `git mesh revert <name> <commit-ish>` — fast-forward to a past state
/// by writing a new commit whose tree matches `<commit-ish>` with the
/// current tip as parent. History is never rewritten (§6.6). Returns
/// the new tip OID.
pub fn revert_mesh(
    _repo: &gix::Repository,
    _name: &str,
    _commit_ish: &str,
) -> Result<String> {
    todo!("mesh::structural::revert_mesh — §6.6")
}
