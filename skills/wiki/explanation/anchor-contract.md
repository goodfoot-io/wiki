---
title: The Anchor Contract Explained
summary: Why certification is per-page epochs over rk64 fingerprints, why relocation demands rename evidence, and why there is no reanchor verb.
tags: [wiki, explanation]
---

Line-range links claim "this prose describes these bytes." That claim rots silently under ordinary development, so the CLI makes rot detectable instead of trusting authors to notice.

## Epochs, not timestamps

Each page's `links-reviewed:` value is an **anchor epoch**. `wiki check` resolves the commit where the value last changed and hashes every cited range as it existed there (rk64 fingerprints). A current link is then healthy (bytes match), moved (bytes found elsewhere via history), or drifted (same place, different bytes).

Per-page whole-file certification is deliberate: reviewing one link usually means reading its surrounding prose anyway, so per-link bookkeeping would cost more than it saves and would invite partial re-certifications that hide drift on the same page.

## Suppression during review

While a bump sits uncommitted (worktree value ≠ HEAD), certification outcomes are suppressed and only structural breakage is flagged. An in-progress re-certification therefore never blocks on the very links being reviewed — but it also never certifies them early.

## Quotes are not moves

A cited range's bytes appearing in an unrelated file is treated as a quote, never as a move. Relocation requires rename-tracked git-history identity linking destination back to source. This is the difference between "the article followed the code" and "someone pasted similar code elsewhere" — first-hit-wins matching would make the wrong one indistinguishable.

## Fail closed

Ambiguity degrades to reported failure, never silent success: unverifiable pairings, shallow clones, unparseable YAML all hard-error. A green check must mean reviewed; anything less must be loud.

## No reanchor verb

Re-pointing a link is editing the page; re-certifying is asserting you read the change. Both are author judgments about prose, so both live in page edits — the CLI only initializes the field when absent, and refuses to guess otherwise.
