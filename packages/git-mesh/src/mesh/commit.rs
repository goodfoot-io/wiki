//! Mesh commit pipeline — §6.1, §6.2.

use crate::Result;

/// Commit the staged operations for `name`. Full pipeline:
///
/// 1. Validate the mesh name (§10.2 reserved list).
/// 2. Load current mesh state (or empty for a new mesh).
/// 3. Validate every staged `remove` has a target.
/// 4. Validate every staged `add` doesn't collide post-remove.
/// 5. Reject if nothing meaningful is staged — a no-op `config` line
///    does not count (§6.2 step 5, `Error::StagingEmpty`).
/// 6. For each staged `add`: resolve anchor (HEAD at commit time when
///    no trailing sha; explicit sha otherwise), run the drift check
///    (§6.3 "Validation at commit time"), write the range blob, create
///    `refs/ranges/v1/<uuid>`.
/// 7. Apply removes then adds, sort by `(path, start, end)`, write the
///    `ranges` and `config` blobs, write the tree, write the commit
///    with the prior tip as parent. Message source: staged `.msg` if
///    present, else parent commit message. First commit with no staged
///    message is `Error::MessageRequired`.
/// 8. CAS-update `refs/meshes/v1/<name>`; retry if another client
///    advanced the tip concurrently.
/// 9. Delete every `.git/mesh/staging/<name>*` file.
/// 10. Rebuild `.git/mesh/file-index` (§3.4).
///
/// All-or-nothing. A single invalid op aborts before any object is
/// written. Returns the new mesh commit's OID.
pub fn commit_mesh(_repo: &gix::Repository, _name: &str) -> Result<String> {
    todo!("mesh::commit::commit_mesh — §6.1, §6.2")
}
