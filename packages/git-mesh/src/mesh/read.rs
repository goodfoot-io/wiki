//! Read-only mesh operations — §6.5, §6.6, §10.4.

use crate::types::Mesh;
use crate::Result;

/// Commit-level metadata for a mesh tip, extracted from the commit
/// object. Used by `git mesh <name>` and `git mesh <name> --log`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeshCommitInfo {
    pub commit_oid: String,
    pub author_name: String,
    pub author_email: String,
    pub author_date: String,
    pub summary: String,
    pub message: String,
}

/// Enumerate every mesh name in the repo (by scanning `refs/meshes/v1/*`).
pub fn list_mesh_names(_repo: &gix::Repository) -> Result<Vec<String>> {
    todo!("mesh::read::list_mesh_names")
}

/// Read the mesh at its current tip (§6.5 `show`).
pub fn read_mesh(_repo: &gix::Repository, _name: &str) -> Result<Mesh> {
    todo!("mesh::read::read_mesh")
}

/// Read the mesh at a specific ancestor commit. `commit_ish = None`
/// means the current tip.
pub fn read_mesh_at(
    _repo: &gix::Repository,
    _name: &str,
    _commit_ish: Option<&str>,
) -> Result<Mesh> {
    todo!("mesh::read::read_mesh_at")
}

/// Alias for `read_mesh` used by the CLI's `show` dispatcher.
pub fn show_mesh(repo: &gix::Repository, name: &str) -> Result<Mesh> {
    read_mesh(repo, name)
}

/// Alias for `read_mesh_at` used by the CLI's `show --at <rev>`.
pub fn show_mesh_at(
    repo: &gix::Repository,
    name: &str,
    commit_ish: Option<&str>,
) -> Result<Mesh> {
    read_mesh_at(repo, name, commit_ish)
}

/// Commit metadata for the current tip.
pub fn mesh_commit_info(_repo: &gix::Repository, _name: &str) -> Result<MeshCommitInfo> {
    todo!("mesh::read::mesh_commit_info")
}

/// Commit metadata at a specific ancestor of the mesh ref.
pub fn mesh_commit_info_at(
    _repo: &gix::Repository,
    _name: &str,
    _commit_ish: Option<&str>,
) -> Result<MeshCommitInfo> {
    todo!("mesh::read::mesh_commit_info_at")
}

/// Walk `refs/meshes/v1/<name>` as a commit chain (§6.6). `limit = None`
/// means no limit. Newest first.
pub fn mesh_log(
    _repo: &gix::Repository,
    _name: &str,
    _limit: Option<usize>,
) -> Result<Vec<MeshCommitInfo>> {
    todo!("mesh::read::mesh_log")
}

/// True iff `ancestor` is an ancestor of the mesh's current tip (or equal).
pub fn is_ancestor_commit(
    _repo: &gix::Repository,
    _name: &str,
    _ancestor: &str,
) -> Result<bool> {
    todo!("mesh::read::is_ancestor_commit")
}

/// Resolve a commit-ish to a concrete commit OID in the mesh ref's
/// ancestry. Errors if the commit is not reachable from the tip.
pub fn resolve_commit_ish(
    _repo: &gix::Repository,
    _name: &str,
    _commit_ish: &str,
) -> Result<String> {
    todo!("mesh::read::resolve_commit_ish")
}
