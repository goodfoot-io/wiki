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

use std::sync::{Arc, OnceLock};

/// Polynomial base for the fingerprint (the FNV-64 prime — odd, so the
/// rolling subtraction is exact over wrapping `u64` arithmetic).
const FP_BASE: u64 = 0x0000_0100_0000_01b3;

/// Per-byte value mapped into the polynomial. Adding one keeps a leading `\0`
/// from vanishing (a zero byte would otherwise contribute nothing and shift
/// silently), so distinct content is less likely to collide.
#[inline]
fn fp_byte(b: u8) -> u64 {
    (b as u64).wrapping_add(1)
}

/// Horner polynomial hash of `bytes`: `Σ fp_byte(bytes[i]) · BASE^(len-1-i)`,
/// over wrapping `u64`. `horner(b"") == 0`. This is the canonical fingerprint
/// of an already-canonicalized content slice; the rolling scan reproduces it
/// per window via prefix hashes.
fn horner(bytes: &[u8]) -> u64 {
    let mut h = 0u64;
    for &b in bytes {
        h = h.wrapping_mul(FP_BASE).wrapping_add(fp_byte(b));
    }
    h
}

/// Window height (line count) of an inclusive 1-based `LineRange`, or `0` for a
/// degenerate extent that selects no content. An extent is degenerate when
/// `start == 0` (no 1-based line) or `end < start` (empty range); both
/// fingerprint to `0`, so the scan family must agree by treating them as a
/// zero-height window. Computed before any arithmetic, so `start == 0,
/// end == u32::MAX` can never overflow.
fn line_range_span(start: u32, end: u32) -> usize {
    if start == 0 || end < start {
        return 0;
    }
    (end - start + 1) as usize
}

/// Byte offsets `[start, end)` of the canonical fingerprint region for the
/// inclusive 1-based line range `[start_line, end_line]`, clamped to EOF
/// per `str::lines` line counting. `None` when the range selects no line
/// (the caller then fingerprints `0`, matching `[].join("\n")`).
///
/// Allocation-free: a single forward pass that stops as soon as the
/// `end`-terminating newline is seen.
///
/// `LineIndex::region` is the indexed equivalent; the test
/// `byte_slice_and_indexed_entry_points_agree` cross-checks the two against
/// every vector and range so they cannot drift independently.
fn line_range_region(bytes: &[u8], start_line: u32, end_line: u32) -> Option<(usize, usize)> {
    if start_line == 0 {
        // `start == 0` has no 1-based line; a degenerate extent selects no
        // content, matching `line_range_span` and the scan family.
        return None;
    }
    let lo = start_line.saturating_sub(1) as usize; // 0-based first wanted line
    let hi = end_line as usize; // exclusive last wanted line (pre-clamp)
    if lo >= hi {
        // `end` selects no line (e.g. `end == 0` or `end < start`), matching
        // the reference's `lo < hi` guard before clamping.
        return None;
    }

    let len = bytes.len();
    // A non-empty buffer not ending in `\n` has an unterminated final line;
    // one ending in `\n` does not (matching `str::lines`).
    let trailing = !bytes.is_empty() && bytes[len - 1] != b'\n';

    let mut region_start: Option<usize> = if lo == 0 { Some(0) } else { None };
    let mut region_end: Option<usize> = None;
    let mut nl = 0usize; // count of '\n' seen so far
    let mut last_nl: Option<usize> = None;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\n' {
            // This is the `(nl + 1)`-th newline: it ends line `nl` at `i` and
            // starts line `nl + 1` at `i + 1`.
            if nl + 1 == lo {
                region_start = Some(i + 1);
            }
            if nl + 1 == hi {
                region_end = Some(i);
            }
            nl += 1;
            last_nl = Some(i);
            if region_end.is_some() {
                break; // found the `end`-terminating newline — stop early.
            }
        }
    }

    let line_count = nl + usize::from(trailing);
    if lo >= line_count {
        // `start` is past every line — an empty range.
        return None;
    }
    let rs = region_start.expect("region_start set for lo < line_count");
    // `region_end == None` means the range runs to (or past) EOF: the last
    // wanted line is the final line, whose content ends at EOF when it is
    // unterminated, or at the last newline when the buffer ends in `\n`.
    let re = region_end.unwrap_or(if trailing {
        len
    } else {
        last_nl.expect("a terminated non-empty range has a final newline")
    });
    Some((rs, re))
}

