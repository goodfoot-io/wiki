//! Pure key derivation for the anchor cache (plan decision 1).
//!
//! Both tiers key on a sha256 digest of a canonical, provably injective
//! length-tagged encoding: every field is its UTF-8 byte length as a
//! little-endian `u64` followed by the bytes; `start`/`end` contribute their
//! decimal ASCII representation (still length-tagged like every other
//! field). Length tags keep the encoding injective over arbitrary bytes —
//! git paths may contain `#`, `L`, tabs, newlines, or any UTF-8 — so two
//! distinct tuples can never share a key digest. On serve the stored tuple
//! is additionally re-bound to the key before a row is trusted
//! (belt-and-braces over the injectivity proof, plan decision 1).

use sha2::{Digest, Sha256};

/// Append one field's canonical encoding: the UTF-8 byte length as a
/// little-endian `u64`, then the bytes.
fn push_field(out: &mut Vec<u8>, field: &str) {
    let len = field.len() as u64;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(field.as_bytes());
}

/// The 64-lowercase-hex sha256 digest of `bytes`.
///
/// This is the digest form used everywhere the cache identifies content: the
/// key digests below, `log_output_sha` in tier-A rows (computed by the
/// upsert caller), and the serve-side verification of tier-A lookups.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        write!(hex, "{byte:02x}").expect("writing to a String cannot fail");
    }
    hex
}

/// Derive the fingerprint-tier key digest from its canonical tuple (field
/// order: page, anchor_sha, target, start, end).
///
/// `start`/`end` are the certified range bounds (`LinkClass` line numbers).
pub fn fingerprint_key(
    page: &str,
    anchor_sha: &str,
    target: &str,
    start: u32,
    end: u32,
) -> String {
    let mut out = Vec::new();
    push_field(&mut out, page);
    push_field(&mut out, anchor_sha);
    push_field(&mut out, target);
    push_field(&mut out, &start.to_string());
    push_field(&mut out, &end.to_string());
    sha256_hex(&out)
}

/// The canonical length-tagged byte encoding of a tuple — the same field
/// framing the key digests hash (u64 LE length + bytes, in field order).
/// [`row_digest`] reuses it so the row digest covers the exact bytes the key
/// names.
pub(crate) fn canonical_fields(fields: &[&str]) -> Vec<u8> {
    let mut out = Vec::new();
    for field in fields {
        push_field(&mut out, field);
    }
    out
}

/// The row integrity digest: raw 32 sha256 bytes over the canonical
/// (tuple + value) encoding, so every stored row carries a self-check and a
/// served row is re-derived and re-verified, never trusted blind (plan
/// decision 5).
///
/// Exact byte layout (length-tagged like every key field, with a tag byte so
/// `None` is distinct from an empty `Some("")`):
///
/// * tuple — each field as its u64 LE byte length + bytes, in key field
///   order: tier F is (page, anchor_sha, target, start, end), tier A is
///   (page, log_output_sha, anchor_sha, path_at_commit);
/// * value — `0x00` for `None`, or `0x01` + u64 LE byte length + bytes.
pub(crate) fn row_digest(tuple: &[&str], value: Option<&str>) -> Vec<u8> {
    let mut out = canonical_fields(tuple);
    match value {
        None => out.push(0x00),
        Some(v) => {
            out.push(0x01);
            push_field(&mut out, v);
        }
    }
    Sha256::digest(&out).to_vec()
}

/// Derive the anchor-walk-tier key digest from its canonical tuple (field
/// order: page, log_output).
///
/// `log_output` must be the *exact* untrimmed `String::from_utf8_lossy`
/// string the walk parses — the output of `git log --follow --name-status
/// --format=%H -- <page>` (plan decision 1). The key is the walk's entire
/// non-blob input: the commit sequence and rename rows are pinned by hashing
/// the log output itself, and the page blobs at those commits are pinned by
/// the commit SHAs, so a served epoch is provably the same computation a
/// from-scratch walk performs.
pub fn walk_key(page: &str, log_output: &str) -> String {
    let mut out = Vec::new();
    push_field(&mut out, page);
    push_field(&mut out, log_output);
    sha256_hex(&out)
}
