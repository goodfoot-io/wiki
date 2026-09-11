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
    /// Per-line canonical prefix hashes — the window-fingerprint tables
    /// ([`CanonicalLines`]). Populated lazily on the first windowed scan and
    /// shared across clones via `Arc`.
    ///
    /// The tables are cut on line boundaries rather than on bytes: at 24 bytes
    /// per *line* they cost a fraction of a per-byte table, and — because
    /// [`CanonicalLines::window_fp`] needs no bytes at all — the same
    /// structure is what [`ScanIndex`] caches to serve many windows of one
    /// buffer without rescanning it.
    canonical: Arc<OnceLock<CanonicalLines>>,
}

/// Canonical per-line prefix hashes for a buffer that is **not** LF-clean.
///
/// Such a buffer's canonical content is the lossy UTF-8 text with `\r\n` (and
/// any lone `\r`) normalized away, so a window's canonical content is
/// `lines[a..b].join("\n")` — not a byte slice of the buffer, and therefore
/// out of reach of any prefix sum over the buffer's own bytes. Rebuilding that
/// string per window instead costs O(span) per window, i.e. O(lines × span)
/// per file: a tracked binary of a few megabytes with thousands of newline
/// bytes took seconds on its own.
///
/// Three per-line arrays make every window O(1) instead, by the same
/// substring identity the per-byte tables use — `horner(canon[u..v]) =
/// P(v) − P(u)·BASE^(v−u)` for `P` the Horner hash of the canonical prefix —
/// with the prefix hashes sampled at the line boundaries that windows are cut
/// on, and the exponent supplied by [`pow_base`].
///
/// For `n` lines the canonical text is
/// `line[0] + "\n" + … + "\n" + line[n-1]`; `offsets[i]` is the byte offset of
/// line `i` in it, with the virtual terminating newline that makes
/// `offsets[i+1] = offsets[i] + line[i].len() + 1` hold for every `i`.
#[derive(Clone)]
pub(crate) struct CanonicalLines {
    /// `starts[i]`: the canonical prefix hash at the start of line `i`.
    starts: Vec<u64>,
    /// `ends[i]`: the canonical prefix hash at the end of line `i`'s content,
    /// before the newline the join inserts.
    ends: Vec<u64>,
    /// `offsets[i]`: the byte offset of line `i` in the canonical text, with
    /// one past-the-end entry for `i == n`.
    offsets: Vec<u64>,
}

impl CanonicalLines {
    /// Build the per-line prefix hashes of `bytes`' canonical text — the
    /// lossy UTF-8 reading with `\r\n` (and any lone `\r`) normalized away —
    /// in one pass, one multiplication per byte.
    ///
    /// Every buffer goes through here, LF-clean or not: for a buffer whose
    /// canonical content *is* a byte slice of itself the arrays describe that
    /// same slice, so the one structure serves both and the window
    /// fingerprint has a single definition.
    fn build(bytes: &[u8]) -> CanonicalLines {
        let text = String::from_utf8_lossy(bytes);
        let lines: Vec<&str> = text.lines().collect();
        let mut starts = Vec::with_capacity(lines.len());
        let mut ends = Vec::with_capacity(lines.len());
        let mut offsets = Vec::with_capacity(lines.len() + 1);
        let mut h = 0u64;
        let mut off = 0u64;
        for line in &lines {
            starts.push(h);
            offsets.push(off);
            for &b in line.as_bytes() {
                h = h.wrapping_mul(FP_BASE).wrapping_add(fp_byte(b));
            }
            ends.push(h);
            // The newline the join inserts between lines — present in the
            // canonical offsets even after the last line, where it only
            // makes the offsets a uniform coordinate.
            h = h.wrapping_mul(FP_BASE).wrapping_add(fp_byte(b'\n'));
            off += line.len() as u64 + 1;
        }
        offsets.push(off);
        CanonicalLines {
            starts,
            ends,
            offsets,
        }
    }

    /// Number of lines, per `str::lines` counting.
    fn line_count(&self) -> usize {
        self.starts.len()
    }

    /// The fingerprint of the `span`-high window starting at line `win`
    /// (0-based) — the canonical content `lines[win..win+span].join("\n")`,
    /// which is `canon[offsets[win] .. offsets[win+span] - 1]`.
    fn window_fp(&self, win: usize, span: usize) -> u64 {
        let lo = self.offsets[win];
        let hi = self.offsets[win + span] - 1;
        self.ends[win + span - 1].wrapping_sub(self.starts[win].wrapping_mul(pow_base(hi - lo)))
    }
}

