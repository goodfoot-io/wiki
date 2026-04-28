---
title: Wiki CLI
summary: Fragment link parsing, validation pipeline, and command reference for the wiki CLI tool.
tags:
  - tooling
---

The wiki CLI validates and maintains fragment links between wiki pages and source code. For the maintenance map of every operator-facing doc and automation prompt that should be checked when CLI behavior changes, see [[Wiki Documentation Touchpoints]]. For the git mesh integration that replaces SHA-pinned staleness detection, see [[Wiki Mesh Integration]].

## Fragment Link Parsing

The [parser](packages/cli/src/parser.rs#L6-L12&628d6f9) extracts two link types from markdown content: fragment links (`[label](path#sha-L10-L20)`) and wikilinks (`[[Title]]`). Both parsers operate on scrubbed content — code blocks, inline code, and HTML comments are blanked out before extraction to avoid false matches.

## Validation Pipeline

The [check command](packages/cli/src/commands/check.rs#L28-L29&628d6f9) runs a full validation pass: frontmatter parsing, title/alias collision detection, wikilink resolution, and fragment link verification (file existence and line range bounds at the pinned SHA). With `--fix`, unpinned fragment links are pinned automatically rather than reported as errors — already-pinned links are never touched.

## Extract

The extract command (`packages/cli/src/commands/extract.rs`) reads arbitrary text from stdin, parses all `[[wikilink]]` references, and outputs the canonical title and summary for each resolved page. Wikilink extraction runs before any file I/O — if no wikilinks are found, the command exits immediately with no output. Unresolved wikilinks are reported to stderr and cause exit code 1.

## PostToolUse Hook

The [hook command](packages/cli/src/commands/hook_check.rs#L16-L66) integrates the wiki with external tools like Claude Code. It processes `PostToolUse` JSON events from stdin: when a `.md` file inside the wiki directory is written or edited, it runs `wiki check` on that file and emits a JSON `systemMessage` envelope if validation errors remain, so the AI can address them immediately.

## Navigation and Discovery

Several commands support navigating and searching the wiki from the command line:

- **Incoming Links**: The [links command](packages/cli/src/commands/links.rs#3d1c3e6) finds all wiki pages that reference a given target, whether that target resolves as a wiki page, a workspace file, or both.
- **Search**: The [search command](packages/cli/src/commands/search.rs#e2b1474) is the primary entrypoint for finding wiki content. It performs a weighted search that ranks exact title matches, repo-relative path matches, and full-text matches (BM25) in a single unified flow.
- **Suggest**: The suggest command (used internally by the hook) finds the best matches for a query with a minimum score threshold, prioritizing titles and aliases.
- **Summary**: The [summary command](packages/cli/src/commands/summary.rs#e2b1474) outputs a page's frontmatter-defined summary along with a repo-relative path to its source file.
- **Print**: The [print command](packages/cli/src/commands/print.rs#e2b1474) outputs the full raw markdown content of a wiki page to stdout.

## Rendering and Serving

The [html command](packages/cli/src/commands/html.rs#e2b1474) renders the wiki as a static site. The [serve command](packages/cli/src/commands/serve.rs#6a486f7) starts a local development server with live-reloading to preview changes. It caches the `WikiIndex` in application state to eliminate per-request index rebuilds, using a background worker thread with debouncing to handle file change events. The server supports incremental indexing via `refresh_paths` and defers search index updates to a background catch-up task. Both commands reserve their names in frontmatter to avoid routing conflicts.

## Frontmatter

The [frontmatter module](packages/cli/src/frontmatter.rs#L37-L47&e2b1474) parses and validates YAML frontmatter from wiki pages. It reserves certain titles (`check`, `pin`, `stale`, `links`, `list`, `summary`, `print`, `html`, `serve`) to prevent ambiguity with command-line dispatch.