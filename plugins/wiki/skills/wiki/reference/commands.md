---
title: Wiki Command Surface
summary: Flat lookup for wiki 0.5.x — subcommands, flags, global options, environment variables, anchor grammar, binary resolution, reserved names.
tags: [wiki, reference]
links-reviewed: 1
---

Surface truth for **wiki 0.5.x**; verify against `wiki --help` when the extension bumps versions.

## Subcommands

| Command | Purpose |
|---|---|
| `wiki [query]` | Default. Ranked search ([enum](/packages/cli/src/main.rs#L97-L175)). |
| `wiki check [glob]…` | Validate frontmatter, links, and anchor certification; `--fix` repairs. |
| `wiki list` | Title-ordered corpus listing. |
| `wiki summary [title\|alias\|path]` | One page's summary; stdin when omitted. |

**Not in 0.5.x**: `mesh` (moved to the git-span plugin — use `git mesh`), `pin`, `extract` (never shipped). Repeated invocations of removed verbs are a known agent failure mode.

## Binary resolution

Consumers resolve the binary in this order:

1. `$WIKI_BIN` — absolute path override (hooks set this).
2. `PATH` — install or symlink the CLI for bare `wiki` invocations.
3. The VS Code extension's managed copy (`<globalStorage>/goodfoot.wiki-extension/bin/<version>/<platform>/wiki`) — version-pinned per installed extension release.

Development trees additionally invoke build outputs directly (`packages/cli/target/{debug,release}/wiki`); those are not on PATH and never satisfy hooks.

## Flags

| Flag | Where | Effect |
|---|---|---|
| `--fix` | check | Relocate moved links, route broken through renames, initialize `links-reviewed:`. Requires worktree source. |
| `--fix-dry-run` | check | Preview rewrites, no mutation. Requires `--fix`. |
| `--print-applied` | check | stdout = rewritten repo-relative paths only; rest → stderr. Requires `--fix`; conflicts with dry-run and JSON. |
| `--no-exit-code` | check | Suppress validation results from the exit code (report-only). Repo-discovery and argument failures still exit 2. |
| `--clear-cache` | check | Delete anchor cache, print path, exit 0. |
| `--tag T`, `--limit N`, `--offset N` | list | Filter by whole token (case-insensitive); paginate title-ordered listing (default unlimited). |
| `-l N` / `-o N` | search | Result limit (default 3) / offset. |
| `--format json` | all | Structured output. |
| `--source worktree\|index\|head` | all | Snapshot to read (default worktree). `--fix` needs worktree. |
| `--perf` / `-v` | all | Timings to stderr / version. |

## Environment

| Var | Effect |
|---|---|
| `WIKI_BIN` | Absolute binary path override (used by hooks). |
| `WIKI_ANCHOR_CACHE=0` | Disable the anchor cache. |
| `WIKI_PERF=1` | Per-event timings to stderr; JSON lines also append to `wiki.log`. |

## Anchor grammar

`path#Lstart-Lend` or single line `path#L5`; bare `path` = whole file. Paths resolve per [Authoring Wiki Pages](../how-to/write-a-page.md). Classification baseline: content at the commit where `links-reviewed:` last changed.

## Reserved names

Titles and aliases may not be `check`, `list`, `summary`.
