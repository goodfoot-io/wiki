//! Fetch/push for mesh and range refs (§7).

use crate::Result;

/// Read `mesh.defaultRemote` (default `origin`, §10.5).
pub fn default_remote(_repo: &gix::Repository) -> Result<String> {
    todo!("sync::default_remote — §10.5")
}

/// `git fetch <remote> +refs/ranges/*:refs/ranges/* +refs/meshes/*:refs/meshes/*`.
/// Calls [`ensure_refspec_configured`] first (§7.1 lazy config).
pub fn fetch_mesh_refs(_repo: &gix::Repository, _remote: &str) -> Result<()> {
    todo!("sync::fetch_mesh_refs")
}

/// Push mesh and range refs. Same lazy refspec bootstrap as fetch.
pub fn push_mesh_refs(_repo: &gix::Repository, _remote: &str) -> Result<()> {
    todo!("sync::push_mesh_refs")
}

/// Ensure `remote.<remote>.fetch` and `remote.<remote>.push` each carry
/// the mesh and range refspec lines. Idempotent via `git config --add`
/// with a pre-check (§7.1). Fail-closed — if the remote does not exist,
/// error out with `Error::RefspecMissing`.
pub fn ensure_refspec_configured(_repo: &gix::Repository, _remote: &str) -> Result<()> {
    todo!("sync::ensure_refspec_configured — §7.1")
}
