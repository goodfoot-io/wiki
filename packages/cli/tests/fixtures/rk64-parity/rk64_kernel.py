#!/usr/bin/env python3
"""rk64 reference kernel, reimplemented in Python.

Rules (identical in the git-mesh-core crate this kernel replaced):

  - LINE RANGE (inclusive 1-based [start, end]):
      read file bytes; UTF-8 lossy decode; split with Rust `str::lines`
      semantics (split on '\n'; strip one trailing '\r' per element; no
      trailing empty line for buffers ending in '\n'); slice
      lines[start-1:end] clamped to EOF; join with '\n'; UTF-8 encode.
      Range selecting no line (start == 0, end < start, start-1 >=
      line_count) -> fingerprint 0.
  - WHOLE FILE: hash the RAW file bytes (no canonicalization).
  - rk64: h = 0; for each byte b: h = (h * 0x0000_0100_0000_01b3 + (b + 1))
      mod 2^64 (wrapping u64). Empty input -> 0. Hex: lowercase, 16 digits.

Consumed by make_fixture.py in this directory; kept free of I/O so the
kernel rules stay independently readable against packages/cli/src/rk64.rs.
"""

import re

ANCHOR_RE = re.compile(
    r"^(?P<path>\S+?)(?:#L(?P<start>\d+)-L(?P<end>\d+))? rk64:(?P<hex>[0-9a-f]{16})$"
)


def rust_lines(text: str) -> list[str]:
    """Split with Rust str::lines semantics."""
    parts = text.split("\n")
    if text.endswith("\n"):
        parts = parts[:-1]  # no trailing empty line
    return [p[:-1] if p.endswith("\r") else p for p in parts]


def horner(data: bytes) -> int:
    h = 0
    for b in data:
        h = (h * 0x0000_0100_0000_01B3 + (b + 1)) & 0xFFFFFFFFFFFFFFFF
    return h


def rk64_hex(data: bytes) -> str:
    return format(horner(data), "016x")


def range_fingerprint(file_bytes: bytes, start: int, end: int) -> tuple[int, str]:
    """Return (fingerprint, note) for a line range.

    note is 'empty-range' when the selection is empty (fingerprint 0),
    'clamped' when end exceeds the line count, else ''.
    """
    if start == 0 or end < start:
        return 0, "empty-range"
    text = file_bytes.decode("utf-8", errors="replace")
    lines = rust_lines(text)
    if start - 1 >= len(lines):
        return 0, "empty-range"
    selected = lines[start - 1 : end]  # Python slice clamps to EOF
    canonical = "\n".join(selected).encode("utf-8")
    note = "clamped" if end > len(lines) else ""
    return horner(canonical), note
