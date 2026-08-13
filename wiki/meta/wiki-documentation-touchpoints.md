---
title: Wiki Documentation Touchpoints
summary: Canonical map of wiki CLI implementation, operator instructions, and maintenance references that must stay aligned when wiki documentation behavior changes.
tags:
  - meta
  - wiki
  - tooling
---

This page is the maintenance map for future wiki documentation updates. When command behavior or recommended usage changes, update the implementation-facing source of truth first, then walk the operator-facing documents and automation references listed here so guidance does not drift.

For the CLI architecture itself, see [Wiki CLI](../architecture/wiki-cli.md). For the broader rules governing wiki pages, see [Wiki Organization](./wiki-organization.md) and [Wiki CLI Advanced Usage](../reference/wiki-cli-advanced-usage.md).

## Command Behavior Source Of Truth

The primary source of truth for top-level CLI behavior is the [Clap configuration and dispatch in `packages/cli/src/main.rs`](/packages/cli/src/main.rs#L31-L87). That block defines the help text, the `query` positional argument, and the wiring to the reserved subcommand set. The [top-level `run(...)` match in the same file](/packages/cli/src/main.rs#L371-L462) is what decides that bare `wiki [query]` executes ranked lookup rather than page printing.

## Operator-Facing Documentation

These files are the public guidance surfaces most likely to drift when the CLI contract changes:

- The [wiki skill instructions](/plugins/wiki/skills/wiki/SKILL.md) are the highest-leverage agent workflow contract for discovering pages, choosing where to write, validating fragment links, and updating related pages — `CLAUDE.md` itself no longer carries wiki-specific instructions.
- The [advanced usage page](../reference/wiki-cli-advanced-usage.md) holds the less common CLI behaviors such as stdin handling, file paths, explicit glob targeting, and JSON output.
- The [feedback log](./wiki-feedback.md) is where observed friction from doc or CLI mismatches should be recorded after the change is understood.

If a documentation update changes the recommended operator workflow, all of these surfaces should be checked explicitly, not only the page that first exposed the inconsistency.

## Automation And Maintenance Touchpoints

The [Gemini wiki gap-detection example script](/examples/githooks/scripts/gemini-wiki-gap-detection.sh) embeds wiki search guidance for automated maintenance work. If the preferred search invocation or page-discovery workflow changes, this prompt must stay aligned with the human-facing docs or automation will continue reinforcing stale instructions.

The [wiki skill's skipped-fix resolution section](/plugins/wiki/skills/wiki/sections/resolving-skipped-fixes.md) defines the stale-anchor repair workflow (re-hash, re-anchor, or delete). It is not a user-facing quickstart, but it is part of the operational contract for wiki upkeep and should be checked whenever the update changes validation, pinning, or page-discovery expectations.

## Update Order

When wiki documentation behavior changes, use this order:

1. Confirm the implementation in [CLI parsing and dispatch](/packages/cli/src/main.rs#L31-L87) and [top-level command routing](/packages/cli/src/main.rs#L371-L462).
2. Update the agent workflow contract in [the wiki skill](/plugins/wiki/skills/wiki/SKILL.md).
3. Update secondary references such as [Wiki CLI Advanced Usage](../reference/wiki-cli-advanced-usage.md), [Wiki CLI Feedback](./wiki-feedback.md), and [the Gemini maintenance example](/examples/githooks/scripts/gemini-wiki-gap-detection.sh).
4. Run `wiki check --fix` on the touched pages so the fragment links pin.

## References

- [Wiki CLI architecture page](../architecture/wiki-cli.md)
- [Wiki Organization](./wiki-organization.md)