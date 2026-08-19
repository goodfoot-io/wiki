# Writing a page

## Frontmatter

```yaml
---
title: Authorization              # required, non-empty
summary: How role/scope checks resolve.   # required, non-empty
aliases: [Auth, AuthZ]            # optional; other names readers will search
tags: [security]                 # optional
keywords: [rbac, permissions]    # optional
---
```

`title` and `summary` are the only required fields and define page-hood. `aliases`/`tags`/`keywords` are arrays of non-empty strings. `links-reviewed:` is optional here but required in practice for any page that cites code — see `./fragment-links-and-coverage.md`.

## Titles: unique and not reserved

- Titles and aliases are unique **case-insensitively** across the whole corpus. A collision is a `wiki check` failure — run `wiki "…"` or `wiki list` first to confirm the name is free.
- A title/alias may not be a reserved subcommand name: `check`, `list`, `summary`. `wiki <title>` would otherwise dispatch to the subcommand.

Add aliases for every name a reader might search. The alias is cheaper than a second page.

## Page-to-page links

Standard relative markdown, resolved against the linking file's directory:

```markdown
See [Authorization](./authorization.md) for the policy model.
Jump to [Authorization#Role checks](./authorization.md#role-checks).
```

`wiki check` verifies the target file exists and that any `#heading` slug resolves to a real heading in the target. A broken link or dangling anchor is a plain-text fix.

## Path resolution

- bare (`images/foo.png`), `./`, `../` → relative to the **page's directory**
- leading `/` (`/packages/api/client.ts`) → relative to the **repo root**
- `http(s)://` → not validated

## Don't anchor purely descriptive prose

A wiki page earns fragment links (and the drift surveillance they get) when it is **load-bearing** — a contract, a spec, a runbook, or part of a curated corpus whose accuracy against the code is itself maintained. A tutorial paragraph that just paraphrases code the reader can read directly does not need a fragment link. If you wouldn't want a code change to flag "this article may now be wrong," don't anchor it.

When you do cite code, see `./fragment-links-and-coverage.md`.
