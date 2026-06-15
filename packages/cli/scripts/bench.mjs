#!/usr/bin/env node
// Source-pinned latency benchmark for the everyday `wiki` commands.
//
// Drives a binary built from the source under test against a real corpus,
// spawning it N times per operation and parsing the per-span timings the
// binary writes to `wiki.log` under WIKI_PERF=1. Reports a per-operation
// latency distribution (median + p10/p90) decomposed into the three terms
// every command shares — startup, index preparation (fast-gate walk + any
// refresh), and the command body — with the detected filesystem class
// labelled on every run.
//
// This is a standalone command (`yarn bench`), deliberately NOT wired into
// `yarn validate`, and report-only: it always exits 0. Budgets are shown as
// `within` / `over` markers, not enforced, until the perf fixes land.

import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join, resolve } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));
const CLI_DIR = resolve(HERE, "..");
const WORKSPACE_ROOT = resolve(CLI_DIR, "..", "..");

// ---- args ----------------------------------------------------------------

function parseArgs(argv) {
  const opts = {
    runs: 11,
    warmup: 2,
    json: false,
    bin: process.env.WIKI_BENCH_BIN ?? join(CLI_DIR, "target", "build", "release", "wiki"),
    corpus: WORKSPACE_ROOT,
  };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--json") opts.json = true;
    else if (arg === "--runs") opts.runs = Number(argv[++i]);
    else if (arg === "--warmup") opts.warmup = Number(argv[++i]);
    else if (arg === "--bin") opts.bin = resolve(argv[++i]);
    else if (arg === "--corpus") opts.corpus = resolve(argv[++i]);
    else {
      console.error(`unknown argument: ${arg}`);
      process.exit(2);
    }
  }
  if (!Number.isFinite(opts.runs) || opts.runs < 1) opts.runs = 11;
  if (!Number.isFinite(opts.warmup) || opts.warmup < 0) opts.warmup = 0;
  return opts;
}

// ---- stats ---------------------------------------------------------------

function percentile(sorted, p) {
  if (sorted.length === 0) return null;
  const idx = Math.min(sorted.length - 1, Math.floor((p / 100) * sorted.length));
  return sorted[idx];
}

function summarize(samples) {
  const xs = samples.filter((x) => x != null).sort((a, b) => a - b);
  if (xs.length === 0) return { median: null, p10: null, p90: null, n: 0 };
  return {
    median: percentile(xs, 50),
    p10: percentile(xs, 10),
    p90: percentile(xs, 90),
    n: xs.length,
  };
}

const ms = (x) => (x == null ? "   —  " : `${x.toFixed(1).padStart(6)}`);

// ---- environment ---------------------------------------------------------

function detectFsClass(path) {
  // `stat -f -c %T` reports the filesystem type name (fuseblk, overlayfs,
  // tmpfs, ext4, …). Fail-closed: an undetectable class is `unknown`, and
  // the report flags it so the numbers are never mistaken for the scoreboard.
  const r = spawnSync("stat", ["-f", "-c", "%T", path], { encoding: "utf8" });
  if (r.status !== 0 || !r.stdout) return "unknown";
  return r.stdout.trim();
}

// fuseblk is the hostile scoreboard this tool actually runs on; overlayfs and
// tmpfs are softer and must only ever be a labelled lower-bound sanity check.
const SCOREBOARD_FS = "fuseblk";

// ---- driver --------------------------------------------------------------

const LOG_EVENTS = ["startup", "index.fast_gate", "index.gix_open", "index.refresh"];

function logPath(corpus) {
  return join(corpus, "wiki.log");
}

function readRunEvents(corpus) {
  const path = logPath(corpus);
  if (!existsSync(path)) return [];
  return readFileSync(path, "utf8")
    .split("\n")
    .filter(Boolean)
    .map((line) => {
      try {
        return JSON.parse(line);
      } catch {
        return null;
      }
    })
    .filter(Boolean);
}

function spawnOnce(bin, corpus, args) {
  // Truncate the log so it contains only this invocation's events.
  writeFileSync(logPath(corpus), "");
  const start = process.hrtime.bigint();
  const r = spawnSync(bin, args, {
    cwd: corpus,
    encoding: "utf8",
    env: { ...process.env, WIKI_PERF: "1" },
  });
  const wall = Number(process.hrtime.bigint() - start) / 1e6;

  const events = readRunEvents(corpus);
  const dur = (name) => {
    const e = events.find((ev) => ev.event === name);
    return e ? e.duration_ms : null;
  };
  const commandTotal = dur("command_finish");
  const terms = Object.fromEntries(LOG_EVENTS.map((n) => [n, dur(n)]));
  const overhead = LOG_EVENTS.filter((n) => n !== "startup").reduce(
    (acc, n) => acc + (terms[n] ?? 0),
    0,
  );
  const body = commandTotal == null ? null : Math.max(0, commandTotal - overhead);

  return {
    ok: r.status === 0 || r.status === 1, // check exits 1 on errors found
    status: r.status,
    wall,
    startup: terms.startup,
    fast_gate: terms["index.fast_gate"],
    refresh: terms["index.refresh"],
    body,
    commandTotal,
  };
}

