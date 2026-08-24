---
title: Citing Source With Fragment Links
summary: Fragment-link grammar and discipline, certification mechanics, and every classification a line-range link can receive.
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

## Certification mechanics

One frontmatter field certifies every line-range link on the page together:

```yaml
links-reviewed: 1
```

The value is the page's **anchor epoch**. `wiki check` walks the page's git history (`--follow`) to the commit where that value last changed — the anchor commit — hashes each cited range as it was there (rk64 fingerprints), and classifies every current link against that baseline. Changing the value is a whole-page re-certification; the decision flow lives in [Validate And Fix](./validate-and-fix.md#when---fix-skips).

While the worktree value differs from HEAD (a bump not yet committed), certification outcomes are suppressed — only structural breakage is flagged — so an in-progress review never blocks.

## Classifications

| Outcome | Meaning | Diagnostic kind |
|---|---|---|
| Healthy | cited bytes match the baseline | silent |
| Moved | certified content relocated; `--fix` rewrites the link to follow it | silent when byte-identical; else reported for re-certification |
| Drifted | same place, different bytes — *this article may now be wrong* | `link_drift` |
| Broken | target missing or range no longer fits | `link_broken` |
| Uncertified | link added since the last review | `link_uncertified` |
| Unverifiable | certified content matches multiple locations, or a deleted duplicate label left pairing ambiguous — never first-hit-wins | `link_unverified` |

Fail-closed rules baked into the engine:

- A match in an unrelated file is a quote, not a move — relocation requires rename-tracked history evidence.
- No `links-reviewed:` field at all → `anchor_epoch_missing`; `--fix` initializes it.
- Shallow clone → hard error, never a guess.
- Unparseable YAML → hard error; `--fix` leaves the page untouched.

Rationale behind these rules: [The Anchor Contract](../explanation/anchor-contract.md).
