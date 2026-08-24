---
title: Resolving Skipped Wiki Fixes
summary: The judgment flow for links wiki check --fix refuses to auto-repair — confirm what changed, then re-point, edit prose, or bump links-reviewed.
tags: [wiki, how-to]
links-reviewed: 1
---

`wiki check --fix` relocates moved content and refuses to guess at the rest. A skip names the *mechanism* ("the cited bytes changed"), not the decision — bumping `links-reviewed:` without reading converts a real "this article may be wrong" signal into a green check with lying prose.

## Confirm before you re-certify

For each skipped link:

1. Read the cited range's history: `git log -L <start>,<end>:<file>` (or blame the range).
2. Read the page prose around the link.
3. Decide: does the change alter what the code does, or only how it looks?

Never batch-bump fields to clear an exit code.

## Classify and act

```mermaid
graph TD
    A[skipped link] --> B{what changed?}
    B -->|behavior: params, logic,<br/>return values, deleted feature| C[update prose, then bump links-reviewed]
    B -->|cosmetic: rename, reformat| D[prose still accurate → bump]
    B -->|range shifted and fix<br/>didn't relocate| E[edit the link range by hand,<br/>then bump]
    B -->|relocated but "not byte-identical"| F[review the new range, then bump]
    B -->|content deleted| G[drop the link from the page]
```

**Bump** = change the page's `links-reviewed:` value after every flagged link on the page is resolved (any change re-certifies; increment):

```yaml
links-reviewed: 2   # was 1
```

The field is per-page: one bump re-certifies every line-range link on it together.

- **Moved, relocated honestly** (`fixed:` line, byte-identical destination) — nothing to do.
- **Moved, "not byte-identical" skip** — `--fix` rewrote the href but won't certify it; review the new range, then bump.
- **Moved, skipped entirely** (ambiguous or no rename evidence) — edit the fragment by hand, then bump.
- **Deleted target with no successor** — drop the link. If other line-range links remain, bump.

There is no `reanchor` verb by design: re-pointing links and re-certifying are page edits, never CLI commands ([initialization is the only field write](/packages/cli/src/commands/drift.rs#L1949-L1966), and it only inserts when absent).

## Update neighbors, then stage together

If your prose edit makes a linked page inaccurate too, fix that page before moving on. Commit as plain page edits, then `wiki check` to confirm the failure cleared.
