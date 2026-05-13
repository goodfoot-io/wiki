---
name: wiki
description: This skill should be used when the user asks to "search the wiki", "write a wiki page", "add a wikilink", "fix a wiki check failure", "resolve mesh_uncovered", "scaffold meshes", or mentions wiki frontmatter, fragment links, `wiki check`, `wiki scaffold`, or wiki/git-mesh integration.
---

# Wiki

A corpus of Markdown pages with relative-path links between pages and **fragment links** (with line ranges) into source code. The `wiki` CLI searches and validates them. `git mesh` keeps fragment links honest.

## Search

```bash
wiki "auth policy"          # ranked search; the default subcommand
```

## What counts as a wiki page

A file is a wiki page if **either**:

- it lives under a directory tree whose ancestor contains a `wiki.toml` — then a plain `*.md` extension is enough (no `namespace` frontmatter needed; the namespace is inherited from the `wiki.toml` root), **or**
- it lives outside any such tree — then it must use the `*.wiki.md` extension. A `namespace` frontmatter field is optional and assigns the page to a peer wiki.

In short: `*.md` is for pages under a `wiki.toml`; `*.wiki.md` is for wiki pages anywhere else (e.g. a package `README.wiki.md`).

## Frontmatter

```markdown
---
title: Authorization
summary: How the runtime evaluates role and scope checks.
aliases: [Auth, AuthZ]
tags: [security]
keywords: [rbac, permissions]
namespace: platform   # *.wiki.md only — assigns the page to a peer wiki
---
```

- `title` and `summary` are **required**. Both are non-empty strings.
- `aliases`, `tags`, `keywords` are arrays of non-empty strings.
- Titles and aliases are unique **case-insensitively** across the wiki.
- `title` may not be a reserved command name: `check`, `pin`, `stale`, `links`, `list`, `summary`, `print`. (`wiki <title>` dispatches to the subcommand if it collides.)
- `namespace` is meaningful **only on `*.wiki.md` files**; pages under a `wiki.toml` inherit their namespace from the root.

## Default namespace vs named namespaces

A repo has at most **one default (anonymous) namespace** — the wiki whose `wiki.toml` omits the `namespace` field. All other wikis are **named peers** (`namespace = "marketing"`, etc.).

- An empty `wiki.toml` (or one without a `namespace` key) → that wiki **is** the default namespace. Don't add `namespace = "wiki"` "to name it" — that demotes it to a named peer and breaks bare links from any page that was relying on the default.
- The literal value `namespace = "default"` is **reserved and rejected**. Omit the field to declare the default.
- Bare relative-path links resolve within the current page's namespace; the default namespace has no special "fallback" status for cross-namespace lookups.
- `*.wiki.md` files outside any `wiki.toml` tree may set `namespace` to join a named peer; omitting it places them in the default namespace.

## Page-to-page links

```markdown
See [Authorization](./authorization.md) for the policy model.
Jump to [Authorization#Role checks](./authorization.md#role-checks) for the heading.
```

Links between wiki pages use standard markdown relative-path syntax, resolved against the linking file's directory. `wiki check` verifies the target file exists and that any `#heading` slug resolves.

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
wiki check                  # links + frontmatter + mesh coverage
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
wiki scaffold               # emits the exact `git mesh add` / `git mesh why` commands
# review and run the emitted commands, then:
git commit
```

`wiki scaffold` walks the corpus and prints the precise mesh commands needed for every fragment link. Pipe it to a shell, or copy the lines you want.

## Authoring workflow

1. Place the page (under a `wiki.toml`, or as `*.wiki.md`).
2. Write `title` + `summary`; add `aliases` for other names readers will use.
3. Cross-link with relative markdown links. Run `wiki "..."` first to pick the canonical title.
4. Cite source code with **line-ranged** fragment links.
5. `wiki check`. For `mesh_uncovered`: `wiki scaffold` → run → commit.

## References

- **`references/cli.md`** — full CLI surface (less-common subcommands and flags: `summary`, `links`, `refs`, `list`, `extract`, `namespaces`, `init`, `hook`, `install`; `--no-mesh`, `--no-exit-code`, `--format json`, `--source`, `-l/-o/-n`). **Use when** reaching past the day-to-day commands above.
- **`references/maintenance.md`** — keeping a wiki current with `git mesh`: `git mesh stale` → re-anchor → `wiki check` → `wiki scaffold`, and writing a durable `why`. **Use when** anchors have drifted, when meshes go stale, or when curating wiki health.
- **`references/git-hook-setup.md`** — two-phase git hooks: `pre-commit` blocks broken links and bad frontmatter; `post-commit` auto-scaffolds mesh coverage. **Use when** wiring wiki validation into a repo for the first time, or debugging why a commit was blocked or auto-scaffolded.
