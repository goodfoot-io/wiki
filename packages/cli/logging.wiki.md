---
title: Wiki Logging and Perf Instrumentation
summary: Documents all logging and performance tracing points in the wiki CLI. 
links-reviewed: 8
---

## Overview

The wiki CLI uses two complementary logging systems:

1. **Perf Instrumentation** ([`perf::scope_result`](./src/perf.rs#L203-L213) and [`perf::log_event`](./src/perf.rs#L194-L201)): Measures performance and records operational metrics to `wiki.log`. Outputs structured JSON events with timing, status, and metadata.

2. **Direct Output** (`println!` and `eprintln!`): Writes user-facing messages to stdout/stderr for command results, errors, and status messages.

The perf module writes to `<common-git-dir>/wiki/wiki.log` — the store directory beside the repository's git dir, never the working tree ([perf.rs](./src/perf.rs#L215-L236)). Each event is a JSON object on a single line containing timestamp, invocation ID, PID, event name, duration, status, and metadata.

## Perf Instrumentation Points

Perf scope events measure execution time and record success/error status. They are organized by module below.

### Command Lifecycle

| Location | Scope Name | Measures | Metadata |
|----------|-----------|----------|----------|
| [main.rs](./src/main.rs#L310) | `command.<name>` | Total wall time of the command (stderr span only; not written to `wiki.log`) | — |

### Index Refresh

These scopes cover the cold-cache path: when the stat-only freshness gate misses, [`WikiIndex::prepare_for_source`](./src/index/mod.rs#L323-L327) resolves the digest gate and drives the three-pass refresh onto the generations store.

| Location | Scope Name | Measures | Metadata |
|----------|-----------|----------|----------|
| [index/mod.rs](./src/index/mod.rs#L450-L455) | `index.gix_open` | Time to open the gix repository for a refresh | Empty object |
| [index/mod.rs](./src/index/mod.rs#L454-L463) | `index.refresh` | Total three-pass refresh (candidate building against the base generation) | Empty object |
| [index/passes/mod.rs](./src/index/passes/mod.rs#L215-L269) | `index.pass_tree` | Pass 1: diff `HEAD^{tree}` against the previously indexed tree | Empty object |
| [index/passes/mod.rs](./src/index/passes/mod.rs#L271-L283) | `index.pass_index` | Pass 2: git index entry scan | Empty object |
| [index/passes/mod.rs](./src/index/passes/mod.rs#L285-L304) | `index.pass_worktree` | Pass 3: worktree walk, read + hash of candidate markdown | Empty object |
| [index/passes/mod.rs](./src/index/passes/mod.rs#L306-L366) | `index.apply_deltas` | Building the publish candidate from merged deltas (blob parse, gen_paths membership, refcount reconciliation) | `deltas` (count) |
| [index/passes/mod.rs](./src/index/passes/mod.rs#L372-L377) | `index.publish` | Publishing the generation: fts materialization + transactional store write | Empty object |

### File Discovery

| Location | Scope Name | Measures | Metadata |
|----------|-----------|----------|----------|
| [commands/mod.rs](./src/commands/mod.rs#L238-L310) | `discover_files` | Time to resolve glob patterns and find wiki markdown files | `globs` (array of glob patterns) |
| [commands/mod.rs](./src/commands/mod.rs#L299-L306) | `discover_files_result` | Zero-duration marker carrying the discovered-file count | `count` |

### Anchor Cache

The disposable anchor-cache tiers inside [`wiki check`](./src/commands/check.rs) — the fingerprint tier (per-link rk64 of the certified target range) and the anchor tier (per-page anchor-epoch walk) — report through **one aggregated event per run** (plan decision 7): per-link or per-page events would flood `wiki.log` (a 10k-link corpus → 10k+ lines per run).

| Location | Event Name | Meaning |
|----------|-----------|----------|
| [check.rs](./src/commands/check.rs#L290-L294) | `anchor_cache` | Emitted once per check invocation after the run body, on every path — early exits included. `meta.hits`, `meta.misses`, `meta.bypasses` tally the row-level outcomes across both tiers; `meta.fingerprint_ms` and `meta.walk_ms` sum each tier's git-leg durations ([drift.rs](./src/commands/drift.rs#L1717) and [drift.rs](./src/commands/drift.rs#L404)), recorded on the miss path only — a served hit runs no git leg — so a fully warm run reports zeros. |

The tally sites live at the tier seams: the shallow gate ([drift.rs](./src/commands/drift.rs#L302-L303)), the verified-hit serves ([drift.rs](./src/commands/drift.rs#L317-L318), [drift.rs](./src/commands/drift.rs#L1667-L1669)), and the misses that precede computing ([drift.rs](./src/commands/drift.rs#L323-L325), [drift.rs](./src/commands/drift.rs#L1670-L1672)).

## Direct Output Points (println! and eprintln!)

### main.rs

| Line | Message | Purpose |
|------|---------|---------|
| 228 | `"\n---"` | Output separator in markdown format |
| 243 | `"wiki {}"` (version) | Display CLI version |
| 276 | JSON error object | JSON-formatted error output (when `--json` flag is set) |
| 278 | `"{e:?}"` | Debug format error output (when not `--json`) |
| 345 | `"error: --fix requires --source=worktree"` | Rejected flag combination |
| 384 | Blank line | Trailing newline after the no-subcommand help output |
| 440 | `"warning: rendezvous lock unavailable ({e}); proceeding without it"` | Rendezvous lock degraded to an unserialized run |

### commands/check.rs

| Line | Message | Purpose |
|------|---------|---------|
| 317 | JSON error object | Formatted error for diagnostics |
| 319 | `"error: {err}"` | Error message |
| 496 | JSON fix plan | Structured `--fix --dry-run --json` plan: fixes, skipped, unverified, certificationSkips, errors |
| 508 | `"no fixes to apply"` | Dry-run plan with nothing to fix or skip |
| 511 | `"fix: {file} line {line}: {old} -> {new}"` | Dry-run fix entry |
| 517 | `"skip: {file} line {line}: {reason}"` | Dry-run skipped fix |
| 532 | `"{path}"` | Applied-path echo under `--print-applied` |
| 543, 545 | `"fixed: {file}:{line}  broken_link  {old} → {new}  ({reason})"` | Applied fix — stderr under `--print-applied`, stdout otherwise |
| 554, 556 | `"skipped: {file}:{line}  broken_link  reason: {reason}"` | Skipped fix — stderr under `--print-applied`, stdout otherwise |
| 581 | JSON fix result | Structured `--fix --json` result |
| 613 | JSON diagnostics array | Structured JSON output of checks |
| 618 | Formatted diagnostics | Rendered diagnostic output (when not `--json`) |
| 872 | Cache directory path | `--clear-cache` echo of the anchor cache location |
| 874 | `"warning: anchor cache busy; not cleared"` | Cache clear skipped because the lock is held |

### commands/summary.rs

| Line | Message | Purpose |
|------|---------|---------|
| 78 | JSON summary object | Structured JSON output |
| 80 | Formatted summary text | Human-readable summary output |
| 87 | JSON error object | Formatted error for a missing page |
| 95 | "Page not found" error with suggestions | Error when page doesn't exist |

### commands/search.rs

| Line | Message | Purpose |
|------|---------|---------|
| 22 | `"[]"` | Empty JSON array (no matches) |
| 28 | JSON search results | Structured JSON output |
| 32 | Blank line | Spacing in formatted output |
| 34 | Formatted search result | Human-readable result entry |
| 38 | `"*{remaining} other wiki matches.*"` | Truncation notice |

### commands/list.rs

| Line | Message | Purpose |
|------|---------|---------|
| 33, 50, 53, 61 | JSON array of entries | Structured JSON output (opened, comma-separated, closed) |
| 68 | `"**{title}** — `{file}`"` | Formatted entry with file location |
| 93 | Metadata string | Joined alias and tag fields |
| 95 | Summary with separators | Formatted summary block |

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

- **command_start**: Logged at [initialization](./src/perf.rs#L152-L180) with command name and json_output flag
- **command_finish**: Logged at [completion](./src/perf.rs#L182-L192) with exit code and total runtime

### Log Rotation

Log files are append-only and live in `<common-git-dir>/wiki/wiki.log`. No automatic rotation is performed; external tools can archive or rotate the log as needed.
