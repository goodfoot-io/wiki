---
title: Benchmarking Wiki CLI Performance
summary: How to run repeatable, source-pinned latency benchmarks of the everyday wiki commands and decompose where the time goes.
---

This guide describes how to measure the per-operation latency of the everyday `wiki` commands — the default search (`wiki "query"`), `list`, `summary`, and `check` — in a way whose numbers are trustworthy. The goal is a repeatable measurement, not a single eyeballed sample: build the binary from the source under test, run each command enough times to report a distribution, and attribute the cost to the right term (process startup, index preparation, or the command body).

For *why* the CLI is fast in steady state — the index, the fast gate, FTS5 — see [Wiki Performance Optimization](./wiki-performance-optimization.md). This page is the complementary how-to: the procedure for measuring it.

## The quick path: `yarn bench`

The procedure below is implemented as a standalone command. From the repo root:

```bash
yarn bench                 # build the release binary, then benchmark the repo corpus
yarn bench --json          # machine-readable output
```

`yarn bench` builds the `wiki` binary from the source under test, spawns it N times per common command against the repo's own corpus, parses the per-span timings from `wiki.log`, and prints a per-operation table — wall-clock median/p10/p90 plus the startup / gate / refresh / body decomposition — with the detected filesystem class labelled on every run. It is **report-only** (always exits 0) and is deliberately **not** part of `yarn validate`: latency on a hostile filesystem is too variant to gate a commit lane on, so budgets are shown as `within`/`over` markers rather than enforced. The driver lives in [`bench.mjs`](/packages/cli/scripts/bench.mjs).

The rest of this page explains the methodology the command encodes — read it to interpret the numbers, to benchmark by hand, or to extend the driver.

## The two rules that make a number trustworthy

Two methodology mistakes invalidate a benchmark before it starts. Both are easy to make and both have already produced false conclusions about this tool.

1. **Always build from the source under test — never measure a prebuilt binary.** A prebuilt `wiki` on `PATH` or in an extension's `bin/` directory may lag the working tree by many performance-relevant commits. Measuring it tells you nothing about your change. Build a release binary from `packages/cli` and confirm its reported version matches [`Cargo.toml`](/packages/cli/Cargo.toml#L1-L5) before trusting a single number.

2. **Measure on the realistic filesystem, and report a distribution — not a single sample.** The tool's hot path is dominated by filesystem `stat` latency, which the index classifies as hostile or not at [`fs_class.rs`](/packages/cli/src/index/fs_class.rs#L34-L37) (`overlayfs`, `nfs`, `cifs`, and `fuse` are `HostileFs::Yes`). On a hostile mount (e.g. a `fuseblk` devcontainer) every stat is a userspace round-trip, so latencies are bursty and a single `--perf` sample is a draw from a high-variance distribution. Run **N ≥ 20**, report the **median plus a p10/p90 band**, and keep cold and warm runs as distinct, labelled measurements. A cache-dropped or `tmpfs` run is allowed only as an explicitly labelled lower-bound sanity check — never as the headline.

## Build the binary under test

From `packages/cli`, build a release binary. If the default `cargo`/`rustc` on `PATH` is too old, use the rustup shims:

```bash
cd packages/cli
PATH=$HOME/.cargo/bin:$PATH cargo build --release
./target/release/wiki --version    # must match the version in Cargo.toml
```

The binary under test is `packages/cli/target/release/wiki`. Run the rest of the benchmark against that absolute path, not against whatever `wiki` resolves to on `PATH`.

## Pin the scoreboard inputs

Latency depends on match count and corpus state, so fix the exact inputs and the invariant that keeps each one stable. Re-pin if the corpus changes enough to shift a result count.

| row     | exact command                | invariant that keeps it stable      |
|---------|------------------------------|-------------------------------------|
| search  | `wiki "index" --format json` | returns a fixed, small result count |
| list    | `wiki list --format json`    | returns the default page rows       |
| summary | `wiki summary "Billing"`     | resolves to one stable page         |
| check   | `wiki check --format json`   | clean corpus (**0** errors)         |

