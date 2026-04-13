---
title: Wiki Logging and Perf Instrumentation
summary: Documents all logging and performance tracing points in the wiki CLI.
---

## Overview

The wiki CLI uses two complementary logging systems:

1. **Perf Instrumentation** (`perf::scope_result` and `perf::scope_async_result`): Measures performance and records operational metrics to `wiki.log` (or path specified by `WIKI_DIR` env var). Outputs structured JSON events with timing, status, and metadata.

2. **Direct Output** (`println!` and `eprintln!`): Writes user-facing messages to stdout/stderr for command results, errors, and status messages.

The perf module writes to: `$WIKI_DIR/wiki.log` (default: `./wiki/wiki.log`). Each event is a JSON object on a single line containing timestamp, invocation ID, PID, event name, duration, status, and metadata.

## Perf Instrumentation Points

Perf scope events measure execution time and record success/error status. They are organized by module below.

### Index Initialization and Management

| Location | Scope Name | Measures | Metadata |
|----------|-----------|----------|----------|
| packages/wiki/src/index.rs:126 | `index.prepare` | Total time to prepare wiki index (open DB, bootstrap schema, verify integrity, sync) | Empty object |
| packages/wiki/src/index.rs:136 | `index.open_database` | Time to connect to SQLite database | `db_path` (database file location) |
| packages/wiki/src/index.rs:159 | `index.bootstrap_schema` | Time to create or verify database schema | Empty object |
| packages/wiki/src/index.rs:165 | `index.verify_integrity` | Time to run database integrity checks | Empty object |
| packages/wiki/src/index.rs:171 | `index.sync` | Time to synchronize wiki files with database index | Empty object |

### Index Syncing and Updates

| Location | Scope Name | Measures | Metadata |
|----------|-----------|----------|----------|
| packages/wiki/src/index.rs:375 | `index.discover_files` | Time to scan filesystem for wiki markdown files | Empty object |
| packages/wiki/src/index.rs:379 | `index.load_existing_documents` | Time to query database for currently indexed documents | Empty object |
| packages/wiki/src/index.rs:502 | `index.validate_lookup_collisions` | Time to detect duplicate lookup keys and title conflicts | `pending_documents` (new/modified docs), `changed_or_new` (count), `stale_paths` (deleted docs) |
| packages/wiki/src/index.rs:525 | `index.begin_transaction` | Time to start database transaction | Empty object |
| packages/wiki/src/index.rs:728 | `index.commit_transaction` | Time to commit database transaction | Empty object |

### Query Operations

| Location | Scope Name | Measures | Metadata |
|----------|-----------|----------|----------|
| packages/wiki/src/index.rs:843 | `index.resolve_page` | Time to look up a page by title or path | `input` (search term), `input_kind` ("path" or "lookup") |
| packages/wiki/src/index.rs:937 | `index.search` | Time to execute full-text search query | `query` (FTS query string), `limit` (max results), `min_score` (relevance threshold), `broad` (broad search mode) |
| packages/wiki/src/index.rs:1041 | `index.list_pages` | Time to list all or tagged pages | `tag` (filter tag, or null for all) |
| packages/wiki/src/index.rs:1098 | `index.backlinks` | Time to retrieve pages that link to a document | `document_id` (target document ID) |
| packages/wiki/src/index.rs:1149 | `index.extract_pages` | Time to retrieve multiple pages by title | `title_count` (number of titles) |

### File Discovery

| Location | Scope Name | Measures | Metadata |
|----------|-----------|----------|----------|
| packages/wiki/src/commands/mod.rs:131 | `discover_files` | Time to resolve glob patterns and find wiki markdown files | `globs` (array of glob patterns) |

## Direct Output Points (println! and eprintln!)

### main.rs

| Line | Message | Purpose |
|------|---------|---------|
| 261 | `"\n---"` | Output separator in markdown format |
| 272 | `"wiki {}"` (version) | Display CLI version |
| 290 | JSON error object | JSON-formatted error output (when `--json` flag is set) |
| 292 | `"{e:?}"` | Debug format error output (when not `--json`) |
| 333 | `"error: one of --claude or --codex must be provided"` | Validation error for search command |
| 372 | `"error: {error_message}"` | Generic error output |
| 404-405 | `"\n---\n"` and content | Output markdown with separator |
| 408 | Blank line | Output spacing |

### commands/serve.rs

| Line | Message | Purpose |
|------|---------|---------|
| 82 | `"Serving wiki on http://0.0.0.0:{port}"` | Status message when HTTP server starts |
| 122 | `"wiki: failed to rebuild index after file change: {error}"` | Error when file watcher detects changes but index rebuild fails |

### commands/stale.rs

| Line | Message | Purpose |
|------|---------|---------|
| 46 | JSON error object | Formatted error for stale link detection |
| 48 | `"error: {e}"` | Error message |
| 61 | `"warning: failed to read {path}: {e}"` | Warning when unable to read file |
| 116-152 | Results and separators | Stale link report output (JSON or formatted text) |
| 155 | `"error: {message}"` | Error message |

