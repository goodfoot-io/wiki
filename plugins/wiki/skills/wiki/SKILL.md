---
name: wiki
description: This skill should be used when the user asks to "search the wiki", "write a wiki page", "fix a wiki check failure", "resolve mesh_uncovered", "create mesh coverage", or mentions wiki frontmatter, fragment links, `wiki check`, `wiki check --fix`, or wiki/git-mesh integration.
---

# Wiki

A corpus of Markdown pages with relative-path links between pages and **fragment links** (with line ranges) into source code. The `wiki` CLI searches and validates them. `git mesh` keeps fragment links honest.

## Search

```bash
wiki "auth policy"          # ranked search; the default subcommand
```

## What counts as a wiki page

A file is a wiki page if it has `title` and `summary` frontmatter. All `*.md` files under the current working directory are candidates; those without the required frontmatter are flagged by `wiki check`.

## Frontmatter

```markdown
---
title: Authorization
summary: How the runtime evaluates role and scope checks.
aliases: [Auth, AuthZ]
tags: [security]
keywords: [rbac, permissions]
---
```

- `title` and `summary` are **required**. Both are non-empty strings.
- `aliases`, `tags`, `keywords` are arrays of non-empty strings.
- Titles and aliases are unique **case-insensitively** across the wiki.
- `title` may not be a reserved command name: `check`, `pin`, `stale`, `links`, `list`, `summary`, `print`. (`wiki <title>` dispatches to the subcommand if it collides.)

## Page-to-page links

```markdown
See [Authorization](./authorization.md) for the policy model.
Jump to [Authorization#Role checks](./authorization.md#role-checks) for the heading.
```

Links between wiki pages use standard markdown relative-path syntax, resolved against the linking file's directory. `wiki check` verifies the target file exists and that any `#heading` slug resolves to an actual heading in the target.

## Fragment links — prefer line ranges

Fragment links point from a wiki page to a sibling file in the repo. **Always include a line range** — they are the unit of mesh coverage and drift detection:

```markdown
The retry loop lives in [client.ts](../packages/api/client.ts#L88-L120).
The config struct is in [config.ts](../packages/api/config.ts#L1-L42).
```

Whole-file links (no `#L…` suffix) are valid but discouraged: coverage falls back to the `0-0` sentinel and you lose line-level drift signal.

Path resolution follows standard markdown: a bare path (`images/foo.png`) or `./` / `../` prefix resolves relative to the wiki page's directory; a leading `/` (e.g. `/packages/api/client.ts`) resolves from the repository root. `http://` / `https://` links are not validated and don't participate in mesh coverage.

## Validate: `wiki check`

```bash
cd wiki && wiki check        # links + frontmatter + mesh coverage, scoped to wiki/
wiki check                   # checks every *.md page under the current directory
```

Diagnostics fall into three buckets:

- **Frontmatter / link errors** — fix in the page.
- **`mesh_uncovered`** — fragment link has no covering mesh. Fix below.
- **`mesh_unavailable`** — `git-mesh` not on `PATH`; mesh check is skipped. Install `git-mesh` to restore it.

## The mesh-coverage contract (non-obvious)

For every fragment link `path#L<start>-L<end>` in a wiki page, there must be a `git mesh` that anchors **both**:

1. the **code target** — at exactly `start-end`, *or* as a whole-file `0-0` anchor, **and**
2. the **wiki page itself**.

A mesh that only anchors one side does not cover the link. Links without a line range and external links are exempt.

### Fix `mesh_uncovered`

```bash
wiki check --fix --fix-dry-run  # preview which meshes will be created (no changes)
git commit                       # pre-commit hook runs wiki check --fix, creates and
                                 # stages meshes automatically
```

`wiki check --fix` walks the corpus and creates a `git mesh` for every uncovered fragment link (anchors only, no why) as part of its fix pass. The pre-commit hook runs `wiki check --fix --print-applied` automatically and stages exactly the meshes it creates or renames into the commit; use `--fix-dry-run` to preview what it will do before committing.

## Authoring workflow

1. Place the page under the directory you check from (e.g. `wiki/`).
2. Write `title` + `summary`; add `aliases` for other names readers will use.
3. Cross-link with relative markdown links. Run `wiki "..."` first to pick the canonical title.
4. Cite source code with **line-ranged** fragment links.
5. `cd wiki && wiki check`. For `mesh_uncovered`: `wiki check --fix --fix-dry-run` (preview) → `git commit` (pre-commit hook runs `wiki check --fix` and stages meshes automatically).

## References

- **`references/cli.md`** — full CLI surface (less-common subcommands and flags: `summary`, `links`, `refs`, `list`, `extract`, `hook`, `install`; `--fix`, `--fix-dry-run`, `--print-applied`, `--no-exit-code`, `--format json`, `--source`, `-l/-o`). **Use when** reaching past the day-to-day commands above.
- **`references/maintenance.md`** — keeping a wiki current with `git mesh`: `git mesh stale` → re-anchor → `wiki check` → `wiki check --fix`, and writing a durable `why`. **Use when** anchors have drifted, when meshes go stale, or when curating wiki health.
- **`references/git-hook-setup.md`** — single-invocation git hook: `pre-commit` runs `wiki check --fix --print-applied` to auto-fix drifted links/frontmatter and create mesh coverage in one pass, then re-stages all touched paths. **Use when** wiring wiki validation into a repo for the first time, or debugging why files were re-staged or meshes were auto-created.
