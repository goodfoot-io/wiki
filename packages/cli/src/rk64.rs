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

// ── Tests ─────────────────────────────────────────────────────────────────────
//
// Phase 0 P2 (tdd-bootstrap): acceptance checks against the stubs, all
// pending. P3 unskips them one concern at a time.

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn from_hex(s: &str) -> Vec<u8> {
        assert_eq!(s.len() % 2, 0, "hex input must have even length");
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex"))
            .collect()
    }

    // ── Parity gate: every stored rk64 anchor pair from the .wiki/ corpus at
    // the baseline, with canonical content embedded in the fixture (the
    // corpus is deleted in Phase 4; the fixture stands alone). The kernel
    // must reproduce each stored value from its embedded content —
    // byte-parity with the git-mesh-core reference is this test's whole job.

    #[test]
    #[ignore = "Phase 0 P3: implement cheap_fingerprint_with_extent"]
    fn parity_gate_reproduces_all_183_stored_anchor_pairs() {
        let fixture = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/rk64-parity.json"
        ));
        let doc: serde_json::Value = serde_json::from_slice(fixture).expect("fixture parses");
        let entries = doc["entries"].as_array().expect("entries array");
        assert_eq!(entries.len(), 183, "the whole baseline corpus is pinned");

        let mut current = 0;
        let mut historical = 0;
        for (i, entry) in entries.iter().enumerate() {
            let path = entry["path"].as_str().unwrap();
            let stored = entry["stored"].as_str().unwrap();
            let content = from_hex(entry["content_hex"].as_str().unwrap());
            let extent = match (entry["start"].as_u64(), entry["end"].as_u64()) {
                (Some(s), Some(e)) => Extent::LineRange {
                    start: s as u32,
                    end: e as u32,
                },
                (None, None) => Extent::WholeFile,
                other => panic!("entry {i}: malformed extent {other:?}"),
            };
            let actual = rk64_to_hex(cheap_fingerprint_with_extent(&content, &extent));
            assert_eq!(
                actual, stored,
                "entry {i} ({path}, {extent:?}): kernel disagrees with the stored pair"
            );
            match entry["kind"].as_str().unwrap() {
                "current" => {
                    current += 1;
                    assert_eq!(entry["historical_commit"], serde_json::Value::Null);
                }
                "historical" => {
                    historical += 1;
                    assert!(entry["historical_commit"].as_str().is_some());
                }
                other => panic!("entry {i}: unknown kind {other}"),
            }
        }
        // The spike's measured split: 162 fresh + 21 stale-by-design.
        assert_eq!(current, 162);
        assert_eq!(historical, 21);
    }

    // ── Golden vectors: hand-pinned values computed by the proven reference
    // implementation during the spike. These hold even if the fixture is
    // regenerated.

    #[test]
    #[ignore = "Phase 0 P3: implement cheap_fingerprint_with_extent"]
    fn golden_vectors_match_reference_values() {
        assert_eq!(rk64_to_hex(cheap_fingerprint_with_extent(b"a\nb\nc", &Extent::WholeFile)), "9e8ea13137a80ccb");
        assert_eq!(rk64_to_hex(cheap_fingerprint_with_extent(b"", &Extent::WholeFile)), "0000000000000000");
        assert_eq!(rk64_to_hex(cheap_fingerprint_with_extent(b"x", &Extent::WholeFile)), "0000000000000079");
        assert_eq!(rk64_to_hex(cheap_fingerprint_with_extent(b"whole\ncontent\n", &Extent::WholeFile)), "9acbca60ec42d854");
        // Whole-file fingerprints hash raw bytes — invalid UTF-8 must not be
        // lossy-rewritten.
        assert_eq!(rk64_to_hex(cheap_fingerprint_with_extent(b"line with \xff byte", &Extent::WholeFile)), "39abc29a440ce15f");
    }

    // ── Canonicalization edges: the reference contract, one test per edge.
    // A line range is UTF-8-lossy decoded, split with `str::lines` semantics,
    // sliced [start-1..end] clamped to EOF, joined with \n.

    #[test]
    #[ignore = "Phase 0 P3: implement cheap_fingerprint_with_extent"]
    fn line_range_joins_lines_with_newlines() {
        let bytes = b"a\nb\nc\nd\n";
        let range = Extent::LineRange { start: 1, end: 3 };
        let joined = cheap_fingerprint_with_extent(b"a\nb\nc", &Extent::WholeFile);
        assert_eq!(
            cheap_fingerprint_with_extent(bytes, &range),
            joined,
            "a range fingerprints exactly its joined canonical content"
        );
    }

    #[test]
    #[ignore = "Phase 0 P3: implement cheap_fingerprint_with_extent"]
    fn crlf_and_lf_twins_share_range_fingerprints() {
        let crlf = b"a\r\nb\r\nc\r\n";
        let lf = b"a\nb\nc\n";
        for (start, end) in [(1, 1), (1, 2), (2, 3), (1, 3)] {
            let range = Extent::LineRange { start, end };
            assert_eq!(
                cheap_fingerprint_with_extent(crlf, &range),
                cheap_fingerprint_with_extent(lf, &range),
                "CRLF range {start}..={end} must canonicalize identically to LF"
            );
        }
        // The pinned canonical value for the CRLF 1..=2 range ("a\nb").
        assert_eq!(
            rk64_to_hex(cheap_fingerprint_with_extent(crlf, &Extent::LineRange { start: 1, end: 2 })),
            "014d1700011b08c6"
        );
        // Whole-file extents hash RAW bytes — the twins must differ there.
        assert_ne!(
            cheap_fingerprint_with_extent(crlf, &Extent::WholeFile),
            cheap_fingerprint_with_extent(lf, &Extent::WholeFile)
        );
    }

    #[test]
    #[ignore = "Phase 0 P3: implement cheap_fingerprint_with_extent"]
    fn invalid_utf8_is_lossy_rewritten_in_ranges_not_whole_files() {
        let bytes = b"x\xffy\nz\n";
        let lossy = String::from_utf8_lossy(bytes);
        let first_line = lossy.lines().next().unwrap();
        let range = Extent::LineRange { start: 1, end: 1 };
        assert_eq!(
            cheap_fingerprint_with_extent(bytes, &range),
            cheap_fingerprint_with_extent(first_line.as_bytes(), &Extent::WholeFile),
            "ranges fingerprint the U+FFFD-rewritten content"
        );
    }

    #[test]
    #[ignore = "Phase 0 P3: implement cheap_fingerprint_with_extent"]
    fn degenerate_ranges_fingerprint_to_zero() {
        let bytes = b"a\nb\nc\n";
        for (start, end) in [(0u32, 3u32), (5, 3), (0, u32::MAX)] {
            let range = Extent::LineRange { start, end };
            assert_eq!(
                cheap_fingerprint_with_extent(bytes, &range),
                0,
                "degenerate range {start}..={end} selects no content"
            );
        }
        // Past-EOF ranges clamp: the slice still holds the whole file.
        let clamped = Extent::LineRange { start: 1, end: 9 };
        assert_eq!(
            cheap_fingerprint_with_extent(bytes, &clamped),
            cheap_fingerprint_with_extent(b"a\nb\nc", &Extent::WholeFile)
        );
        // A range starting past EOF selects nothing.
        assert_eq!(
            cheap_fingerprint_with_extent(bytes, &Extent::LineRange { start: 5, end: 6 }),
            0
        );
    }

    #[test]
    #[ignore = "Phase 0 P3: implement cheap_fingerprint_with_extent"]
    fn trailing_newline_yields_no_final_empty_line() {
        let with_nl = b"a\nb\n";
        let without_nl = b"a\nb";
        for (start, end) in [(1, 2), (2, 2), (1, 9)] {
            let range = Extent::LineRange { start, end };
            assert_eq!(
                cheap_fingerprint_with_extent(with_nl, &range),
                cheap_fingerprint_with_extent(without_nl, &range),
                "trailing newline must not add a phantom final line"
            );
        }
    }

    #[test]
    #[ignore = "Phase 0 P3: implement LineIndex and cheap_fingerprint_indexed"]
    fn byte_slice_and_indexed_entry_points_agree() {
        let cases: &[&[u8]] = &[b"", b"a", b"a\n", b"a\nb\nc", b"a\r\nb\r\nc\r\n", b"x\xffy\nz\n"];
        for &bytes in cases {
            let idx = LineIndex::build(bytes);
            assert_eq!(idx.line_count(), String::from_utf8_lossy(bytes).lines().count());
            assert_eq!(
                cheap_fingerprint_with_extent(bytes, &Extent::WholeFile),
                cheap_fingerprint_indexed(&idx, &Extent::WholeFile)
            );
            for start in 0u32..=6 {
                for end in 0u32..=8 {
                    let extent = Extent::LineRange { start, end };
                    assert_eq!(
                        cheap_fingerprint_with_extent(bytes, &extent),
                        cheap_fingerprint_indexed(&idx, &extent),
                        "{bytes:?} range {start}..={end}"
                    );
                }
            }
        }
    }

    // ── Hex encoding.

    #[test]
    #[ignore = "Phase 0 P3: implement rk64_to_hex / rk64_from_hex"]
    fn hex_encoding_is_canonical_and_round_trips() {
        for fp in [0u64, 1, 0xff, 0x1234_5678_9abc_def0, u64::MAX] {
            let hexed = rk64_to_hex(fp);
            assert_eq!(hexed.len(), 16, "zero-padded to 16 digits");
            assert_eq!(hexed, hexed.to_lowercase(), "lowercase");
            assert_eq!(rk64_from_hex(&hexed), Some(fp), "round-trips");
        }
        assert_eq!(rk64_to_hex(0x1), "0000000000000001", "big-endian");
        assert_eq!(rk64_from_hex("1"), None);
        assert_eq!(rk64_from_hex("00000000000000001"), None);
        assert_eq!(rk64_from_hex("0000000000ABCDEF"), None, "uppercase rejected");
        assert_eq!(rk64_from_hex("000000000000000g"), None);
    }

    // ── The exhaustive, fail-closed window scan.

    #[test]
    #[ignore = "Phase 0 P3: implement scan_indexed_rk64"]
    fn scan_finds_duplicated_windows_fail_closed() {
        let files = vec![("dup.txt".to_string(), b"x\ny\nz\nq\nx\ny\nz\n".to_vec())];
        let extent = Extent::LineRange { start: 1, end: 3 };
        let fp = cheap_fingerprint_with_extent(b"x\ny\nz", &Extent::WholeFile);
        let hits = scan_for_content_hash_rk64(&files, fp, extent, None);
        assert_eq!(
            hits,
            vec![
                Location { path: "dup.txt".into(), start_line: 1, end_line: 3 },
                Location { path: "dup.txt".into(), start_line: 5, end_line: 7 },
            ],
            "≥2 matches is the caller's ambiguity signal — the scan never picks"
        );
    }

    #[test]
    #[ignore = "Phase 0 P3: implement scan_indexed_rk64"]
    fn scan_orders_nearest_window_first() {
        let files = vec![("a.txt".to_string(), b"d\nd\nx\nd\nd\n".to_vec())];
        let extent = Extent::LineRange { start: 1, end: 2 };
        let fp = cheap_fingerprint_with_extent(b"d\nd", &Extent::WholeFile);
        let near_top = scan_for_content_hash_rk64(&files, fp, extent, Some(1));
        assert_eq!(near_top[0].start_line, 1);
        let near_bottom = scan_for_content_hash_rk64(&files, fp, extent, Some(4));
        assert_eq!(near_bottom[0].start_line, 4);
    }

    #[test]
    #[ignore = "Phase 0 P3: implement scan_indexed_rk64"]
    fn scan_matches_whole_files_by_fingerprint() {
        let files = vec![
            ("yes.txt".to_string(), b"whole\ncontent\n".to_vec()),
            ("no.txt".to_string(), b"other\n".to_vec()),
        ];
        let fp = cheap_fingerprint_with_extent(b"whole\ncontent\n", &Extent::WholeFile);
        assert_eq!(
            scan_for_content_hash_rk64(&files, fp, Extent::WholeFile, None),
            vec![Location { path: "yes.txt".into(), start_line: 0, end_line: 0 }]
        );
    }

    #[test]
    #[ignore = "Phase 0 P3: implement scan_indexed_rk64"]
    fn scan_handles_crlf_windows_canonically() {
        let files = vec![("crlf.txt".to_string(), b"a\r\nb\r\nc\r\nd\r\n".to_vec())];
        let extent = Extent::LineRange { start: 1, end: 2 };
        let fp = cheap_fingerprint_with_extent(b"b\nc", &Extent::WholeFile);
        let hits = scan_for_content_hash_rk64(&files, fp, extent, None);
        assert_eq!(
            hits,
            vec![Location { path: "crlf.txt".into(), start_line: 2, end_line: 3 }]
        );
    }
}
