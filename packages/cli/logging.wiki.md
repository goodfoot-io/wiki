---
title: Wiki Logging and Perf Instrumentation
summary: Documents all logging and performance tracing points in the wiki CLI. 
---

## Overview

The wiki CLI uses two complementary logging systems:

1. **Perf Instrumentation** ([`perf::scope_result`](./src/perf.rs#L119-L129) and [`perf::scope_async_result`](./src/perf.rs#L131-L140)): Measures performance and records operational metrics to `wiki.log` (or path specified by `WIKI_DIR` env var). Outputs structured JSON events with timing, status, and metadata.

2. **Direct Output** (`println!` and `eprintln!`): Writes user-facing messages to stdout/stderr for command results, errors, and status messages.

The perf module writes to: `$WIKI_DIR/wiki.log` (default: `./wiki/wiki.log`). Each event is a JSON object on a single line containing timestamp, invocation ID, PID, event name, duration, status, and metadata.

## Perf Instrumentation Points

Perf scope events measure execution time and record success/error status. They are organized by module below.

### Command Lifecycle

| Location | Scope Name | Measures | Metadata |
|----------|-----------|----------|----------|
| [main.rs](./src/main.rs#L327-L330) | `command.<name>` | Total wall time of the command (stderr span only; not written to `wiki.log`) | — |

### Index Refresh

These scopes cover the cold-cache path: when the stat-only freshness gate misses, [`WikiIndex::prepare_for_source`](./src/index/mod.rs#L348-L368) opens the repository and drives the three-pass refresh.

| Location | Scope Name | Measures | Metadata |
|----------|-----------|----------|----------|
| [index/mod.rs](./src/index/mod.rs#L353-L356) | `index.gix_open` | Time to open the gix repository for a refresh | Empty object |
| [index/mod.rs](./src/index/mod.rs#L357-L366) | `index.refresh` | Total three-pass refresh including delta apply and commit | Empty object |
| [index/passes/mod.rs](./src/index/passes/mod.rs#L126-L129) | `index.pass_tree` | Pass 1: diff `HEAD^{tree}` against the previously indexed tree | Empty object |
| [index/passes/mod.rs](./src/index/passes/mod.rs#L139-L142) | `index.pass_index` | Pass 2: git index entry scan | Empty object |
| [index/passes/mod.rs](./src/index/passes/mod.rs#L147-L157) | `index.pass_worktree` | Pass 3: worktree walk, read + hash of candidate markdown | Empty object |
| [index/passes/mod.rs](./src/index/passes/mod.rs#L185-L213) | `index.apply_deltas` | Applying all merged deltas (blob parse, FTS insert, paths/refcount bookkeeping) | `deltas` (count) |
| [index/passes/mod.rs](./src/index/passes/mod.rs#L272-L272) | `index.commit` | SQLite transaction commit | Empty object |

### File Discovery

| Location | Scope Name | Measures | Metadata |
|----------|-----------|----------|----------|
| [commands/mod.rs](./src/commands/mod.rs#L199-L266) | `discover_files` | Time to resolve glob patterns and find wiki markdown files | `globs` (array of glob patterns) |
| [commands/mod.rs](./src/commands/mod.rs#L253-L260) | `discover_files_result` | Zero-duration marker carrying the discovered-file count | `count` |

## Direct Output Points (println! and eprintln!)

### main.rs

| Line | Message | Purpose |
|------|---------|---------|
| 261 | `"\n---"` | Output separator in markdown format |
| 272 | `"wiki {}"` (version) | Display CLI version |
| 290 | JSON error object | JSON-formatted error output (when `--json` flag is set) |
| 292 | `"{e:?}"` | Debug format error output (when not `--json`) |
| 372 | `"error: {error_message}"` | Generic error output |
| 404-405 | `"\n---\n"` and content | Output markdown with separator |
| 408 | Blank line | Output spacing |

### commands/serve.rs

| Line | Message | Purpose |
|------|---------|---------|
| 82 | `"Serving wiki on http://0.0.0.0:{port}"` | Status message when HTTP server starts |
| 122 | `"wiki: failed to rebuild index after file change: {error}"` | Error when file watcher detects changes but index rebuild fails |

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

### commands/check.rs

| Line | Message | Purpose |
|------|---------|---------|
| 34 | JSON error object | Formatted error for diagnostics |
| 36 | `"error: {e}"` | Error message |
| 399 | JSON diagnostics array | Structured JSON output of checks |
| 405 | `"**{kind}** — `{file}:{line}`"` | Formatted diagnostic with location and message |
| 414 | `"Fixed {count} file(s)."` | Summary of auto-fixes applied |

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

- **command_start**: Logged at [initialization](./src/perf.rs#L67-L96) with command name, json_output flag, and repo_root
- **command_finish**: Logged at [completion](./src/perf.rs#L98-L108) with exit code and total runtime

### Log Rotation

Log files are append-only and live in `$WIKI_DIR/wiki.log`. No automatic rotation is performed; external tools can archive or rotate the log as needed.
