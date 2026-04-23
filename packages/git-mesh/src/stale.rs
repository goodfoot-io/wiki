//! Resolver: compute staleness for ranges and meshes (§5).
//!
//! Pure read path. No side effects on the stored records.

use crate::types::{MeshResolved, RangeResolved};
use crate::Result;

/// Resolve a single range id against HEAD. Runs `git log -L` with the
/// mesh config's `copy-detection`, then compares bytes honouring
/// `ignore-whitespace` (§5.1, §5.2).
///
/// `Orphaned` is returned (not an error) when `anchor_sha` is
/// unreachable or the anchor path blob has been gc'd (§6.8).
pub fn resolve_range(
    _repo: &gix::Repository,
    _mesh_name: &str,
    _range_id: &str,
) -> Result<RangeResolved> {
    todo!("stale::resolve_range — §5.1/§5.2")
}

/// Resolve every range in a mesh, preserving stored order.
pub fn resolve_mesh(_repo: &gix::Repository, _name: &str) -> Result<MeshResolved> {
    todo!("stale::resolve_mesh")
}

/// For a `Changed` resolved range, find the commit that introduced the
/// drift (§10.4 stale output). Returns `None` for non-`Changed` input.
pub fn culprit_commit(
    _repo: &gix::Repository,
    _resolved: &RangeResolved,
) -> Result<Option<String>> {
    todo!("stale::culprit_commit")
}

/// Resolve every mesh in the repo, worst-first. Used by
/// `git mesh stale` with no `<name>` argument (§10.4).
pub fn stale_meshes(_repo: &gix::Repository) -> Result<Vec<MeshResolved>> {
    todo!("stale::stale_meshes")
}