/// Apply `fingerprint` to the canonical content of buffer region `[rs, re)`.
/// On the LF-and-UTF-8 fast path the region is byte-identical to the
/// canonical `lines[lo..hi].join("\n")`, so `fingerprint` runs directly on the
/// slice with no allocation. Otherwise (`\r` present, or invalid UTF-8
/// that `from_utf8_lossy` would rewrite) `fingerprint` receives the
/// `canonical_join_bytes` fallback so the output is byte-identical.
fn canonical_region<T>(
    bytes: &[u8],
    rs: usize,
    re: usize,
    start: u32,
    end: u32,
    fingerprint: impl FnOnce(&[u8]) -> T,
) -> T {
    let slice = &bytes[rs..re];
    if is_lf_and_utf8_clean(slice) {
        fingerprint(slice)
    } else {
        fingerprint(&canonical_join_bytes(bytes, start, end))
    }
}

/// True when a buffer region can be fingerprinted directly as the canonical
/// content. The fast path is valid only when the region contains no `\r`
/// (CRLF would otherwise leak `\r` bytes that `str::lines` strips) and is
/// valid UTF-8 (otherwise `from_utf8_lossy` would rewrite bytes to U+FFFD
/// before fingerprinting).
fn is_lf_and_utf8_clean(slice: &[u8]) -> bool {
    !slice.contains(&b'\r') && std::str::from_utf8(slice).is_ok()
}

/// The reference canonicalization, materialized: `from_utf8_lossy` the whole
/// buffer, split with `str::lines`, take the inclusive 1-based `[start, end]`
/// slice (clamped to EOF), and `join("\n")`. These are the exact bytes the
/// fingerprint is taken over on the fallback path.
fn canonical_join_bytes(bytes: &[u8], start: u32, end: u32) -> Vec<u8> {
    let text = String::from_utf8_lossy(bytes);
    let lines: Vec<&str> = text.lines().collect();
    let lo = (start as usize).saturating_sub(1);
    let hi = (end as usize).min(lines.len());
    let slice = if lo < hi { &lines[lo..hi] } else { &[][..] };
    slice.join("\n").into_bytes()
}

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
    bytes: &'a [u8],
    /// Start offset of each line.
    starts: Vec<u32>,
    /// Content end (exclusive of the `\n`/`\r\n` terminator) of each line.
    ends: Vec<u32>,
    /// Lazily-computed prefix-hash and power tables for the rolling
    /// fingerprint scan. Populated on first scan of an LF-clean file within
    /// the size threshold. Shared across clones via `Arc` — at most one set
    /// of tables per file per `LineIndex` lifetime.
    ///
    /// Files exceeding [`PREFILTER_TABLES_MAX_BYTES`] never allocate these
    /// tables; the scan falls back to per-window `horner`.
    fp_tables: Arc<OnceLock<PrefixTables>>,
    /// `true` when the buffer contains no `\r` bytes and is valid UTF-8.
    /// Computed once at build time so the scan inner loop avoids re-scanning
    /// the whole buffer per call.
    lf_clean: bool,
}

/// Cached rolling-fingerprint prefix hashes and powers for the file bytes.
/// These are a pure function of the bytes and are computed at most once per
/// `LineIndex` lifetime.
struct PrefixTables {
    ph: Vec<u64>,
    pow: Vec<u64>,
}

/// Files larger than this threshold skip precomputed prefix-hash tables
/// and fall back to per-window `horner` (O(N·S) time, O(1) extra memory).
/// Bounds peak table memory to ~512 MiB (2 × 8 B × 32M). Source-code
/// files (the common anchor target) are virtually always under this limit.
pub const PREFILTER_TABLES_MAX_BYTES: usize = 32 * 1024 * 1024; // 32 MiB

