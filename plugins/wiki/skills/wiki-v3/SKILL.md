---
name: wiki-v3
description: Operating manual for the wiki CLI (wiki 0.5.x). Load when searching or reading wiki pages, writing or editing one, citing source with fragment links, running or interpreting `wiki check`, resolving drift or skipped fixes, certifying pages with `links-reviewed:`, excluding files via `.wiki/.wikiignore`, wiring the pre-commit hook, or locating the wiki binary.
title: Wiki Skill V3
summary: Hub for the wiki CLI — validate-and-fix loop first, page authoring, certified fragment links, binary resolution, and mechanical drift detection.
---

<instructions>
Reach for `wiki "[query]"` before grepping unfamiliar code. Run it again before authoring to catch title collisions.

Key distinctions:

- **Page-hood**: a `.md` file is a wiki page iff frontmatter has non-empty `title` **and** `summary`. Everything else is invisible to search and skipped by `wiki check`.
- **The loop**: edit code or pages → `wiki check --fix` clears mechanical drift → judgment resolves what `--fix` skips → bump `links-reviewed:` only after reviewing.
- **Scoping**: file selection follows CWD; links and anchors always resolve against the repo root.

## Start from your job

- **Validating after edits, interpreting failures, resolving what `--fix` skipped**: Read [`./how-to/validate-and-fix.md`](./how-to/validate-and-fix.md)
- **Creating or editing a page**: Read [`./how-to/write-a-page.md`](./how-to/write-a-page.md)
- **Citing code with fragment links**: Read [`./how-to/cite-source.md`](./how-to/cite-source.md)
- **Wiring or debugging the pre-commit hook**: Read [`./how-to/wire-the-pre-commit-hook.md`](./how-to/wire-the-pre-commit-hook.md)
- **Deciding what belongs in the wiki, placing pages, restructuring**: Read [`./how-to/organize-a-wiki.md`](./how-to/organize-a-wiki.md)

## Start from a trigger

| You notice | Load |
|---|---|
| `wiki`: command not found (exit 127) | [Binary resolution](./reference/commands.md#binary-resolution) — `$WIKI_BIN` → PATH → VS Code extension's managed copy |
| A check fails and you don't know the vocabulary | [Diagnostic kinds](./reference/diagnostics.md#diagnostic-kinds) |
| `--fix` skipped a link, or a link drifted | [Resolve skipped fixes](./how-to/validate-and-fix.md#when---fix-skips) |
| Exit 2 (infrastructure) | [Diagnostics: exit codes](./reference/diagnostics.md#exit-codes) — shallow clone, malformed `.wikiignore`, empty corpus, off-worktree `--fix` |
| Check is slow or behaves stale | [`wiki check --clear-cache`](./how-to/validate-and-fix.md#anchor-cache) |
| A check times out on a large corpus | Scope it: `wiki check path/to/page.md` or a narrower glob |
| About to type `wiki mesh`, `wiki pin`, or `wiki extract` | Stop — not in 0.5.x. Mesh lives in git-span as `git mesh`; the others never shipped |
| A query returned nothing | Unknown words are searches (exit 0, silent). Reserved verbs `check`/`list`/`summary` dispatch instead — quote the query |
| A hook emitted a `<wiki>…</wiki>` block | See [PostToolUse hook output](#posttooluse-hook-output) below |

## Browse by mode

- **`how-to/`** — validate-and-fix, write-a-page, cite-source, wire-the-pre-commit-hook, organize-a-wiki
- **`reference/`** — [`commands.md`](./reference/commands.md) (flags, grammar, env, surface truth), [`search-and-output-shapes.md`](./reference/search-and-output-shapes.md), [`diagnostics.md`](./reference/diagnostics.md)
- **`explanation/`** — [`anchor-contract.md`](./explanation/anchor-contract.md): why certification works the way it does

## Validate the work

Handoff requires `wiki check` clean and every inter-page link resolving. Wiki-only changes don't require full codebase validation.

## PostToolUse hook output

Editing a wiki page fires this plugin's hook (`wiki check --fix` on that file). A `<wiki>…</wiki>` block means residual diagnostics `--fix` couldn't resolve (act per [resolve skipped fixes](./how-to/validate-and-fix.md#when---fix-skips)) or validation was skipped because the binary wouldn't launch (install it / set `WIKI_BIN`, re-save). No block = clean or fully auto-fixed.
</instructions>
