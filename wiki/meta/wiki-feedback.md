---
title: Wiki CLI Feedback
summary: Feature requests, bug reports, and observations from using the wiki utility — living document updated after each wiki session.
aliases:
  - wiki feedback
tags:
  - meta
  - tooling
---

Living record of feedback on the `wiki` CLI utility. Updated after each wiki session with any friction, surprises, or requests encountered in practice.

For the canonical map of documentation and automation files that should be checked after wiki CLI guidance changes, see [Wiki Documentation Touchpoints](./wiki-documentation-touchpoints.md).

## Feature Requests

## Bug Reports

- **[`wiki check`](/packages/cli/src/commands/check.rs#L203-L270) scanned git worktree directories** — `globwalk` (used internally for file discovery) uses `walkdir::WalkDir`, which does not honour `.gitignore`. Directories like `.worktrees/` that are gitignored were traversed, causing title-collision errors from duplicate pages in worktrees. Fixed in `packages/wiki` by replacing `globwalk` with [`ignore::WalkBuilder`](/packages/cli/src/commands/mod.rs#L375-L380) + [`globset::GlobSet`](/packages/cli/src/commands/mod.rs#L358-L370), which respects `.gitignore` during traversal.

## Observations

- **[`wiki [query]`](/packages/cli/src/commands/search.rs#L61-L90) exits with code 1 on no matches, cancelling parallel Bash calls** — when `wiki "query"` returns no results it exits 1, which the Bash tool treats as an error and cancels any sibling tool calls that were issued in the same parallel batch. Workarounds: run wiki queries in their own message, or append `; true` / `2>&1` to suppress the exit code. A non-zero exit for "no results" is standard CLI convention but is disruptive in tool-call contexts where parallel execution is the norm.

- `wiki check` accepts [glob patterns as positional arguments](/packages/cli/src/main.rs#L117-L130), allowing focused validation of specific files (e.g. `wiki check "packages/extension/**/*.md"`). Default (no args) scans all `.md` files and identifies wiki pages by their frontmatter. This is useful for validating a single newly-created page without scanning the whole repo, consistent with CLAUDE.md guidance to focus validation runs.
- Ranked wiki lookup is exposed as the default `wiki [query]` form. Current operator guidance should not refer to `wiki search [query]`. For a known page, [`wiki summary "Page Title"`](/packages/cli/src/commands/summary.rs#L130-L158) is the documented CLI path to confirm the canonical page and summary before opening the markdown file directly.
