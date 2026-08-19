//! rk64 fingerprint kernel for git-derived fragment-link drift detection.
//!
//! Vendored from the `git-mesh-core` crate (rev `958f3a0`, MIT, Goodfoot
//! Media LLC — [github.com/goodfoot-io/git-mesh](https://github.com/goodfoot-io/git-mesh)),
//! taking only the rk64 fingerprint family: the SHA-256 family and the mesh
//! file format are mesh concerns and stay with git-mesh-core. Byte-parity with
//! the reference kernel is proven by the Phase-0 parity gate
//! (`tests/fixtures/rk64-parity.json`, generated from the pre-removal `.wiki/`
//! anchor corpus).
//!
//! ## Fingerprint contract
//!
//! rk64 is a 64-bit, **non-cryptographic**, linear (polynomial/Rabin–Karp)
//! fingerprint of an extent's canonical content:
//!
//! - **Line range** (inclusive 1-based `[start, end]`): the file bytes are
//!   UTF-8-lossy decoded, split with Rust `str::lines` semantics (split on
//!   `\n`, one trailing `\r` stripped per element, no trailing empty line when
//!   the buffer ends in `\n`), sliced `lines[start-1..end]` clamped to EOF, and
//!   joined with `\n`.
//! - **Whole file**: the raw byte buffer, no canonicalization.
//! - A degenerate range (`start == 0`, `end < start`, or past EOF) selects no
//!   content and fingerprints to `0`.
//!
//! The fingerprint is `h = Σ (b + 1) · BASE^(len-1-i)` over wrapping `u64` with
//! `BASE = 0x0000_0100_0000_01b3` (the FNV-64 prime — odd, so the rolling
//! subtraction is exact). `horner(b"") == 0`. The hex encoding is
//! [`rk64_to_hex`]: lowercase, zero-padded to 16 digits, big-endian.
//!
//! rk64 is sound here because it tracks documentation links, where a rare
//! wrong/missed match is self-correcting — never use it as a content-integrity
//! hash.

use std::marker::PhantomData;

/// The extent of a fingerprint: either the whole file, or an inclusive
/// 1-based line range.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Extent {
    WholeFile,
    LineRange { start: u32, end: u32 },
}

/// One place a stored fingerprint was found in the caller-supplied files.
/// For a whole-file match `start_line` and `end_line` are both `0` (the
/// whole-file convention); for a line range they are the 1-based inclusive
/// bounds of the matching window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
}

/// A reusable, allocation-cheap line index over a byte buffer: the start and
/// content-end (terminator-excluded) offset of every line, derived once with a
/// single newline scan. Line counting matches `str::lines` exactly — a `\r\n`
/// or `\n` ends a line and a trailing line terminator yields no final empty
/// line.
///
/// Callers that read a file once and fingerprint **many** ranges against it
/// build the index once and reuse it across [`cheap_fingerprint_indexed`] and
/// [`scan_indexed_rk64`], paying the newline scan a single time.
///
/// Line offsets are stored as `u32`, so the buffer must be at most `u32::MAX`
/// (just under 4 GiB) bytes. A larger buffer is **refused** (panic) rather than
/// indexed with silently truncated offsets: the contract is fail-closed, and a
/// wrapped offset would produce a syntactically valid but semantically wrong
/// fingerprint.
#[derive(Clone)]
pub struct LineIndex<'a> {
    _marker: PhantomData<&'a [u8]>,
}

impl<'a> LineIndex<'a> {
    /// Build the line index for `bytes` with one forward newline scan.
    pub fn build(bytes: &'a [u8]) -> LineIndex<'a> {
        let _ = bytes;
        todo!("rk64::LineIndex::build (Phase 1)")
    }

    /// The underlying buffer.
    pub fn bytes(&self) -> &'a [u8] {
        todo!("rk64::LineIndex::bytes (Phase 1)")
    }

    /// Number of lines, per `str::lines` counting.
    pub fn line_count(&self) -> usize {
        todo!("rk64::LineIndex::line_count (Phase 1)")
    }
}

/// Cheap fingerprint of an extent's canonical content (whole-file extents
/// fingerprint the full buffer; a range selecting no line fingerprints to `0`).
pub fn cheap_fingerprint_with_extent(bytes: &[u8], extent: &Extent) -> u64 {
    let _ = (bytes, extent);
    todo!("rk64::cheap_fingerprint_with_extent (Phase 1)")
}

/// [`cheap_fingerprint_with_extent`] over a prebuilt [`LineIndex`]. Produces an
/// identical `u64`; the only difference is that the newline scan is already
/// paid for.
pub fn cheap_fingerprint_indexed(idx: &LineIndex<'_>, extent: &Extent) -> u64 {
    let _ = (idx, extent);
    todo!("rk64::cheap_fingerprint_indexed (Phase 1)")
}

/// Find every window whose fingerprint equals `cheap_fp`, with **no
/// content-hash confirmation** — the 64-bit fingerprint is the sole content
/// identity.
///
/// Exhaustive, fail-closed match set: **all** matches are returned (same `near`
/// ordering as the reference kernel: stable sort by distance from the 1-based
/// `near` line, ties toward the lower start line), so ≥2 matches means
/// ambiguous and the caller refuses to act. A whole-file extent matches whole
/// files by their fingerprint.
pub fn scan_indexed_rk64(
    files: &[(String, LineIndex<'_>)],
    cheap_fp: u64,
    extent: Extent,
    near: Option<u32>,
) -> Vec<Location> {
    let _ = (files, cheap_fp, extent, near);
    todo!("rk64::scan_indexed_rk64 (Phase 1)")
}

/// [`scan_indexed_rk64`] over `Vec<u8>` inputs, building each [`LineIndex`]
/// internally.
pub fn scan_for_content_hash_rk64(
    files: &[(String, Vec<u8>)],
    cheap_fp: u64,
    extent: Extent,
    near: Option<u32>,
) -> Vec<Location> {
    let _ = (files, cheap_fp, extent, near);
    todo!("rk64::scan_for_content_hash_rk64 (Phase 1)")
}

/// Canonical hex encoding of an rk64 fingerprint: **lowercase, zero-padded to
/// 16 digits, big-endian** (most-significant nibble first), i.e.
/// `format!("{fp:016x}")`. Pair with [`rk64_from_hex`] so a writer and any
/// reader agree on the exact bytes.
pub fn rk64_to_hex(fp: u64) -> String {
    let _ = fp;
    todo!("rk64::rk64_to_hex (Phase 1)")
}

/// Parse the canonical [`rk64_to_hex`] encoding back to a `u64`. Returns
/// `None` for anything other than exactly 16 lowercase hex digits, so a
/// malformed or non-canonical token is rejected rather than silently
/// mis-decoded.
pub fn rk64_from_hex(s: &str) -> Option<u64> {
    let _ = s;
    todo!("rk64::rk64_from_hex (Phase 1)")
}