impl<'a> LineIndex<'a> {
    /// Build the line index for `bytes` with one forward newline scan.
    ///
    /// Line offsets are stored as `u32`, so the buffer must be at most
    /// `u32::MAX` (just under 4 GiB) bytes. A larger buffer is **refused**
    /// (panic) rather than indexed with silently truncated offsets: the
    /// contract is fail-closed, and a wrapped offset would produce a
    /// syntactically valid but semantically wrong fingerprint.
    pub fn build(bytes: &'a [u8]) -> LineIndex<'a> {
        assert!(
            bytes.len() <= u32::MAX as usize,
            "rk64: buffer of {} bytes exceeds the supported size of {} bytes \
             (LineIndex stores u32 line offsets); files of 4 GiB or larger are not indexable",
            bytes.len(),
            u32::MAX,
        );
        let mut starts = Vec::new();
        let mut ends = Vec::new();
        let mut seg = 0usize;
        for (i, &b) in bytes.iter().enumerate() {
            if b == b'\n' {
                starts.push(seg as u32);
                ends.push(i as u32);
                seg = i + 1;
            }
        }
        // A trailing segment with no terminating newline is the final,
        // unterminated line; a buffer ending in `\n` has none (matching
        // `str::lines`).
        if seg < bytes.len() {
            starts.push(seg as u32);
            ends.push(bytes.len() as u32);
        }
        let lf_clean = !bytes.contains(&b'\r') && std::str::from_utf8(bytes).is_ok();
        LineIndex {
            bytes,
            starts,
            ends,
            fp_tables: Arc::new(OnceLock::new()),
            lf_clean,
        }
    }

    /// The underlying buffer.
    pub fn bytes(&self) -> &'a [u8] {
        self.bytes
    }

    /// Number of lines, per `str::lines` counting.
    pub fn line_count(&self) -> usize {
        self.starts.len()
    }

    /// Byte offsets `[start, end)` of the canonical region for the inclusive
    /// 1-based line range, clamped to the line count. `None` for an empty
    /// range.
    fn region(&self, start: u32, end: u32) -> Option<(usize, usize)> {
        if start == 0 || start > end {
            // Degenerate extent (`start == 0` or `end < start`): no content,
            // matching `line_range_span` and `line_range_region`.
            return None;
        }
        let lo = start.saturating_sub(1) as usize;
        let hi = (end as usize).min(self.line_count());
        if lo >= hi {
            return None;
        }
        Some((self.starts[lo] as usize, self.ends[hi - 1] as usize))
    }

    /// Returns cached fingerprint tables if the file is within the size
    /// threshold, computing them lazily on first call. Returns `None`
    /// for files exceeding [`PREFILTER_TABLES_MAX_BYTES`], so the caller
    /// falls back to per-window `horner` (O(N·S) time, O(1) memory).
    fn prefilter_tables(&self) -> Option<&PrefixTables> {
        if self.bytes.len() > PREFILTER_TABLES_MAX_BYTES {
            return None;
        }
        Some(self.fp_tables.get_or_init(|| {
            let (ph, pow) = prefix_hashes_and_powers(self.bytes);
            PrefixTables { ph, pow }
        }))
    }
}

/// Build the prefix-hash and power tables for `bytes` in one pass. `ph` has
/// length `bytes.len() + 1` with `ph[0] = 0` and `ph[k+1] = ph[k]·BASE +
/// fp_byte(bytes[k])`; `pow[i] = BASE^i` for `i in 0..=bytes.len()`. Then
/// `horner(bytes[a..b]) == ph[b] - ph[a]·pow[b-a]` over wrapping `u64`.
fn prefix_hashes_and_powers(bytes: &[u8]) -> (Vec<u64>, Vec<u64>) {
    let n = bytes.len();
    let mut ph = Vec::with_capacity(n + 1);
    let mut pow = Vec::with_capacity(n + 1);
    ph.push(0u64);
    pow.push(1u64);
    for (i, &b) in bytes.iter().enumerate() {
        ph.push(ph[i].wrapping_mul(FP_BASE).wrapping_add(fp_byte(b)));
        pow.push(pow[i].wrapping_mul(FP_BASE));
    }
    (ph, pow)
}

