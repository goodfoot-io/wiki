//! `compute_blob_oid(bytes)` and refcount bookkeeping. Phase 1
//! skeleton; bodies land in Phase 3 step 1.

use crate::index::BlobOid;

/// Compute the git blob SHA-1 of `bytes` (i.e. `sha1("blob " + len + "\0"
/// + bytes)`). Implementation will use `gix_object::compute_hash` +
/// `gix_hash::Kind::Sha1`.
pub fn compute_blob_oid(_bytes: &[u8]) -> BlobOid {
    unimplemented!("Phase 1 skeleton: blob::compute_blob_oid")
}
