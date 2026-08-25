---
title: Wiki Check Diagnostics Reference
summary: Every diagnostic kind, exit code, and JSON output shape wiki check can produce.
tags: [wiki, reference]
links-reviewed: 2
---

## Diagnostic kinds

One pass emits frontmatter/link diagnostics plus per-link anchor classification ([kind construction](/packages/cli/src/commands/check.rs#L923-L988) routes every hard failure):

| Kind | Trigger |
|---|---|
| `runtime` | infrastructure error during the run (see exit codes) |
| `frontmatter` | missing/invalid `title`/`summary`, bad arrays, reserved title, YAML syntax |
| `collision` | title/alias defined twice (case-insensitive) |
| `broken_link` / `broken_anchor` | non-range link target missing / heading fragment unresolved |
| `anchor_epoch_missing` | line-range links but no `links-reviewed:` field |
| `link_drift` / `link_broken` / `link_uncertified` / `link_unverified` | certification outcomes — see [Citing Source With Fragment Links](../how-to/cite-source.md) |

Healthy and moved links are silent.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Clean, or `--no-exit-code`. Search/list/summary: success (search exits 0 even with zero hits). |
| 1 | Diagnostics present; summary target not found. Fix mode: unresolvable certification skips remain. |
| 2 | Infrastructure failure: shallow clone, unreadable repo, malformed `.wikiignore`, empty corpus (non-fix check), `--fix` off-worktree, missing summary input. |

## JSON shapes

Read-only check → `{"errors":[{kind,file,line,message}]}`. Fix runs add `fixes`, `skipped`, `appliedPaths`, `unverified`, `certificationSkips`. Hard errors go to stderr as `{"error":…}` keeping stdout parseable — note that running where no wiki root exists yields `{"error":"failed to discover git repository from the current directory"}`, an object with **no** `errors` array; consumers must guard for it.
