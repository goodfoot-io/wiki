---
title: Wiki CLI
summary: Fragment link parsing, validation pipeline, and command reference for the wiki CLI tool.
tags:
  - tooling
---

The wiki CLI validates and maintains fragment links between wiki pages and source code. For the maintenance map of every operator-facing doc and automation prompt that should be checked when CLI behavior changes, see [Wiki Documentation Touchpoints](../meta/wiki-documentation-touchpoints.md). For the git mesh integration that replaces SHA-pinned staleness detection, see [Wiki Mesh Integration](./wiki-mesh-integration.md).

## Fragment Link Parsing

The [parser](/packages/cli/src/parser.rs#L6-L12) extracts fragment links from markdown content: [fragment links](/packages/cli/src/parser.rs#L213-L213) (`[label](path#sha-L10-L20)`). The parser operates on [scrubbed content](/packages/cli/src/parser.rs#L79-L79) — code blocks, inline code, and HTML comments are blanked out before extraction to avoid false matches.

## Validation Pipeline

The [check command](/packages/cli/src/commands/check.rs#L28-L29) runs a full validation pass: [frontmatter parsing](/packages/cli/src/frontmatter.rs#L56-L56), title/alias collision detection, wikilink resolution, and fragment link verification (file existence and line range bounds at the pinned SHA). With `--fix`, unpinned fragment links are pinned automatically rather than reported as errors — already-pinned links are never touched.

## PostToolUse Hook

The [hook command](/packages/cli/src/commands/hook_check.rs#L16-L63) integrates the wiki with external tools like Claude Code. It processes `PostToolUse` JSON events from stdin: when a `.md` file inside the wiki directory is written or edited, it runs `wiki check` on that file and emits a JSON `systemMessage` envelope if validation errors remain, so the AI can address them immediately.

## Navigation and Discovery

Several commands support navigating and searching the wiki from the command line:

- **Incoming Links**: The [links command](/packages/cli/src/commands/links.rs) finds all wiki pages that reference a given target, whether that target resolves as a wiki page, a workspace file, or both.
- **Search**: The [search command](/packages/cli/src/commands/search.rs) is the primary entrypoint for finding wiki content. It performs a weighted search that ranks exact title matches, repo-relative path matches, and full-text matches (BM25) in a single unified flow.
- **Suggest**: The suggest command (used internally by the hook) finds the best matches for a query with a minimum score threshold, prioritizing titles and aliases.
- **Summary**: The [summary command](/packages/cli/src/commands/summary.rs#L130-L130) outputs a page's frontmatter-defined summary along with a repo-relative path to its source file.

## Rendering

The CLI does not render markdown. HTML rendering is owned entirely by the VS Code extension's webview, which reads pages directly from disk; the CLI's responsibilities stop at read, search, validation, and indexing.

## Frontmatter

The [frontmatter module](/packages/cli/src/frontmatter.rs#L45-L50) parses and validates YAML frontmatter from wiki pages. It [reserves certain titles](/packages/cli/src/frontmatter.rs#L48-L50) (`check`, `pin`, `stale`, `links`, `list`, `summary`, `print`) to prevent ambiguity with command-line dispatch.