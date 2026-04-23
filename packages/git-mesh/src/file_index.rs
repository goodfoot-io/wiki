//! `.git/mesh/file-index` — derived lookup table (§3.4).
//!
//! Not synced, regenerated if absent. Lines are TAB-separated, sorted
//! by `(path, start)`:
//!
//! ```text
//! # mesh-index v1
//! <path>\t<mesh-name>\t<range-id>\t<start>\t<end>\t<anchor-sha-short>
//! ```

use crate::Result;

/// One parsed entry in the file index.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct IndexEntry {
    pub path: String,
    pub mesh_name: String,
    pub range_id: String,
    pub start: u32,
    pub end: u32,
    pub anchor_short: String,
}

/// Rebuild the file index from scratch by scanning every
/// `refs/meshes/v1/*` ref and resolving its Range ids. Full rewrite;
/// no diff-and-patch. Called after every successful commit / fetch (§3.4).
pub fn rebuild_index(_repo: &gix::Repository) -> Result<()> {
    todo!("file_index::rebuild_index — §3.4")
}

/// Read and parse the file index. Regenerates it if absent or if the
/// header is wrong (§3.4 lifecycle).
pub fn read_index(_repo: &gix::Repository) -> Result<Vec<IndexEntry>> {
    todo!("file_index::read_index")
}

/// `git mesh ls` — all files that have any range (§3.4).
pub fn ls_all(_repo: &gix::Repository) -> Result<Vec<IndexEntry>> {
    todo!("file_index::ls_all")
}

/// `git mesh ls <path>` — all ranges in all meshes referencing `path`.
pub fn ls_by_path(_repo: &gix::Repository, _path: &str) -> Result<Vec<IndexEntry>> {
    todo!("file_index::ls_by_path")
}

/// `git mesh ls <path>#L<s>-L<e>` — ranges whose `[a, b]` overlaps
/// `[start, end]` (i.e. `a <= end && b >= start`). (§3.4 overlap rule.)
pub fn ls_by_path_range(
    _repo: &gix::Repository,
    _path: &str,
    _start: u32,
    _end: u32,
) -> Result<Vec<IndexEntry>> {
    todo!("file_index::ls_by_path_range")
}
