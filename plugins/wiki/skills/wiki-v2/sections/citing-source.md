---
title: Citing Source From A Wiki Page
summary: Fragment-link grammar and discipline, the links-reviewed anchor contract, and every classification a line-range link can receive.
tags: [wiki, how-to]
links-reviewed: 1
---

A fragment link points from a page to a source file; it is the unit of drift detection, so **always include a line range** ([range grammar](/packages/cli/src/parser.rs#L277-L304)):

```markdown
The retry loop lives in [client.ts](../packages/api/client.ts#L88-L120).
```

Whole-file links are valid but blind — no range, no drift signal. Use a range whenever the cited thing has one.

## Discipline

- **If you mention it, link it.** Every function, type, schema, constant, or module named in prose gets a fragment link to its definition.
- **Link definitions, not call sites**, spanning opening line through closing brace.
- Orphans collect under `## References` rather than going uncited.

## The anchor contract

One frontmatter field certifies every line-range link on the page together:

```yaml
links-reviewed: 1
```

The value is the page's **anchor epoch**. `wiki check` walks the page's full git history (`--follow`) to the commit where that value last changed — the **anchor commit** — hashes each cited range as it was there (rk64 fingerprints), and classifies every current link against that baseline. Changing the value is a whole-page re-certification: bump only after reviewing every flagged link on the page.

While the worktree value differs from HEAD (a bump not yet committed), certification outcomes are suppressed — only structural breakage is flagged — so an in-progress review never blocks.

## Classifications

| Outcome | Meaning | Diagnostic kind |
|---|---|---|
| Healthy | cited bytes match the baseline | silent |
| Moved | certified content relocated; `--fix` rewrites the link to follow it | silent when byte-identical; else reported for re-certification |
| Drifted | same place, different bytes — *this article may now be wrong* | `link_drift` |
| Broken | target missing or range no longer fits | `link_broken` |
| Uncertified | link added since the last review | `link_uncertified` |
| Unverifiable | certified content matches multiple locations, or a duplicate label was deleted leaving pairing ambiguous — never first-hit-wins | `link_unverified` |

Fail-closed rules baked into the engine:

- A match in an **unrelated file is a quote, not a move** — relocation requires git-history identity evidence (the destination traces back to the cited file through renames).
- No `links-reviewed:` field at all → `anchor_epoch_missing` error; `--fix` initializes it.
- Shallow clone → hard error, never a guess (see [Running Wiki Check](./running-wiki-check.md)).
- Unparseable frontmatter YAML → hard error; `--fix` leaves the page untouched — broken YAML is always a human repair.

The remedy for drifted/uncertified/unverifiable links is judgment, not automation: [Resolving Skipped Wiki Fixes](./resolving-skipped-fixes.md).
