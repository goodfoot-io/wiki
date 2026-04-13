---
title: Wiki Documentation Touchpoints
summary: Canonical map of wiki CLI implementation, operator instructions, and maintenance references that must stay aligned when wiki documentation behavior changes.
tags:
  - meta
  - wiki
  - tooling
---

This page is the maintenance map for future wiki documentation updates. When command behavior or recommended usage changes, update the implementation-facing source of truth first, then walk the operator-facing documents and automation references listed here so guidance does not drift.

For the CLI architecture itself, see [[Wiki CLI]]. For the broader rules governing wiki pages, see [[Wiki Organization]] and [[Wiki CLI Advanced Usage]].

## Command Behavior Source Of Truth

The primary source of truth for top-level CLI behavior is the [Clap configuration and dispatch in `packages/cli/src/main.rs`](wiki/meta/packages/cli/src/main.rs#L26-L60). That block defines the help text, the `query` positional argument, and the reserved subcommand set. The [top-level `run(...)` match in the same file](wiki/meta/packages/cli/src/main.rs#L292-L374) is what decides that bare `wiki [query]` executes ranked lookup rather than page printing.

When documentation changes are about lookup semantics, read the [search semantics plan notes](wiki/meta/documentation/wiki-index-plan.md#L68-L125) as design context rather than user-facing contract. They capture intent around ranking, fallback lookup, and future search behavior that may need to be reflected in docs once implemented.

## Operator-Facing Documentation

These files are the public guidance surfaces most likely to drift when the CLI contract changes:

- The [wiki README CLI section](wiki/meta/wiki/README.md#L29-L82) is the reader-facing quick reference for `wiki check`, `wiki summary`, and default `wiki [query]` lookup.
- The [repository `CLAUDE.md` wiki instructions](CLAUDE.md#L83-L94) shape how agents in this workspace are told to search and read wiki content.
- The [wiki skill instructions](.claude/skills/wiki/SKILL.md#L73-L172) are the highest-leverage agent workflow contract for discovering pages, choosing where to write, validating fragment links, and updating related pages.
- The [advanced usage page](wiki/reference/wiki-cli-advanced-usage.md#L8-L132) holds the less common CLI behaviors such as stdin handling, file paths, explicit glob targeting, and JSON output.
- The [feedback log](wiki/meta/wiki-feedback.md#L11-L21) is where observed friction from doc or CLI mismatches should be recorded after the change is understood.

If a documentation update changes the recommended operator workflow, all of these surfaces should be checked explicitly, not only the page that first exposed the inconsistency.

## Automation And Maintenance Touchpoints

The [Gemini wiki gap-detection script prompt](wiki/meta/scripts/gemini-wiki-gap-detection.sh#L261-L276) embeds wiki search guidance for automated maintenance work. If the preferred search invocation or page-discovery workflow changes, this prompt must stay aligned with the human-facing docs or automation will continue reinforcing stale instructions.

The [wiki skill’s maintenance reference](.claude/skills/wiki/references/maintenance.md#L1-L104) defines the stale-page repair workflow. It is not a user-facing quickstart, but it is part of the operational contract for wiki upkeep and should be checked whenever the update changes validation, pinning, or page-discovery expectations.

## Update Order

When wiki documentation behavior changes, use this order:

1. Confirm the implementation in [CLI parsing and dispatch](wiki/meta/packages/cli/src/main.rs#L26-L60) and [top-level command routing](wiki/meta/packages/cli/src/main.rs#L292-L374).
2. Update the primary user docs in [wiki/README.md](wiki/meta/wiki/README.md#L29-L82) and [CLAUDE.md](CLAUDE.md#L83-L94).
3. Update the agent workflow contract in [.claude/skills/wiki/SKILL.md](.claude/skills/wiki/SKILL.md#L73-L172).
4. Update secondary references such as [Wiki CLI Advanced Usage](wiki/reference/wiki-cli-advanced-usage.md#L8-L132), [Wiki CLI Feedback](wiki/meta/wiki-feedback.md#L11-L21), and [the Gemini maintenance prompt](wiki/meta/scripts/gemini-wiki-gap-detection.sh#L261-L276).
5. Run `wiki check --fix` on the touched pages so the fragment links pin and the page participates in `wiki stale`.

## References

- [Wiki CLI architecture page](wiki/architecture/wiki-cli.md#L8-L54)
- [Wiki Organization](wiki/meta/wiki-organization.md#L9-L67)
40a03d#L9-L67)