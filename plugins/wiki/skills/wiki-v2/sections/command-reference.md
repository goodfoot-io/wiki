---
title: Wiki Command Reference
summary: Flat lookup — subcommands, flags, global options, environment variables, anchor grammar, diagnostic kinds, exit codes, JSON schemas, reserved names.
tags: [wiki, reference]
links-reviewed: 1
---

Surface lookup. For workflows see the other sections.

## Subcommands

| Command | Purpose |
|---|---|
| `wiki [query]` | Default. Ranked search ([enum](/packages/cli/src/main.rs#L97-L175)). |
| `wiki check [glob]…` | Validate frontmatter, links, and anchor certification; `--fix` repairs. |
| `wiki list` | Title-ordered corpus listing. |
| `wiki summary [title\|alias\|path]` | One page's summary; stdin when omitted. |

## Flags

| Flag | Where | Effect |
|---|---|---|
| `--fix` | check | Relocate moved links, route broken through renames, initialize `links-reviewed:`. Requires worktree source. |
| `--fix-dry-run` | check | Preview rewrites, no mutation. Requires `--fix`. |
| `--print-applied` | check | stdout = rewritten repo-relative paths only; rest → stderr. Requires `--fix`; conflicts with dry-run and JSON. |
| `--no-exit-code` | check | Report-only; always exit 0. |
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

`path#Lstart-Lend` or single line `path#L5`; bare `path` = whole file. Paths resolve per [Writing A Wiki Page](./writing-a-page.md). Classification baseline: content at the commit where `links-reviewed:` last changed.

## Diagnostic kinds (`check --format json`)

`runtime`, `frontmatter`, `collision`, `broken_link`, `broken_anchor`, `anchor_epoch_missing`, `link_drift`, `link_broken`, `link_uncertified`, `link_unverified`.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Clean, or `--no-exit-code`. Search/list/summary: success (search exits 0 even with zero hits). |
| 1 | Diagnostics present; summary target not found. Fix mode: unresolvable certification skips remain. |
| 2 | Infrastructure failure: shallow clone, unreadable repo, malformed `.wikiignore`, empty corpus (non-fix check), `--fix` off-worktree, missing summary input. |

## Reserved names

Titles and aliases may not be `check`, `list`, `summary`.