/// Cheap fingerprint of an extent's canonical content (whole-file extents
/// fingerprint the full buffer; a range selecting no line fingerprints to `0`).
pub fn cheap_fingerprint_with_extent(bytes: &[u8], extent: &Extent) -> u64 {
    match extent {
        Extent::WholeFile => horner(bytes),
        Extent::LineRange { start, end } => match line_range_region(bytes, *start, *end) {
            Some((rs, re)) => canonical_region(bytes, rs, re, *start, *end, horner),
            None => 0,
        },
    }
}

/// [`cheap_fingerprint_with_extent`] over a prebuilt [`LineIndex`]. Produces an
/// identical `u64`; the only difference is that the newline scan is already
/// paid for.
pub fn cheap_fingerprint_indexed(idx: &LineIndex<'_>, extent: &Extent) -> u64 {
    match extent {
        Extent::WholeFile => horner(idx.bytes),
        Extent::LineRange { start, end } => match idx.region(*start, *end) {
            Some((rs, re)) => canonical_region(idx.bytes, rs, re, *start, *end, horner),
            None => 0,
        },
    }
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
    match extent {
        Extent::WholeFile => {
            whole_file_matches(files, |idx| idx.bytes, |b| horner(b) == cheap_fp)
        }
        Extent::LineRange { start, end } => {
            let span = line_range_span(start, end);
            if span == 0 {
                return Vec::new();
            }
            let mut out: Vec<Location> = Vec::new();
            scan_files(
                files,
                span,
                |n| Some((0, n - span)),
                |path, idx, w, out| {
                    // No content-hash verify: a matching fingerprint is the match.
                    scan_one_file_fp_filtered(path, idx, span, w, cheap_fp, out);
                },
                &mut out,
            );
            if let Some(near) = near {
                sort_near(&mut out, near);
            }
            out
        }
    }
}