Avoid queries whose result count drifts as the wiki grows. The `check` row is only meaningful while the corpus stays at 0 errors — assert that as a precondition.

## Measure wall-clock, N ≥ 20

Warm the binary once, then time each command across N runs. `WIKI_PERF=1` writes per-span timings to stderr (and to the perf log) for the decomposition step below.

```bash
B=packages/cli/target/release/wiki
$B warmup >/dev/null 2>&1
for i in $(seq 1 25); do
  t0=$(date +%s%N)
  WIKI_PERF=1 "$B" "index" --format json >/dev/null 2>>spans.txt
  t1=$(date +%s%N)
  awk "BEGIN{printf \"%.3f\n\",($t1-$t0)/1000000.0}"
done   # repeat for: list --format json | summary "Billing" | check --format json
```

Collect the 25 wall-clock samples per command and report `min / p10 / median / p90 / max`. The p10–p90 band on a hostile filesystem can span ~3× the median; that spread is the data, not noise to be averaged away.

## Decompose the time into three terms

A median is not actionable until you know *which* term it lives in. Every subcommand dispatched from [`main.rs`](/packages/cli/src/main.rs#L358-L370) shares one prefix — resolve the repo root, then [`WikiIndex::prepare`](/packages/cli/src/index/mod.rs#L241), then run the command body — so a win or regression must be attributed to the right term:

1. **Startup.** Process spawn plus the in-process repo-root discovery at [`main.rs`](/packages/cli/src/main.rs#L360), which runs *before* the command span starts. This is captured directly by the `startup` event, emitted in [`run`](/packages/cli/src/main.rs#L352) from an `Instant` taken at process entry — so it no longer has to be recovered as `wall − command-span`. A bare `wiki --version` (which skips `prepare`) gives the floor.
2. **Index preparation.** [`prepare`](/packages/cli/src/index/mod.rs#L241) calls the stat-only [`fast_gate`](/packages/cli/src/index/freshness.rs#L18) at [its call site](/packages/cli/src/index/mod.rs#L318-L324); on a gate miss it falls through to a full `index.refresh`. Both the gate walk and the refresh are wrapped in perf spans — `index.fast_gate` and `index.refresh` — so the dominant per-invocation cost (the worktree walk in [`compute_worktree_dir_hash`](/packages/cli/src/index/freshness.rs#L143-L147)) is now visible rather than hiding in an unspanned gap.
3. **Command body.** The work inside the command span itself (the actual search, list, summary, or check), recovered as `command_finish − (fast_gate + gix_open + refresh)`.

Read the spans back out of the perf log to separate the terms:

```bash
WIKI_PERF=1 "$B" "index" --format json >/dev/null 2>spans.txt
grep -E "startup|index\.fast_gate|index\.refresh|index\.gix_open|command\." spans.txt
```

The gap between the command span's start and the first `index.gix_open` event is the gate-walk cost. The `index.refresh` span is the refresh cost. Whatever remains inside the command span is the body.

## Cold vs warm

Report cold and warm as separate rows, never folded into one number:

- **Cold** — first run after the index is invalidated (HEAD moved, the git index changed, or the worktree generation shifted). Pays the full gate walk plus `index.refresh`.
- **Warm** — steady-state repeat where the [fast gate](/packages/cli/src/index/freshness.rs#L18) hits and `prepare` returns without a refresh. This is the "feels fast" path and is the one a budget should be compared against — but only if the gate actually hits on your corpus. If every run pays a refresh, there is no cheap warm path to report, and that itself is a finding worth recording.

## Common pitfalls

- **Stale binary.** The single most common way to publish a wrong number. Re-confirm `--version` after every rebuild.
- **A single `--perf` sample.** One draw from a high-variance distribution. Always N ≥ 20 with a reported band.
- **`tmpfs` as the headline.** It factors out exactly the OS-cache-sensitive stat costs that dominate the real environment. Use it only as a labelled lower bound.
- **Unequal inputs across runs.** A query whose result count drifts changes the body cost and corrupts the comparison. Pin inputs and their invariants.

See also: [Wiki Performance Optimization](./wiki-performance-optimization.md) · [Wiki CLI](../architecture/wiki-cli.md)
