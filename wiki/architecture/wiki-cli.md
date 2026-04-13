---
title: Wiki CLI
summary: Fragment link parsing, staleness detection, and validation pipeline for the wiki CLI tool.
tags:
  - tooling
---

The wiki CLI validates and maintains fragment links between wiki pages and source code. For the maintenance map of every operator-facing doc and automation prompt that should be checked when CLI behavior changes, see [[Wiki Documentation Touchpoints]].

## Fragment Link Parsing

The [parser](packages/cli/src/parser.rs#L6-L12&e2b1474) extracts two link types from markdown content: fragment links (`[label](path#sha-L10-L20)`) and wikilinks (`[[Title]]`). Both parsers operate on scrubbed content — code blocks, inline code, and HTML comments are blanked out before extraction to avoid false matches.

## Staleness Detection

The [stale command](packages/cli/src/commands/stale.rs#L41-L54&e2b1474) compares each pinned SHA against the current HEAD to find fragment links whose referenced files have changed. It reports the number of commits since the pin and optionally includes a diff. For performance, it caches Git operation results (commits, stats, and patches) when multiple fragment links reference the same file and SHA.

## Validation Pipeline

The [check command](packages/cli/src/commands/check.rs#L28-L29&e2b1474) runs a full validation pass: frontmatter parsing, title/alias collision detection, wikilink resolution, and fragment link verification (file existence and line range bounds at the pinned SHA). With `--fix`, unpinned fragment links are pinned automatically rather than reported as errors — already-pinned links are never touched.

## Extract

The extract command (`packages/cli/src/commands/extract.rs`) reads arbitrary text from stdin, parses all `[[wikilink]]` references, and outputs the canonical title and summary for each resolved page. Wikilink extraction runs before any file I/O — if no wikilinks are found, the command exits immediately with no output. Unresolved wikilinks are reported to stderr and cause exit code 1.

## Context Injection Hook

The [hook command](packages/cli/src/commands/hook.rs#e4b76c2ef) integrates the wiki with external tools like Claude Code. It processes `PostToolUse` JSON events from stdin and injects relevant wiki context into the system prompt.

### Suppression Logic

To avoid circularity, the hook [suppresses injection](packages/cli/src/commands/hook.rs#L33-L45&e2b1474) when the tool is operating on a wiki document. This is determined by checking if the file is inside the wiki directory or has a `.wiki.md` extension.

### Session Deduplication

To minimize prompt noise, the hook [tracks which file-path lookups have been shown](packages/cli/src/commands/hook.rs#L77-L87&e2b1474) in a given session. If a page that references the current file has already been injected in the current session, it is skipped. Wikilinks explicitly mentioned in tool output are always injected, regardless of the session state.

## Navigation and Discovery

Several commands support navigating and searching the wiki from the command line:

- **Incoming Links**: The [links command](packages/cli/src/commands/links.rs#e4b76c2ef) finds all wiki pages that reference a given target, whether that target resolves as a wiki page, a workspace file, or both.
- **Search**: The [search command](packages/cli/src/commands/search.rs#e4b76c2ef) is the primary entrypoint for finding wiki content. It performs a weighted search that ranks exact title matches, repo-relative path matches, and full-text matches (BM25) in a single unified flow.
- **Suggest**: The suggest command (used internally by the hook) finds the best matches for a query with a minimum score threshold, prioritizing titles and aliases.
- **Summary**: The [summary command](packages/cli/src/commands/summary.rs#e4b76c2ef) outputs a page's frontmatter-defined summary along with a repo-relative path to its source file.
- **Print**: The [print command](packages/cli/src/commands/print.rs#5ca0f4050) outputs the full raw markdown content of a wiki page to stdout.

## Rendering and Serving

The [html command](packages/cli/src/commands/html.rs#e4b76c2ef) renders the wiki as a static site. The [serve command](packages/cli/src/commands/serve.rs#81acdb20d) starts a local development server with live-reloading to preview changes. It caches the `WikiIndex` in application state to eliminate per-request index rebuilds, using a background worker thread with debouncing to handle file change events. The server supports incremental indexing via `refresh_paths` and defers search index updates to a background catch-up task. Both commands reserve their names in frontmatter to avoid routing conflicts.

## Frontmatter

The [frontmatter module](packages/cli/src/frontmatter.rs#L37-L47&e2b1474) parses and validates YAML frontmatter from wiki pages. It reserves certain titles (`check`, `pin`, `stale`, `links`, `list`, `summary`, `print`, `html`, `serve`) to prevent ambiguity with command-line dispatch.