### commands/html.rs

| Line | Message | Purpose |
|------|---------|---------|
| 52 | "Page not found" error with suggestions | Error output when HTML render target doesn't exist |

### commands/pin.rs

| Line | Message | Purpose |
|------|---------|---------|
| 34 | JSON error object | Formatted error for pin operations |
| 36 | `"error: failed to resolve ref '{ref_name}': {e}"` | Error resolving git ref |
| 46 | JSON error object | Formatted error |
| 48 | `"error: {e}"` | Error message |
| 61 | `"warning: failed to read {path}: {e}"` | Warning when reading file fails |
| 101 | `"error: {message}"` | Error message |
| 169 | `"error: failed to write {path}: {e}"` | Error when writing pin entries |
| 176 | JSON pin entries | Output of pin command (JSON format) |
| 180 | Pin entries (formatted) | Formatted text output of pin entries |

### commands/print.rs

| Line | Message | Purpose |
|------|---------|---------|
| 19 | JSON output | Structured JSON output of page content |
| 28 | `"error: {message}"` | Error message |
| 36 | "Page not found" error with suggestions | Error when page doesn't exist |

### commands/summary.rs

| Line | Message | Purpose |
|------|---------|---------|
| 69 | JSON summary object | Structured JSON output |
| 71 | Formatted summary text | Human-readable summary output |
| 78 | `"error: {message}"` | Error message |
| 86 | "Page not found" error with suggestions | Error when page doesn't exist |

### commands/extract.rs

| Line | Message | Purpose |
|------|---------|---------|
| 30 | `"[]"` | Empty array (when no extraction requested) |
| 48 | `"No page found with title or alias `{title}`."` | Error when page not found |
| 52 | JSON array of entries | Structured JSON output of extracted entries |
| 55 | `"**{title}** — {summary}"` | Formatted text output of entries |

### commands/search.rs

| Line | Message | Purpose |
|------|---------|---------|
| 15 | `"[]"` | Empty JSON array (no matches) |
| 21 | JSON search results | Structured JSON output |
| 25 | Blank line | Spacing in formatted output |
| 27 | Formatted search result | Human-readable result entry |

### commands/list.rs

| Line | Message | Purpose |
|------|---------|---------|
| 26 | JSON array of entries | Structured JSON output |
| 29 | `"**{title}** — `{file}`"` | Formatted entry with file location |
| 54 | Metadata string | Joined metadata fields (tags, etc.) |
| 56 | Summary with separators | Formatted summary output |

### commands/refs.rs

| Line | Message | Purpose |
|------|---------|---------|
| 28 | `"[]"` | Empty JSON array (no references) |
| 34 | JSON references array | Structured JSON output |
| 38 | Blank line | Spacing in formatted output |
| 40 | Formatted reference result | Human-readable reference entry |

### commands/backlinks.rs

| Line | Message | Purpose |
|------|---------|---------|
| 22 | `"error: {message}"` | Error message |
| 30 | "Page not found" error with suggestions | Error when page doesn't exist |
| 47 | JSON output | Structured JSON backlinks data |
| 49 | `"**{title}** — `{file}`"` | Formatted backlink entry |
| 51 | `"_(no backlinks)_"` | Notice when no backlinks found |
| 53 | Blank line | Spacing |
| 55 | Formatted backlink text | Human-readable backlink with context |

### commands/check.rs

| Line | Message | Purpose |
|------|---------|---------|
| 34 | JSON error object | Formatted error for diagnostics |
| 36 | `"error: {e}"` | Error message |
| 399 | JSON diagnostics array | Structured JSON output of checks |
| 405 | `"**{kind}** — `{file}:{line}`"` | Formatted diagnostic with location and message |
| 414 | `"Fixed {count} file(s)."` | Summary of auto-fixes applied |

### commands/hook.rs

| Line | Message | Purpose |
|------|---------|---------|
| 146 | JSON envelope | Structured JSON output for hook invocation |

## Log File Format

Events written to `wiki.log` follow this JSON schema:

```json
{
  "timestamp_ms": 1712579206234,
  "invocation_id": "12345-1712579206234",
  "pid": 12345,
  "event": "index.prepare",
  "duration_ms": 45.23,
  "status": "ok|error",
  "meta": { /* scope-specific metadata */ }
}
```

- **timestamp_ms**: Unix millisecond timestamp when event occurred
- **invocation_id**: Unique ID combining process ID and invocation timestamp for grouping related events
- **pid**: Operating system process ID
- **event**: Event name (perf scope name or lifecycle event)
- **duration_ms**: Elapsed time in milliseconds (0.0 for non-timed events)
- **status**: "ok" for success, "error" for failures
- **meta**: Scope-specific metadata (varies per event type)

### Lifecycle Events

Two special events mark command execution boundaries:

- **command_start**: Logged at initialization with command name, json_output flag, and repo_root
- **command_finish**: Logged at completion with exit code and total runtime

### Log Rotation

Log files are append-only and live in `$WIKI_DIR/wiki.log`. No automatic rotation is performed; external tools can archive or rotate the log as needed.