function benchOp(bin, corpus, op, runs, warmup) {
  const raw = [];
  for (let i = 0; i < runs; i++) raw.push(spawnOnce(bin, corpus, op.args));
  const failed = raw.find((r) => !r.ok);
  const measured = raw.slice(warmup); // discard cold-cache warm-ups
  const pick = (key) => summarize(measured.map((r) => r[key]));
  return {
    name: op.name,
    args: op.args,
    failedStatus: failed ? failed.status : null,
    wall: pick("wall"),
    startup: pick("startup"),
    fast_gate: pick("fast_gate"),
    refresh: pick("refresh"),
    body: pick("body"),
    budgetMs: op.budgetMs,
  };
}

// ---- operation set -------------------------------------------------------

// Resolve a summary target that actually exists in the corpus, so the row is
// stable regardless of which repo the benchmark runs against.
function firstPageTitle(bin, corpus) {
  writeFileSync(logPath(corpus), "");
  const r = spawnSync(bin, ["list", "--format", "json"], { cwd: corpus, encoding: "utf8" });
  if (r.status !== 0) return null;
  try {
    const parsed = JSON.parse(r.stdout);
    const rows = Array.isArray(parsed) ? parsed : (parsed.results ?? parsed.pages ?? []);
    return rows[0]?.title ?? null;
  } catch {
    return null;
  }
}

function buildOps(bin, corpus) {
  const summaryTitle = firstPageTitle(bin, corpus);
  const ops = [
    { name: "search", args: ["index", "--format", "json"], budgetMs: 200 },
    { name: "list", args: ["list", "--format", "json"], budgetMs: 200 },
  ];
  if (summaryTitle) {
    ops.push({ name: "summary", args: ["summary", summaryTitle], budgetMs: 200 });
  }
  // `check` validates the corpus directly and stays expensive even on a gate
  // hit, so it gets its own, looser budget and never shares a row.
  ops.push({ name: "check", args: ["check", "--format", "json"], budgetMs: 1000 });
  return ops;
}

// ---- output --------------------------------------------------------------

function printTable(results, env) {
  const scoreboard = env.fsClass === SCOREBOARD_FS;
  console.log("");
  console.log(`wiki benchmark — ${env.binVersion}`);
  console.log(`corpus:  ${env.corpus}`);
  console.log(
    `fs:      ${env.fsClass}` +
      (scoreboard
        ? "  (scoreboard)"
        : `  (NOT the ${SCOREBOARD_FS} scoreboard — treat as a labelled lower-bound only)`),
  );
  console.log(`runs:    ${env.runs} (first ${env.warmup} discarded as warm-up); gate on median`);
  console.log("");
  console.log(
    "op        wall(med/p10/p90)        startup  gate    refresh  body    budget   status",
  );
  console.log(
    "─".repeat(92),
  );
  for (const r of results) {
    const w = r.wall;
    const over = w.median != null && w.median > r.budgetMs;
    const status = r.failedStatus != null && r.failedStatus > 1 ? "ERROR" : over ? "over" : "within";
    console.log(
      `${r.name.padEnd(9)} ` +
        `${ms(w.median)}/${ms(w.p10)}/${ms(w.p90)}  ` +
        `${ms(r.startup.median)}  ${ms(r.fast_gate.median)}  ${ms(r.refresh.median)}  ${ms(r.body.median)}  ` +
        `${String(r.budgetMs).padStart(5)}ms  ${status}`,
    );
  }
  console.log("");
  console.log("All times in ms. wall = full process wall-clock; startup/gate/refresh/body are");
  console.log("median per-term spans from wiki.log. Report-only: this command always exits 0.");
}

// ---- main ----------------------------------------------------------------

function main() {
  const opts = parseArgs(process.argv.slice(2));

  if (!existsSync(opts.bin)) {
    console.error(`benchmark binary not found at ${opts.bin}`);
    console.error("build it first (yarn bench runs the build automatically), or pass --bin <path>.");
    process.exit(2);
  }

  let binVersion = "unknown";
  try {
    binVersion = execFileSync(opts.bin, ["--version"], { encoding: "utf8" }).trim();
  } catch {
    /* leave as unknown */
  }

  const env = {
    binVersion,
    corpus: opts.corpus,
    fsClass: detectFsClass(opts.corpus),
    runs: opts.runs,
    warmup: opts.warmup,
  };

  const ops = buildOps(opts.bin, opts.corpus);
  const results = ops.map((op) => benchOp(opts.bin, opts.corpus, op, opts.runs, opts.warmup));

  if (opts.json) {
    console.log(JSON.stringify({ env, results }, null, 2));
  } else {
    printTable(results, env);
  }

  // Report-only: always succeed.
  process.exit(0);
}

main();
