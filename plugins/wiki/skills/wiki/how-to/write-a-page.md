---
title: Authoring Wiki Pages
summary: Frontmatter rules, reserved titles, case-insensitive collisions, page-to-page links, heading-anchor slugs, and path resolution.
tags: [wiki, how-to]
links-reviewed: 1
---

## Frontmatter

```yaml
---
title: Authorization              # required, non-empty — with summary, defines page-hood
summary: How role/scope checks resolve.   # required, non-empty, one line
aliases: [Auth, AuthZ]            # optional; every name a reader might search
tags: [security]                  # optional; whole-token filter for `wiki list --tag`
keywords: [rbac, permissions]     # optional; search-boost terms (weight 3)
links-reviewed: 1                 # optional; required in practice for pages citing code ranges
---
```

All array fields take non-empty strings. `title`+`summary` make the file a wiki page; everything else is optional ([parser](/packages/cli/src/frontmatter.rs#L123)).

## Titles

- Titles and aliases are unique **case-insensitively** across the corpus; a collision fails `wiki check` (`collision`). Run `wiki "…"` or `wiki list` before naming.
- Reserved as title or alias: `check`, `list`, `summary` — `wiki <title>` would dispatch to the subcommand ([reserved names](/packages/cli/src/frontmatter.rs#L51)).

Add an alias instead of a second page whenever two names compete for one topic.

## Page-to-page links

Standard relative markdown, resolved against the linking page's directory:

```markdown
See [Authorization](./authorization.md).
Jump to [Authorization#Role checks](./authorization.md#role-checks).
```

`wiki check` verifies the target exists and any `#fragment` resolves to a real heading (`broken_anchor`). Slugs are GitHub-style ([slug rules](/packages/cli/src/headings.rs#L29-L46)): lowercase, spaces → single `-` each (not collapsed), punctuation dropped; duplicate headings get `-1`, `-2`, …. Fragments are percent-decoded before matching.

## Path resolution

| Form | Resolves against |
|---|---|
| bare, `./`, `../` | the page's directory |
| leading `/` | repo root |
| `http(s)://`, other schemes | not validated |

## Don't anchor descriptive prose

Fragment links earn drift surveillance. Anchor load-bearing content only — contracts, specs, runbooks, curated corpus pages. A tutorial paraphrasing code the reader can open does not need a fragment link; if you wouldn't want a code change to flag the article as possibly wrong, don't cite it. When you do cite: [Citing Source With Fragment Links](./cite-source.md).