/// [`scan_indexed_rk64`] over `Vec<u8>` inputs, building each [`LineIndex`]
/// internally.
pub fn scan_for_content_hash_rk64(
    files: &[(String, Vec<u8>)],
    cheap_fp: u64,
    extent: Extent,
    near: Option<u32>,
) -> Vec<Location> {
    let indexed: Vec<(String, LineIndex<'_>)> = files
        .iter()
        .map(|(path, bytes)| (path.clone(), LineIndex::build(bytes)))
        .collect();
    scan_indexed_rk64(&indexed, cheap_fp, extent, near)
}

/// Nearest-window ordering: stable sort by distance from the 1-based `near`
/// line, ties toward the lower start line. `start_line` and `near` are both
/// 1-based, so the window that starts on the `near` line is distance 0.
fn sort_near(out: &mut [Location], near: u32) {
    out.sort_by_key(|l| (l.start_line.abs_diff(near), l.start_line));
}

/// Emit a whole-file `Location { 0, 0 }` for every file whose bytes satisfy
/// `keep`. `bytes_of` projects each file element to its buffer.
fn whole_file_matches<T>(
    files: &[(String, T)],
    bytes_of: impl Fn(&T) -> &[u8],
    keep: impl Fn(&[u8]) -> bool,
) -> Vec<Location> {
    files
        .iter()
        .filter(|(_, t)| keep(bytes_of(t)))
        .map(|(path, _)| Location {
            path: path.clone(),
            start_line: 0,
            end_line: 0,
        })
        .collect()
}

/// Drive a per-file windowed scan: for each file with enough lines, compute its
/// window bounds and run `scan_one`, accumulating into `out`. `wins` maps a
/// file's line count to its `(win_lo, win_hi)` window range, returning `None`
/// to skip the file.
fn scan_files(
    files: &[(String, LineIndex)],
    span: usize,
    wins: impl Fn(usize) -> Option<(usize, usize)>,
    mut scan_one: impl FnMut(&str, &LineIndex, (usize, usize), &mut Vec<Location>),
    out: &mut Vec<Location>,
) {
    for (path, idx) in files {
        let n = idx.line_count();
        if n < span {
            continue;
        }
        let Some(w) = wins(n) else { continue };
        scan_one(path, idx, w, out);
    }
}

/// Scan one file's `span`-high windows, emitting a [`Location`] for every
/// window whose rolling polynomial fingerprint equals `cheap_fp`. On the
/// LF-and-UTF-8 fast path a single prefix-hash pass over the buffer makes each
/// window's fingerprint an O(1) subtraction; `\r`/non-UTF-8 files fingerprint
/// the canonical lossy join per window so results stay byte-identical to the
/// reference matcher.
fn scan_one_file_fp_filtered(
    path: &str,
    idx: &LineIndex,
    span: usize,
    wins: (usize, usize),
    cheap_fp: u64,
    out: &mut Vec<Location>,
) {
    let (win_lo, win_hi) = wins;
    let bytes = idx.bytes;
    let simple = idx.lf_clean;

    if simple {
        // Prefix hashes `ph[k] = horner(bytes[0..k])` and powers `pow[i] =
        // BASE^i` give every window's fingerprint as `ph[re] - ph[rs]·pow[re-rs]`
        // in O(1) — the rolling reduction of recomputing `horner` per window.
        // For files under the size threshold the tables are cached on the
        // `LineIndex`; larger files fall back to per-window `horner`.
        let tables = idx.prefilter_tables();
        for win in win_lo..=win_hi {
            let rs = idx.starts[win] as usize;
            let re = idx.ends[win + span - 1] as usize;
            let fp = match tables {
                Some(t) => t.ph[re].wrapping_sub(t.ph[rs].wrapping_mul(t.pow[re - rs])),
                None => horner(&bytes[rs..re]),
            };
            if fp == cheap_fp {
                out.push(Location {
                    path: path.to_string(),
                    start_line: (win as u32) + 1,
                    end_line: (win as u32) + span as u32,
                });
            }
        }
    } else {
        let text = String::from_utf8_lossy(bytes);
        let lines: Vec<&str> = text.lines().collect();
        for win in win_lo..=win_hi {
            let joined = lines[win..win + span].join("\n");
            if horner(joined.as_bytes()) == cheap_fp {
                out.push(Location {
                    path: path.to_string(),
                    start_line: (win as u32) + 1,
                    end_line: (win as u32) + span as u32,
                });
            }
        }
    }
}

/// Canonical hex encoding of an rk64 fingerprint: **lowercase, zero-padded to
/// 16 digits, big-endian** (most-significant nibble first), i.e.
/// `format!("{fp:016x}")`. Pair with [`rk64_from_hex`] so a writer and any
/// reader agree on the exact bytes.
pub fn rk64_to_hex(fp: u64) -> String {
    format!("{fp:016x}")
}

/// Parse the canonical [`rk64_to_hex`] encoding back to a `u64`. Returns
/// `None` for anything other than exactly 16 lowercase hex digits, so a
/// malformed or non-canonical token is rejected rather than silently
/// mis-decoded.
pub fn rk64_from_hex(s: &str) -> Option<u64> {
    if s.len() != 16 || !s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    u64::from_str_radix(s, 16).ok()
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
            assert!(
                (entry["start"].is_null() && entry["end"].is_null())
                    || (entry["start"].is_number() && entry["end"].is_number()),
                "entry {i}: malformed extent"
            );
            // The embedded content IS the canonical content — for ranges, the
            // already-split/clamped/joined slice the reference fingerprinted.
            // Hashing it whole-file is exactly the reference's canonical
            // fingerprint; re-applying the original range to the joined bytes
            // would re-slice and canonically empty it. The kernel's own
            // line-range canonicalization is proven by the edge tests above.
            let actual = rk64_to_hex(cheap_fingerprint_with_extent(&content, &Extent::WholeFile));
            assert_eq!(
                actual, stored,
                "entry {i} ({path}, {extent:?}): kernel disagrees with the stored pair",
                extent = entry["start"].as_u64().map(|s| format!(
                    "L{s}-L{}",
                    entry["end"].as_u64().unwrap()
                ))
                .unwrap_or_else(|| "whole-file".into()),
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
