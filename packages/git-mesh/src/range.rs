//! Range blob I/O (v1) — see §3.1, §4.1, §6.1.
//!
//! A Range is an immutable blob at `refs/ranges/v1/<rangeId>` with a
//! commit-object-style text format:
//!
//! ```text
//! anchor <sha>
//! created <iso-8601>
//! range <start> <end> <blob>\t<path>
//! ```

use crate::types::Range;
use crate::Result;

/// Compute the canonical ref path for a range id.
pub fn range_ref_path(range_id: &str) -> String {
    format!("refs/ranges/v1/{range_id}")
}

/// Create a Range record from user intent.
///
/// Workflow (§6.1):
/// 1. Verify `anchor_sha` is reachable and `path` exists in its tree.
/// 2. Resolve the blob OID of `path` at `anchor_sha`.
/// 3. Validate `(start, end)` fits within the blob's line count.
/// 4. Serialize, write the blob, create `refs/ranges/v1/<uuid>`.
///
/// Returns the fresh range id (UUID v4).
pub fn create_range(
    _repo: &gix::Repository,
    _anchor_sha: &str,
    _path: &str,
    _start: u32,
    _end: u32,
) -> Result<String> {
    todo!("range::create_range — §6.1")
}

/// Read the Range at `refs/ranges/v1/<range_id>`.
pub fn read_range(_repo: &gix::Repository, _range_id: &str) -> Result<Range> {
    todo!("range::read_range")
}

/// Parse a serialized Range blob (§4.1 format).
pub fn parse_range(_text: &str) -> Result<Range> {
    todo!("range::parse_range")
}

/// Serialize a Range record to the §4.1 on-disk format. Trailing
/// newline, no blank lines.
pub fn serialize_range(_range: &Range) -> String {
    todo!("range::serialize_range")
}
