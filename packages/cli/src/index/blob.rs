//! `compute_blob_oid(bytes)` and refcount bookkeeping.

use crate::index::BlobOid;

/// Compute the git blob SHA-1 of `bytes` (i.e. `sha1("blob " + len + "\0"
/// + bytes)`).
pub fn compute_blob_oid(bytes: &[u8]) -> BlobOid {
    let oid = gix::objs::compute_hash(gix::hash::Kind::Sha1, gix::objs::Kind::Blob, bytes)
        .expect("SHA-1 hashing is infallible");
    BlobOid(oid.to_hex().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_bytes() {
        let oid = compute_blob_oid(b"");
        assert_eq!(oid.0, "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391");
    }

    #[test]
    fn hello_newline() {
        let oid = compute_blob_oid(b"hello\n");
        assert_eq!(oid.0, "ce013625030ba8dba906f756967f9e9ca394464a");
    }

    #[test]
    fn title_body() {
        let oid = compute_blob_oid(b"# Title\n\nbody\n");
        assert_eq!(oid.0, "b055f63566615463e7f075b0ee7882fb683a1dc3");
    }
}
