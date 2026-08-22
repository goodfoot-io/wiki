#!/usr/bin/env python3
"""Regenerate packages/cli/tests/fixtures/rk64-parity.json.

Usage (from anywhere inside a checkout of this repository):

    python3 packages/cli/tests/fixtures/rk64-parity/make_fixture.py

See README.md beside this script for the full procedure. The fixture pins
every stored rk64 anchor pair from the .wiki/ corpus as it existed at the
baseline commit, with canonical content embedded (hex) so the parity gate in
packages/cli/src/rk64.rs validates the vendored kernel without reading live
files — the corpus was deleted after migration by design, and future page
edits must never break the byte-parity proof.

All target content is read from git history at the pinned baseline — never
the working tree. For each anchor: fingerprint the target's canonical content
at the baseline tree; when it equals the stored value the entry is `current`
(content embedded from baseline); otherwise (the 21 by-design-stale anchors)
walk `git log --follow` oldest-to-newest for the oldest commit at which it
matches and embed that content as `historical` with the commit sha.
"""

import json
import pathlib
import subprocess
import sys

from rk64_kernel import ANCHOR_RE, horner, range_fingerprint, rust_lines

assert (
    pathlib.Path(__file__).resolve().parents[4].name == "packages"
), "make_fixture.py must live at <repo>/packages/cli/tests/fixtures/rk64-parity/"
REPO_ROOT = pathlib.Path(__file__).resolve().parents[5]

# The commit whose .wiki/ corpus the fixture pins. BASELINE_REF is a local tag
# created when this generator was committed; if it is absent (e.g. a clone
# made before the tag was pushed) or has moved, fall back to the bare SHA —
# same object, same bytes.
BASELINE_SHA = "a1ef5b2cd8c60c73227d52de553c658b3d2a0943"
BASELINE_REF = "rk64-parity-baseline"
GENERATED_FROM = f"{BASELINE_REF} ({BASELINE_SHA})"
GENERATOR_PATH = "packages/cli/tests/fixtures/rk64-parity/make_fixture.py"

FIXTURE = REPO_ROOT / "packages/cli/tests/fixtures/rk64-parity.json"

# Non-anchor files living under .wiki/ (git plumbing, sqlite index, logs)
SKIP_NAMES = {
    ".gitattributes",
    ".wikiignore",
    "wiki-index.sqlite",
    "wiki.log",
    "wiki-refresh.lock",
}


def resolve_baseline() -> str:
    out = subprocess.run(
        ["git", "rev-parse", "--verify", "--quiet", f"{BASELINE_REF}^{{commit}}"],
        capture_output=True,
        cwd=REPO_ROOT,
    )
    if out.returncode == 0 and out.stdout.decode().strip() == BASELINE_SHA:
        return BASELINE_REF
    return BASELINE_SHA


def git(args: list[str]) -> bytes:
    out = subprocess.run(
        ["git"] + args, capture_output=True, cwd=REPO_ROOT
    )
    if out.returncode != 0:
        raise RuntimeError(f"git {' '.join(args)} failed: {out.stderr!r}")
    return out.stdout


def git_show(commit: str, path: str) -> bytes | None:
    out = subprocess.run(
        ["git", "show", f"{commit}:{path}"],
        capture_output=True,
        cwd=REPO_ROOT,
    )
    if out.returncode != 0:
        return None
    return out.stdout


def canonical_range(file_bytes: bytes, start: int, end: int) -> tuple[bytes, int]:
    fp, note = range_fingerprint(file_bytes, start, end)
    if note == "empty-range":
        # Historical commits may predate the anchored range; the reference
        # kernel fingerprints that selection as 0 (empty content).
        return b"", fp
    text = file_bytes.decode("utf-8", errors="replace")
    lines = rust_lines(text)
    return "\n".join(lines[start - 1 : end]).encode("utf-8"), fp


def main() -> int:
    baseline = resolve_baseline()
    tree = git(
        ["ls-tree", "-r", "--name-only", baseline, ".wiki/"]
    ).decode().splitlines()
    anchor_files = sorted(
        p for p in tree if pathlib.Path(p).name not in SKIP_NAMES
    )

    entries = []
    current_kind = 0
    historical_kind = 0
    for af in anchor_files:
        text = git_show(baseline, af).decode("utf-8", errors="replace")
        for raw in text.split("\n"):
            if raw == "":
                break  # trailing prose after a blank line is not anchors
            m = ANCHOR_RE.match(raw)
            if m is None:
                print(f"PARSE-SKIP {af}: {raw!r}")
                continue
            target = m.group("path")
            stored = m.group("hex")
            start = int(m.group("start")) if m.group("start") else None
            end = int(m.group("end")) if m.group("end") else None

            baseline_bytes = git_show(baseline, target)
            assert baseline_bytes is not None, f"missing at baseline: {target}"

            if start is None:
                content = baseline_bytes
                fp = horner(content)
            else:
                content, fp = canonical_range(baseline_bytes, start, end)

            entry = {
                "anchor_file": af,
                "path": target,
                "start": start,
                "end": end,
                "stored": stored,
                "kind": "current",
                "historical_commit": None,
                "content_hex": content.hex(),
            }

            if format(fp, "016x") == stored:
                current_kind += 1
                entries.append(entry)
                continue

            # Stale: find the oldest commit at which the stored value matches.
            log = git(
                ["log", "--format=%H", "--follow", "--", target]
            ).decode().splitlines()
            found = None
            for sha in reversed(log):  # oldest → newest
                hist = git_show(sha, target)
                if hist is None:
                    continue
                if start is None:
                    hfp = horner(hist)
                    hcontent = hist
                else:
                    hcontent, hfp = canonical_range(hist, start, end)
                if format(hfp, "016x") == stored:
                    found = (sha, hcontent)
                    break
            assert found is not None, (
                f"stored {stored} for {target} matches no historical commit"
            )
            sha, hcontent = found
            entry["kind"] = "historical"
            entry["historical_commit"] = sha
            entry["content_hex"] = hcontent.hex()
            historical_kind += 1
            entries.append(entry)

    doc = {
        "generated_from": GENERATED_FROM,
        "generator": GENERATOR_PATH,
        "note": (
            "Every stored rk64 anchor pair from the .wiki/ corpus at the "
            "baseline, with canonical content embedded. `current` entries "
            "match the target's content at baseline; `historical` entries "
            "(the by-design-stale anchors) match at their oldest matching "
            "historical commit."
        ),
        "entries": entries,
    }
    FIXTURE.write_text(json.dumps(doc, indent=2) + "\n")
    print(
        f"wrote {FIXTURE} with {len(entries)} entries "
        f"({current_kind} current, {historical_kind} historical)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
