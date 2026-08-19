---
name: wiki
description: CLI for documentation in this repository - search with `wiki "[query]"` to start. Includes tools for validation, drift tracking, and authoring.
---

<instructions>
**Reach for `wiki "[query]"` to learn about this project.** It's the point of the wiki: ranked, source-anchored answers about how this codebase actually works — architecture, design rationale, how subsystems connect. Before grepping or guessing at unfamiliar code, search the wiki first; a maintained page beats re-deriving intent from source. Run it early and often, not just when authoring.

- **Search**: `wiki "[query]"` (default subcommand). Your first move on any unfamiliar area; also run it before authoring to find the canonical title to link and to catch a collision early. `-l N`/`-o N` paginate (default 3).
- **Check**: `wiki check --fix` is the first line of defense — run it *before* hand-editing fragment line numbers or investigating a failure. Editing cited source shifts line numbers, so a bare `wiki check` surfaces alarming `link_broken`/`link_drift` errors that are often mechanical: `--fix` relocates links whose certified content moved, and initializes `links-reviewed:` on field-less pages. The drift is often pre-existing, not from your change. Only what `--fix` leaves behind (see `resolving-skipped-fixes.md`) needs judgment. The pre-commit hook runs `--fix` automatically.
- **Write a page**: `.md` with `title`+`summary` frontmatter (both required = page-hood); cross-link with relative markdown; cite code with **line-ranged** fragment links (`path#L10-L40`).

For anything beyond these, route by condition:

- **Setting up a new wiki, or deciding where a page belongs / whether content belongs in the wiki at all / when to restructure: Diátaxis mode separation, embed-vs-centralize, hub pages, reorganization signals**: Read `./sections/organizing-a-wiki.md`
- **Finding a page, paginating results, listing the corpus, reading a summary, or deciding whether a `.md` file is even a wiki page**: Read `./sections/searching-and-reading.md`
- **Writing a new page or editing one: frontmatter rules, required `title`/`summary`, reserved titles, case-insensitive collisions, page-to-page links, path resolution**: Read `./sections/writing-a-page.md`
- **Citing source code from a page, choosing a line range vs whole-file anchor, or understanding the `links-reviewed:` anchor-epoch contract**: Read `./sections/fragment-links-and-coverage.md`
- **Running `wiki check`: what it validates, the three diagnostic buckets, CWD scoping, `--format json`, `--no-exit-code`, `--source worktree|index|head`**: Read `./sections/running-wiki-check.md`
- **`wiki check --fix` skipped a link and named a reason, or a cited range drifted and a decision is needed (re-point the link, fix the prose, bump `links-reviewed:`) — the judgment lives here, not in the command**: Read `./sections/resolving-skipped-fixes.md`
- **Exact subcommand, flag, anchor grammar, certification, or reserved-name lookup**: Read `./sections/command-reference.md`
- **Wiring `wiki` into a repo's pre-commit hook for the first time (copyable script at `./examples/pre-commit.wiki.sh`), or debugging why files were re-staged on commit — including an unrelated `.md` file getting swept into a commit (older hooks had this defect; it's a hook bug, not a CLI bug)**: Read `./sections/git-hook-setup.md`

**Wiki-page and doc-only changes don't require full codebase validation.**

## Hook output

Editing a wiki page (one with `title`+`summary`) fires a PostToolUse hook that runs `wiki check --fix` on it. A `<wiki>…</wiki>` block in context means one of:
- **Residual conditions** — `--fix` fixed what it could; the block is the leftover `wiki check` output (links, frontmatter, or drift it couldn't resolve). Act on it via the route bullets above (usually `./sections/resolving-skipped-fixes.md`).
- **Validation skipped** — the block says the `wiki` binary couldn't launch (the message is self-explanatory: install it on PATH or set `WIKI_BIN`, then re-save). The page went unvalidated.

No block = clean or fully auto-fixed; nothing to do.
</instructions>