/// The per-file facts a windowed scan needs, and the seam that lets one index
/// be built once and scanned many times ([`ScanIndex`]) or built on the spot
/// for a single query ([`LineIndex`]) without the kernel knowing which it has.
pub(crate) trait Scannable {
    /// Number of lines, per `str::lines` counting.
    fn line_count(&self) -> usize;
    /// The window-fingerprint tables.
    fn canon(&self) -> &CanonicalLines;
    /// `horner` of the buffer itself, for whole-file extents (which
    /// fingerprint the raw bytes, not the canonical text).
    fn whole_fp(&self) -> u64;
}

impl Scannable for LineIndex<'_> {
    fn line_count(&self) -> usize {
        LineIndex::line_count(self)
    }

    fn canon(&self) -> &CanonicalLines {
        self.canonical_lines()
    }

    fn whole_fp(&self) -> u64 {
        horner(self.bytes)
    }
}

/// An owned scan index over one buffer: its canonical window fingerprints,
/// built once and reused by every query against the same bytes.
///
/// A pure function of the buffer — it borrows nothing, so it outlives the
/// bytes it was built from and may cross the worker pool. That is the whole
/// point: an inventory-wide move scan asks the same question of the same
/// several thousand files once per link, and the per-query cost of indexing
/// them (one newline scan and one canonical pass **per file**) is what made
/// those scans cost seconds. Indexed once per pass, each query is window
/// arithmetic over tables already in cache.
#[derive(Clone)]
pub struct ScanIndex {
    canon: CanonicalLines,
    whole_fp: u64,
}

impl ScanIndex {
    /// Build the index for `bytes`: the canonical tables, plus the buffer's
    /// own hash for whole-file extents.
    pub fn build(bytes: &[u8]) -> ScanIndex {
        ScanIndex {
            canon: CanonicalLines::build(bytes),
            whole_fp: horner(bytes),
        }
    }
}

impl Scannable for ScanIndex {
    fn line_count(&self) -> usize {
        self.canon.line_count()
    }

    fn canon(&self) -> &CanonicalLines {
        &self.canon
    }

    fn whole_fp(&self) -> u64 {
        self.whole_fp
    }
}

