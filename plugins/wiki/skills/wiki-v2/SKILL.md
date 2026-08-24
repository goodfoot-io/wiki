---
name: wiki-v2
description: Operating manual for the wiki CLI (wiki 0.5.x) in this repository. Load when searching or reading wiki pages, writing or editing one, citing source with fragment links, running or interpreting `wiki check`, resolving drift or skipped fixes, certifying pages with `links-reviewed:`, excluding files via `.wiki/.wikiignore`, or wiring the pre-commit wiki hook.
title: Wiki Skill
summary: Hub for the wiki CLI — search-first reading, page authoring, certified fragment links, drift validation, and hook wiring.
---

<instructions>
**Reach for `wiki "[query]"` before grepping unfamiliar code.** It returns ranked, source-anchored answers about how this codebase works; a maintained page beats re-deriving intent from source.

The loop has three moves:

- **Search**: `wiki "[query]"` (default subcommand). Run it first on any unfamiliar area; run it again before authoring to find the canonical title and catch collisions. `-l N`/`-o N` paginate (default 3). → `./sections/searching-and-reading.md`
- **Write**: a page is `.md` with non-empty `title`+`summary` frontmatter (both required = page-hood); cite code with line-ranged fragment links (`path#L10-L40`) certified by `links-reviewed:`. → `./sections/writing-a-page.md`, `./sections/citing-source.md`
- **Check**: `wiki check --fix` is the first move on any validation failure — editing cited source shifts line numbers, so most `link_broken`/`link_drift` errors are mechanical and auto-repairable. Only what `--fix` skips needs judgment. → `./sections/running-wiki-check.md`, `./sections/resolving-skipped-fixes.md`

Route by condition:

- **Choosing commands, paginating, listing the corpus, resolving titles/aliases/paths, or deciding whether a `.md` file is a wiki page**: Read `./sections/searching-and-reading.md`
- **Creating or editing a page: frontmatter fields, reserved titles, collisions, page-to-page links, heading anchors, path resolution**: Read `./sections/writing-a-page.md`
- **Citing code: range vs whole-file links, the anchor contract, every link classification**: Read `./sections/citing-source.md`
- **Running `wiki check`: diagnostic kinds, exit codes, CWD scoping, `--source`, the anchor cache, `.wiki/.wikiignore`, shallow clones**: Read `./sections/running-wiki-check.md`
- **`--fix` skipped a link, or a link drifted and a re-certification decision is needed**: Read `./sections/resolving-skipped-fixes.md`
- **Exact flag, grammar, kind string, env var, or JSON schema lookup**: Read `./sections/command-reference.md`
- **Starting a wiki, placing a page, centralize-vs-embed, hub pages, restructuring**: Read `./sections/organizing-a-wiki.md`
- **Wiring the pre-commit hook (copyable script at [`./examples/pre-commit.wiki.sh`](./examples/pre-commit.wiki.sh)), or debugging what a commit re-staged**: Read `./sections/git-hook-setup.md`

Wiki-only changes don't require full codebase validation.

## PostToolUse hook output

Editing a wiki page fires this plugin's hook (`wiki check --fix` on that file; binary resolved from `$WIKI_BIN`, then PATH, then the VS Code extension's managed copy). A `<wiki>…</wiki>` block means either residual diagnostics `--fix` couldn't resolve (act per the routes above — usually `./sections/resolving-skipped-fixes.md`) or validation was skipped because the binary wouldn't launch (install it / set `WIKI_BIN`, re-save). No block = clean or fully auto-fixed.
</instructions>
