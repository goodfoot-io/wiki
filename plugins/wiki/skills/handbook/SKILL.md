---
name: handbook
description: Use with the `wiki` CLI, wiki pages, fragment links, `wiki check`, or `.wiki/` mesh coverage.
---

<instructions>
- **Finding a page, paginating results, listing the corpus, reading a summary, or deciding whether a `.md` file is even a wiki page**: Read `./sections/searching-and-reading.md`
- **Writing a new page or editing one: frontmatter rules, required `title`/`summary`, reserved titles, case-insensitive collisions, page-to-page links, path resolution**: Read `./sections/writing-a-page.md`
- **Citing source code from a page, choosing a line range vs whole-file anchor, or understanding why a fragment link demands mesh coverage (the one-mesh-anchors-both contract)**: Read `./sections/fragment-links-and-coverage.md`
- **Running `wiki check`: what it validates, the three diagnostic buckets, CWD scoping, `--format json`, `--no-exit-code`, `--source worktree|index|head`**: Read `./sections/running-wiki-check.md`
- **A `mesh_uncovered` diagnostic, or covering new fragment links: `--fix`, `--fix-dry-run`, `--print-applied`, and what the pre-commit hook does automatically**: Read `./sections/fixing-mesh-coverage.md`
- **`wiki check --fix` fail-closed and named a `wiki mesh …` command, or an anchor drifted and a decision is needed (re-hash, re-anchor, delete) — the judgment lives here, not in the command**: Read `./sections/resolving-skipped-fixes.md`
- **Exact subcommand, flag, anchor grammar, reserved-name, or `wiki mesh show/add/remove` semantics lookup**: Read `./sections/command-reference.md`
- **`wiki: command not found`, a stale binary rejecting a flag, or the pre-commit/PostToolUse hook silently doing nothing**: Read `./sections/install-and-path.md`

**Mesh-only changes (`.wiki/` edits) do not require full codebase validation.** Reconciliation needs only the `wiki` binary — never the `git mesh` executable.
</instructions>