/// `BASE^e` over wrapping `u64` by square-and-multiply, from a cached table of
/// the 64 power-of-two powers — the arbitrary-exponent counterpart of a
/// per-byte power table. An exponent of zero is `1`, the empty-content hash's
/// multiplier.
fn pow_base(e: u64) -> u64 {
    static POW2: OnceLock<[u64; 64]> = OnceLock::new();
    let pow2 = POW2.get_or_init(|| {
        let mut pow2 = [1u64; 64];
        pow2[0] = FP_BASE;
        for k in 1..64 {
            pow2[k] = pow2[k - 1].wrapping_mul(pow2[k - 1]);
        }
        pow2
    });
    let mut out = 1u64;
    let mut rest = e;
    let mut k = 0usize;
    while rest != 0 {
        if rest & 1 == 1 {
            out = out.wrapping_mul(pow2[k]);
        }
        rest >>= 1;
        k += 1;
    }
    out
}

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
        LineIndex {
            bytes,
            starts,
            ends,
            canonical: Arc::new(OnceLock::new()),
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

    /// The canonical per-line prefix hashes, computed lazily on the first
    /// windowed scan that needs them. One pass over the lossy text, one
    /// multiplication per byte.
    fn canonical_lines(&self) -> &CanonicalLines {
        self.canonical.get_or_init(|| {
            let canon = CanonicalLines::build(self.bytes);
            // `str::lines` and the newline scan that built `starts`/`ends`
            // count lines identically (a `\n` ends a line, a trailing
            // terminator yields no final empty line), so the window bounds
            // derived from `line_count` index these arrays exactly.
            assert_eq!(
                canon.line_count(),
                self.starts.len(),
                "rk64: canonical line count disagrees with the newline scan",
            );
            canon
        })
    }
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
pub fn scan_indexed_rk64<T: Scannable>(
    files: &[(String, T)],
    cheap_fp: u64,
    extent: Extent,
    near: Option<u32>,
) -> Vec<Location> {
    let mut out: Vec<Location> = Vec::new();
    for (path, idx) in files {
        scan_one_indexed(path, idx, cheap_fp, extent, &mut out);
    }
    if let Some(near) = near {
        sort_near(&mut out, near);
    }
    out
}

/// [`scan_indexed_rk64`] over borrowed path/bytes pairs, indexing each buffer
/// with the on-the-spot [`LineIndex`]. Generic over the borrowed forms so
/// callers with `Vec`-backed inventories need no per-call clones.
pub fn scan_for_content_hash_rk64<P: AsRef<str>, B: AsRef<[u8]>>(
    files: &[(P, B)],
    cheap_fp: u64,
    extent: Extent,
    near: Option<u32>,
) -> Vec<Location> {
    let indexed: Vec<(String, LineIndex<'_>)> = files
        .iter()
        .map(|(path, bytes)| (path.as_ref().to_string(), LineIndex::build(bytes.as_ref())))
        .collect();
    scan_indexed_rk64(&indexed, cheap_fp, extent, near)
}

/// One file's contribution to a scan, for any indexed file: whole-file
/// extents match the buffer's own hash, line ranges scan every window of the
/// requested span. No content-hash verify: a matching fingerprint is the
/// match.
///
/// This is the unit a caller drives across workers when it holds an indexed
/// inventory: the semantics are identical to [`scan_indexed_rk64`]'s per-file
/// step, so a worker's output concatenated in inventory order is
/// byte-identical to the whole-inventory scan's.
pub fn scan_one_indexed<T: Scannable>(
    path: &str,
    idx: &T,
    cheap_fp: u64,
    extent: Extent,
    out: &mut Vec<Location>,
) {
    match extent {
        Extent::WholeFile => {
            if idx.whole_fp() == cheap_fp {
                out.push(Location {
                    path: path.to_string(),
                    start_line: 0,
                    end_line: 0,
                });
            }
        }
        Extent::LineRange { start, end } => {
            let span = line_range_span(start, end);
            if span == 0 {
                return;
            }
            let n = idx.line_count();
            if n < span {
                return;
            }
            // No content-hash verify: a matching fingerprint is the match.
            scan_windows(path, idx.canon(), span, (0, n - span), cheap_fp, out);
        }
    }
}

/// Nearest-window ordering: stable sort by distance from the 1-based `near`
/// line, ties toward the lower start line. `start_line` and `near` are both
/// 1-based, so the window that starts on the `near` line is distance 0.
fn sort_near(out: &mut [Location], near: u32) {
    out.sort_by_key(|l| (l.start_line.abs_diff(near), l.start_line));
}

/// Scan one buffer's `span`-high windows, emitting a [`Location`] for every
/// window whose rolling polynomial fingerprint equals `cheap_fp`.
///
/// One kernel for every buffer shape: the window's fingerprint is
/// `ends[win+span-1] − starts[win]·BASE^len` over the canonical per-line
/// prefix hashes, which is O(1) per window and byte-identical to the
/// reference matcher's `lines[a..b].join("\n")` for both LF-clean buffers and
/// the `\r`/non-UTF-8 shapes [`CanonicalLines`] normalizes.
fn scan_windows(
    path: &str,
    canon: &CanonicalLines,
    span: usize,
    wins: (usize, usize),
    cheap_fp: u64,
    out: &mut Vec<Location>,
) {
    let (win_lo, win_hi) = wins;
    for win in win_lo..=win_hi {
        if canon.window_fp(win, span) == cheap_fp {
            out.push(Location {
                path: path.to_string(),
                start_line: (win as u32) + 1,
                end_line: (win as u32) + span as u32,
            });
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

    /// The brute-force reference for the non-LF-clean scan: for every window
    /// of every file, fingerprint the canonical content the way the canonical
    /// form defines it — `lines[a..b].join("\n")` — and keep the windows whose
    /// fingerprint matches. This is the per-window `join` the scan performs
    /// no longer, kept as the oracle [`scan_for_content_hash_rk64`] must
    /// agree with byte-for-byte on every buffer shape.
    fn join_oracle_hits(files: &[(String, Vec<u8>)], cheap_fp: u64, span: usize) -> Vec<Location> {
        let mut out = Vec::new();
        for (path, bytes) in files {
            let text = String::from_utf8_lossy(bytes);
            let lines: Vec<&str> = text.lines().collect();
            if lines.len() < span {
                continue;
            }
            for win in 0..=lines.len() - span {
                let joined = lines[win..win + span].join("\n");
                if horner(joined.as_bytes()) == cheap_fp {
                    out.push(Location {
                        path: path.clone(),
                        start_line: (win as u32) + 1,
                        end_line: (win as u32) + span as u32,
                    });
                }
            }
        }
        out
    }

    /// Buffer shapes the canonical path must handle: CRLF, lone `\r`, CRLF
    /// with no trailing newline, invalid UTF-8 (lossy replacement), a lone
    /// newline, an empty buffer, and a buffer with no newline at all.
    fn non_lf_clean_fixtures() -> Vec<Vec<u8>> {
        vec![
            b"a\r\nb\r\nc\r\nd\r\n".to_vec(),
            b"a\r\nb\r\nc".to_vec(),
            b"\r\n".to_vec(),
            b"a\rb\rc\r".to_vec(),
            b"\xff\xfe alpha\n\xc3 beta\n gamma \xf0\x9f".to_vec(),
            b"\n".to_vec(),
            b"".to_vec(),
            b"no newline here".to_vec(),
            b"\xff\n\xfe\n\xfd\n".to_vec(),
        ]
    }

    #[test]
    fn canonical_scan_agrees_with_the_join_oracle_on_every_buffer_shape() {
        for bytes in non_lf_clean_fixtures() {
            // Every span the buffer can hold, and a few it cannot.
            for span in 1..=5 {
                let files = vec![("f".to_string(), bytes.clone())];
                // Every window's own canonical fingerprint, plus one that
                // matches nothing: the scan must return exactly the windows
                // the oracle finds, in the same order.
                let text = String::from_utf8_lossy(&bytes);
                let lines: Vec<&str> = text.lines().collect();
                let mut probes: Vec<u64> = Vec::new();
                if lines.len() >= span {
                    probes.extend(
                        (0..=lines.len() - span)
                            .map(|win| horner(lines[win..win + span].join("\n").as_bytes())),
                    );
                    probes.push(0xdead_beef_dead_beef);
                }
                for fp in probes {
                    let extent = Extent::LineRange {
                        start: 1,
                        end: span as u32,
                    };
                    assert_eq!(
                        scan_for_content_hash_rk64(&files, fp, extent, None),
                        join_oracle_hits(&files, fp, span),
                        "buffer {bytes:?} span {span} fp {fp:#x}",
                    );
                }
            }
        }
    }

    /// The per-file unit must compose: driving [`scan_one_indexed`] over an
    /// indexed inventory one file at a time, concatenated in inventory order,
    /// has to equal the whole-inventory scan — the contract callers that
    /// spread the scan across workers rely on.
    #[test]
    fn per_file_scan_concatenates_to_the_whole_inventory_scan() {
        let files: Vec<(String, Vec<u8>)> = vec![
            ("a.txt".to_string(), b"alpha\nbeta\ngamma\ndelta\n".to_vec()),
            (
                "b.txt".to_string(),
                b"alpha\r\nbeta\r\ngamma\r\n".to_vec(),
            ),
            ("c.txt".to_string(), b"alpha\nbeta\n".to_vec()),
            ("d.txt".to_string(), b"\xff\n\xfe\nalpha\nbeta\n".to_vec()),
            ("e.txt".to_string(), b"".to_vec()),
            ("f.txt".to_string(), b"only one line\n".to_vec()),
        ];
        let indexed: Vec<(String, ScanIndex)> = files
            .iter()
            .map(|(path, bytes)| (path.clone(), ScanIndex::build(bytes)))
            .collect();
        for span in 1..=4u32 {
            let extent = Extent::LineRange { start: 1, end: span };
            for fp in [
                cheap_fingerprint_with_extent(b"alpha\nbeta", &Extent::WholeFile),
                cheap_fingerprint_with_extent(b"alpha", &Extent::WholeFile),
                cheap_fingerprint_with_extent(b"gamma", &Extent::WholeFile),
                0xdead_beef_dead_beef,
            ] {
                let whole = scan_indexed_rk64(&indexed, fp, extent, None);
                let mut per_file: Vec<Location> = Vec::new();
                for (path, idx) in &indexed {
                    scan_one_indexed(path, idx, fp, extent, &mut per_file);
                }
                assert_eq!(
                    per_file, whole,
                    "span {span} fp {fp:#x}: per-file scan must concatenate to \
                     the whole-inventory scan",
                );
            }
        }
        // Whole-file extents take the same contract.
        let fp = cheap_fingerprint_with_extent(b"alpha\nbeta\n", &Extent::WholeFile);
        let whole = scan_indexed_rk64(&indexed, fp, Extent::WholeFile, None);
        let mut per_file: Vec<Location> = Vec::new();
        for (path, idx) in &indexed {
            scan_one_indexed(path, idx, fp, Extent::WholeFile, &mut per_file);
        }
        assert_eq!(per_file, whole);
    }

    /// The owned [`ScanIndex`] and the on-the-spot [`LineIndex`] must be
    /// interchangeable: a scan over an inventory indexed once has to return
    /// exactly what the same scan returns when every query indexes the buffer
    /// again. Every buffer shape, every span, every probe — a real match and a
    /// miss — and both extents.
    #[test]
    fn owned_index_scans_identically_to_the_on_the_spot_index() {
        let mut fixtures = non_lf_clean_fixtures();
        fixtures.push(b"a\nb\nc\nd\n".to_vec()); // LF-clean
        for bytes in fixtures {
            let indexed = vec![("f.txt".to_string(), ScanIndex::build(&bytes))];
            let on_the_spot = vec![("f.txt".to_string(), bytes.clone())];
            let text = String::from_utf8_lossy(&bytes);
            let lines: Vec<&str> = text.lines().collect();
            for span in 1..=5usize {
                let mut probes = vec![0xdead_beef_dead_beef, horner(&bytes)];
                if lines.len() >= span {
                    probes.extend(
                        (0..=lines.len() - span)
                            .map(|win| horner(lines[win..win + span].join("\n").as_bytes())),
                    );
                }
                for fp in probes {
                    for extent in [
                        Extent::LineRange {
                            start: 1,
                            end: span as u32,
                        },
                        Extent::WholeFile,
                    ] {
                        assert_eq!(
                            scan_indexed_rk64(&indexed, fp, extent, None),
                            scan_for_content_hash_rk64(&on_the_spot, fp, extent, None),
                            "buffer {bytes:?} span {span} fp {fp:#x} extent {extent:?}",
                        );
                    }
                }
            }
        }
    }

    /// A [`ScanIndex`] borrows nothing, so the move scan's workers share one
    /// indexed inventory between them: the index must be `Send + Sync`, and
    /// scanning it concurrently must yield exactly the serial scan's hits, in
    /// inventory order.
    #[test]
    fn scan_index_crosses_the_worker_pool() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ScanIndex>();

        let indexed: Vec<(String, ScanIndex)> = non_lf_clean_fixtures()
            .into_iter()
            .enumerate()
            .map(|(i, bytes)| (format!("f{i}.txt"), ScanIndex::build(&bytes)))
            .collect();
        let extent = Extent::LineRange { start: 1, end: 1 };
        // Several fixtures canonicalize to a line `c` (with and without a
        // trailing newline, CRLF and LF alike), so the probe hits more than
        // one file and the comparison is not trivially empty on both sides.
        let fp = horner(b"c");
        let serial = scan_indexed_rk64(&indexed, fp, extent, None);
        assert!(serial.len() >= 2, "the probe must hit several files");

        let concurrent: Vec<Location> = std::thread::scope(|scope| {
            let workers: Vec<_> = indexed
                .iter()
                .map(|(path, idx)| {
                    scope.spawn(|| {
                        let mut out = Vec::new();
                        scan_one_indexed(path, idx, fp, extent, &mut out);
                        out
                    })
                })
                .collect();
            workers
                .into_iter()
                .flat_map(|worker| worker.join().expect("scan worker panicked"))
                .collect()
        });
        assert_eq!(concurrent, serial);
    }

    /// The line-window hash the per-line prefix arrays compute must equal the
    /// Horner hash of the joined window for arbitrary line mixes — including
    /// windows whose lines have very different lengths, where an exponent or
    /// offset error would show up.
    #[test]
    fn canonical_window_hashes_match_joined_content_across_line_lengths() {
        let bytes = b"x\nyy\n\r\nzzz\r\nlong line with several tokens\n \nq\r\n".to_vec();
        let idx = LineIndex::build(&bytes);
        assert!(
            bytes.windows(2).any(|w| w == b"\r\n"),
            "fixture must exercise the canonical path: a window's content is \
             the lines joined by `\\n`, not a byte slice of the buffer",
        );
        let canon = idx.canonical_lines();
        let text = String::from_utf8_lossy(&bytes);
        let lines: Vec<&str> = text.lines().collect();
        let mut checked = 0usize;
        for span in 1..=lines.len() {
            for win in 0..=lines.len() - span {
                assert_eq!(
                    canon.window_fp(win, span),
                    horner(lines[win..win + span].join("\n").as_bytes()),
                    "window {win}..{}",
                    win + span,
                );
                checked += 1;
            }
        }
        assert!(checked > 0, "the fixture must yield at least one window");
    }
}
