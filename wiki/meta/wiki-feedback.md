---
title: Wiki CLI Feedback
summary: Feature requests, bug reports, and observations from using the wiki utility — living document updated after each wiki session.
aliases:
  - wiki feedback
tags:
  - meta
  - tooling
links-reviewed: 1
---

Living record of feedback on the `wiki` CLI utility. Updated after each wiki session with any friction, surprises, or requests encountered in practice.

For the canonical map of documentation and automation files that should be checked after wiki CLI guidance changes, see [Wiki Documentation Touchpoints](./wiki-documentation-touchpoints.md).

## Feature Requests

## Bug Reports

- **[`wiki check`](/packages/cli/src/commands/mod.rs#L231-L237) scanned git worktree directories** — `globwalk` (used internally for file discovery) uses `walkdir::WalkDir`, which does not honour `.gitignore`. Directories like `.worktrees/` that are gitignored were traversed, causing title-collision errors from duplicate pages in worktrees. Fixed in `packages/wiki` by replacing `globwalk` with [`ignore::WalkBuilder`](/packages/cli/src/commands/mod.rs#L570-L607) + [`globset::GlobSet`](/packages/cli/src/commands/mod.rs#L576-L588), which respects `.gitignore` during traversal.

## Observations

- **[`wiki [query]`](/packages/cli/src/commands/search.rs#L20-L25) now exits 0 on no matches** — an earlier version exited 1 when a query returned no results, which the Bash tool treated as an error and which cancelled sibling tool calls issued in the same parallel batch. The search command now returns `Ok(0)` for an empty result set (an empty `[]` under `--format json`), so a no-match query is no longer disruptive in parallel tool-call contexts. Business-logic failures and runtime errors still use non-zero exits.

- `wiki check` accepts [glob patterns as positional arguments](/packages/cli/src/main.rs#L109-L111), allowing focused validation of specific files (e.g. `wiki check "packages/extension/**/*.md"`). Default (no args) scans all `.md` files and identifies wiki pages by their frontmatter. This is useful for validating a single newly-created page without scanning the whole repo, consistent with CLAUDE.md guidance to focus validation runs.
- Ranked wiki lookup is exposed as the default `wiki [query]` form. Current operator guidance should not refer to `wiki search [query]`. For a known page, [`wiki summary "Page Title"`](/packages/cli/src/commands/summary.rs#L72-L100) is the documented CLI path to confirm the canonical page and summary before opening the markdown file directly.
