---
title: Running Wiki Check
summary: The validation gate — invocations, diagnostic kinds, exit codes, CWD scoping, --source snapshots, the anchor cache, and .wiki/.wikiignore.
tags: [wiki, how-to]
links-reviewed: 1
---

```bash
wiki check --fix             # the usual invocation: clear mechanical drift first
wiki check                   # read-only; every page under CWD
wiki check path/to/page.md   # explicit globs (also **/*.md patterns), resolved from CWD
```

## What it validates

One pass emits frontmatter/link diagnostics plus per-link anchor classification ([kinds](/packages/cli/src/commands/check.rs#L332-L339) route every hard failure):

| Kind | Trigger |
|---|---|
| `frontmatter` | missing/invalid `title`/`summary`, bad arrays, reserved title, YAML syntax |
| `collision` | title/alias defined twice (case-insensitive) |
| `broken_link` / `broken_anchor` | non-range link target missing / heading fragment unresolved |
| `anchor_epoch_missing` | line-range links but no `links-reviewed:` field |
| `link_drift` / `link_broken` / `link_uncertified` / `link_unverified` | see [Citing Source From A Wiki Page](./citing-source.md) |

Healthy and moved links are silent. Exit codes: **0** clean; **1** any diagnostic (in fix mode, also unapplied certification skips); **2** infrastructure — shallow clone, unreadable repo, malformed `.wiki/.wikiignore` pattern, empty corpus in non-fix mode, or `--fix` without `--source=worktree`.

## Scoping

Selection follows the CWD; links and anchors always resolve against the git repo root — a subdirectory run yields the same diagnostics as repo-relative globs from root.

## Flags that matter

- `--fix` — relocate moved links, route broken targets through renames, initialize `links-reviewed:`. Worktree only.
- `--fix-dry-run` — preview rewrites; requires `--fix`.
- `--print-applied` — stdout = one repo-relative path per rewritten file; everything else → stderr. Lets callers stage exactly what the run touched.
- `--no-exit-code` — report-only; always exit 0 (the hook's mode).
- `--format json` — `{"errors":[{kind,file,line,message}]}`; fix runs add `fixes`/`skipped`/`appliedPaths`/`unverified`/`certificationSkips`; hard errors go to stderr as `{"error":…}` keeping stdout parseable.
- `--clear-cache` — delete the anchor cache, print its path, always exit 0.
- `--source worktree|index|head` — validate another snapshot: staged content (`index`) for pre-commit, committed state (`head`) for CI. Uncommitted pages are invisible at `head`.

## Anchor cache

History walks memoize in `<git common dir>/wiki/anchor-cache.sqlite`, shared across worktrees, keyed by exact git-log output so history changes invalidate naturally. Cache faults degrade to uncached with one warning; `WIKI_ANCHOR_CACHE=0` disables it. Delete with `wiki check --clear-cache`.

## `.wiki/.wikiignore`

`.wiki/.wikiignore` excludes paths from discovery before any validation, on every command and source ([loader](/packages/cli/src/wikiignore.rs#L12-L34)). Gitignore syntax, patterns anchored at repo root. A path under an excluded directory can't be resurrected by a deeper `!pattern`. Malformed pattern = exit 2 (fail closed). Ignored targets are also exempt from `broken_link`.

## Shallow clones fail closed

Anchor lookup needs full history; shallow clones error (exit 2) rather than guess. CI: `fetch-depth: 0`. With `--no-exit-code` the error prints but exits 0 — protection resumes when the clone is deepened.
