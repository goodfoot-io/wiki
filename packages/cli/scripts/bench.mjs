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
import { existsSync, mkdirSync, mkdtempSync, readFileSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
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
    coldRuns: 11,
    trials: 1,
    json: false,
    anchorCache: false,
    anchorRuns: 5,
    anchorCommits: 10000,
    bin: process.env.WIKI_BENCH_BIN ?? join(CLI_DIR, "target", "build", "release", "wiki"),
    corpus: WORKSPACE_ROOT,
  };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--json") opts.json = true;
    else if (arg === "--anchor-cache") opts.anchorCache = true;
    else if (arg === "--runs") opts.runs = Number(argv[++i]);
    else if (arg === "--warmup") opts.warmup = Number(argv[++i]);
    else if (arg === "--cold-runs") opts.coldRuns = Number(argv[++i]);
    else if (arg === "--trials") opts.trials = Number(argv[++i]);
    else if (arg === "--anchor-runs") opts.anchorRuns = Number(argv[++i]);
    else if (arg === "--anchor-commits") opts.anchorCommits = Number(argv[++i]);
    else if (arg === "--bin") opts.bin = resolve(argv[++i]);
    else if (arg === "--corpus") opts.corpus = resolve(argv[++i]);
    else {
      console.error(`unknown argument: ${arg}`);
      process.exit(2);
    }
  }
  if (!Number.isFinite(opts.runs) || opts.runs < 1) opts.runs = 11;
  if (!Number.isFinite(opts.warmup) || opts.warmup < 0) opts.warmup = 0;
  if (!Number.isFinite(opts.coldRuns) || opts.coldRuns < 1) opts.coldRuns = 11;
  if (!Number.isFinite(opts.trials) || opts.trials < 1) opts.trials = 1;
  if (!Number.isFinite(opts.anchorRuns) || opts.anchorRuns < 1) opts.anchorRuns = 5;
  if (!Number.isFinite(opts.anchorCommits) || opts.anchorCommits < 1) opts.anchorCommits = 10000;
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
  if (xs.length === 0) return { median: null, p10: null, p90: null, p99: null, n: 0 };
  return {
    median: percentile(xs, 50),
    p10: percentile(xs, 10),
    p90: percentile(xs, 90),
    p99: percentile(xs, 99),
    n: xs.length,
  };
}

const ms = (x) => (x == null ? "   —  " : `${x.toFixed(1).padStart(6)}`);

// Median of a plain numeric array (non-null only), or null if empty. Used to
// collapse a per-trial statistic into a single robust value.
function medianOf(values) {
  const xs = values.filter((x) => x != null).sort((a, b) => a - b);
  if (xs.length === 0) return null;
  return percentile(xs, 50);
}

// Max of a plain numeric array (non-null only), or null if empty.
function maxOf(values) {
  const xs = values.filter((x) => x != null);
  if (xs.length === 0) return null;
  return Math.max(...xs);
}

// Collapse one stat object (the summarize() shape: {median,p10,p90,p99,n}) per
// trial into a single stat object of the same shape. Distribution stats take
// the MEDIAN across trials (median-of-medians — robust to one bad trial); the
// p99 tail takes the MAX across trials (worst observed tail — be pessimistic);
// n is kept as the per-trial sample count (it is identical across trials).
function aggregateStat(perTrialStats) {
  return {
    median: medianOf(perTrialStats.map((s) => s.median)),
    p10: medianOf(perTrialStats.map((s) => s.p10)),
    p90: medianOf(perTrialStats.map((s) => s.p90)),
    p99: maxOf(perTrialStats.map((s) => s.p99)),
    n: perTrialStats.length > 0 ? perTrialStats[0].n : 0,
  };
}

// The summarize-shaped fields on every op result that aggregateAcrossTrials
// collapses across trials.
const STAT_KEYS = ["wall", "startup", "fast_gate", "refresh", "body"];

// Given an array of per-trial result sets — each `{ warm: [...], cold: [...] }`
// where every op carries summarize-shaped stat objects — return a single result
// set of the SAME shape with each op's stat objects aggregated across trials
// via aggregateStat. Non-stat fields (name/args/budgetMs/available/failedStatus)
// are taken from the first trial; failedStatus surfaces the first failing trial.
function aggregateAcrossTrials(perTrialResults) {
  const aggregatePath = (path) => {
    const opCount = perTrialResults[0][path].length;
    const out = [];
    for (let i = 0; i < opCount; i++) {
      const perTrialOps = perTrialResults.map((set) => set[path][i]);
      const base = perTrialOps[0];
      const agg = {
        name: base.name,
        args: base.args,
        failedStatus: perTrialOps.map((o) => o.failedStatus).find((s) => s != null) ?? null,
        budgetMs: base.budgetMs,
      };
      if ("available" in base) agg.available = base.available;
      for (const key of STAT_KEYS) {
        agg[key] = aggregateStat(perTrialOps.map((o) => o[key]));
      }
      out.push(agg);
    }
    return out;
  };
  return { warm: aggregatePath("warm"), cold: aggregatePath("cold") };
}

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

// Cold path: force a fast-gate MISS before every spawn by bumping the mtime of
// one tracked markdown file. That changes the worktree-generation signal the
// gate keys on (so the command pays a full index refresh) without touching file
// content or git status. Every run is independently cold, so nothing is
// discarded as warm-up. With no touchable file the cold stats are unavailable.
function benchOpCold(bin, corpus, op, runs, touchFile) {
  if (!touchFile) {
    const unavailable = summarize([]);
    return {
      name: op.name,
      args: op.args,
      failedStatus: null,
      available: false,
      wall: unavailable,
      startup: unavailable,
      fast_gate: unavailable,
      refresh: unavailable,
      body: unavailable,
      budgetMs: op.budgetMs,
    };
  }
  const raw = [];
  for (let i = 0; i < runs; i++) {
    // Monotonically increasing timestamp so each run is a genuine mtime bump.
    const t = new Date();
    utimesSync(touchFile, t, t);
    raw.push(spawnOnce(bin, corpus, op.args));
  }
  const failed = raw.find((r) => !r.ok);
  const pick = (key) => summarize(raw.map((r) => r[key]));
  return {
    name: op.name,
    args: op.args,
    failedStatus: failed ? failed.status : null,
    available: true,
    wall: pick("wall"),
    startup: pick("startup"),
    fast_gate: pick("fast_gate"),
    refresh: pick("refresh"),
    body: pick("body"),
    budgetMs: op.budgetMs,
  };
}

// First stable tracked markdown file in the corpus, absolute path, or null.
function firstTrackedMarkdown(corpus) {
  const r = spawnSync("git", ["ls-files", "*.md"], { cwd: corpus, encoding: "utf8" });
  if (r.status !== 0 || !r.stdout) return null;
  const first = r.stdout.split("\n").find((line) => line.trim().length > 0);
  return first ? join(corpus, first.trim()) : null;
}

// ---- perceived latency ---------------------------------------------------

// How often each command is actually invoked; search dominates everyday use.
// The freq-weighted perceived latency is the single scoreboard number users
// feel — tail latency (p99) is what they remember.
const PERCEIVED_WEIGHTS = { search: 0.5, list: 0.2, summary: 0.2, check: 0.1 };

// Weighted sum of one wall stat across the ops actually present, normalized by
// the sum of those ops' weights (so a missing op doesn't deflate the number).
function weightedWall(results, key) {
  let num = 0;
  let den = 0;
  for (const r of results) {
    const w = PERCEIVED_WEIGHTS[r.name];
    const v = r.wall?.[key];
    if (w == null || v == null) continue;
    num += w * v;
    den += w;
  }
  return den === 0 ? null : num / den;
}

function perceivedLatency(warm, cold) {
  return {
    warm: { median: weightedWall(warm, "median"), p99: weightedWall(warm, "p99") },
    cold: { median: weightedWall(cold, "median"), p99: weightedWall(cold, "p99") },
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
  // These budgetMs values are informational regression tripwires on the fuseblk
  // scoreboard, not commit gates — they flag drift, they never fail the build.
  const summaryTitle = firstPageTitle(bin, corpus);
  const ops = [
    { name: "search", args: ["index", "--format", "json"], budgetMs: 200 },
    { name: "list", args: ["list", "--format", "json"], budgetMs: 200 },
  ];
  if (summaryTitle) {
    ops.push({ name: "summary", args: ["summary", summaryTitle], budgetMs: 200 });
  }
  // `check` validates the corpus directly and stays expensive even on a gate
  // hit, so it gets its own, looser budget and never shares a row. Re-baselined
  // to 400ms now that check medians ~110-130ms: ample headroom over the cold
  // median, tight enough that a real regression trips the `over` marker.
  ops.push({ name: "check", args: ["check", "--format", "json"], budgetMs: 400 });
  return ops;
}

// ---- output --------------------------------------------------------------

function printOpTable(title, results) {
  console.log(title);
  console.log(
    "op        wall(med/p90/p99)        startup  gate    refresh  body    budget   status",
  );
  console.log("─".repeat(92));
  for (const r of results) {
    const w = r.wall;
    if (r.available === false) {
      console.log(`${r.name.padEnd(9)} (unavailable — no tracked markdown file to bump)`);
      continue;
    }
    const over = w.median != null && w.median > r.budgetMs;
    const status = r.failedStatus != null && r.failedStatus > 1 ? "ERROR" : over ? "over" : "within";
    console.log(
      `${r.name.padEnd(9)} ` +
        `${ms(w.median)}/${ms(w.p90)}/${ms(w.p99)}  ` +
        `${ms(r.startup.median)}  ${ms(r.fast_gate.median)}  ${ms(r.refresh.median)}  ${ms(r.body.median)}  ` +
        `${String(r.budgetMs).padStart(5)}ms  ${status}`,
    );
  }
}

function printTable(warm, cold, perceived, env) {
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
  console.log(
    `runs:    warm ${env.runs} (first ${env.warmup} discarded as warm-up); ` +
      `cold ${env.coldRuns} (mtime-bumped, none discarded); gate on median` +
      (env.trials > 1 ? `; trials ${env.trials} (median-of-medians, worst-case p99)` : ""),
  );
  console.log("");
  printOpTable("warm path (steady-state, gate hits → no refresh)", warm);
  console.log("");
  printOpTable("cold path (gate miss → refresh)", cold);
  console.log("");
  const pw = perceived.warm;
  const pc = perceived.cold;
  console.log(
    `perceived (freq-weighted):  warm  med ${ms(pw.median)}ms  p99 ${ms(pw.p99)}ms` +
      `     cold  med ${ms(pc.median)}ms  p99 ${ms(pc.p99)}ms`,
  );
  console.log("");
  console.log("All times in ms. wall = full process wall-clock; startup/gate/refresh/body are");
  console.log("median per-term spans from wiki.log. The cold path bumps one tracked .md file's");
  console.log("mtime before each run to force a gate miss (content & git status unchanged).");
  console.log("Report-only: this command always exits 0.");
}

// ---- anchor-cache mode ---------------------------------------------------

// Synthetic deep-history corpus: `commits` commits touching a wiki page and
// a cited target. The target's cited range (L1-L3) is byte-identical across
// both target blobs — only a comment below the range alternates — so the
// page's `links-reviewed: 1` anchors at the oldest commit and the check
// stays Healthy while the anchor walk still blob-reads the page at every
// walked commit. That makes the per-commit read loop the measured cost,
// which is exactly the tier-A leg the cache memoizes.
const TARGET_V1 = "fn stable() {\n    work()\n}\n// A\n";
const TARGET_V2 = "fn stable() {\n    work()\n}\n// B\n";
const SYNTH_PAGE =
  "---\ntitle: Guide\nsummary: Synthetic anchor-cache benchmark page.\nlinks-reviewed: 1\n---\n\nSee [the target](docs/target.md#L1-L3).\n";

function buildSyntheticCorpus(commits) {
  const root = mkdtempSync(join(tmpdir(), "wiki-anchor-bench-"));
  const git = (args, opts = {}) => spawnSync("git", args, { cwd: root, encoding: "utf8", ...opts });
  git(["init", "-q", "-b", "main"]);
  mkdirSync(join(root, "wiki"));
  mkdirSync(join(root, "docs"));

  // fast-import stream: two target blobs and one page blob, then `commits`
  // commits alternating the target blob. fast-import turns 10k commits into
  // seconds where 10k git(1) invocations would take minutes.
  const stream = [];
  const blobs = [TARGET_V1, TARGET_V2, SYNTH_PAGE];
  for (const [i, body] of blobs.entries()) {
    stream.push(`blob\nmark :${i + 1}\ndata ${Buffer.byteLength(body)}\n${body}`);
  }
  for (let i = 0; i < commits; i++) {
    const target = i % 2 === 0 ? ":1" : ":2";
    const lines = i === 0 ? [`M 100644 :3 wiki/guide.md`, `M 100644 ${target} docs/target.md`] : [`M 100644 ${target} docs/target.md`];
    const msg = `c${i}`;
    stream.push(
      `commit refs/heads/main\ncommitter Bench <bench@example.invalid> 1700000000 +0000\ndata ${Buffer.byteLength(msg)}\n${msg}\n` +
        lines.join("\n") + "\n",
    );
  }
  const imp = git(["fast-import", "--quiet"], { input: stream.join("\n") + "\n" });
  if (imp.status !== 0) throw new Error(`fast-import failed: ${imp.stderr}`);
  const co = git(["checkout", "-q", "main"]);
  if (co.status !== 0) throw new Error(`checkout failed: ${co.stderr}`);
  return root;
}

function commitCount(corpus) {
  const r = spawnSync("git", ["rev-list", "--count", "HEAD"], { cwd: corpus, encoding: "utf8" });
  return r.status === 0 ? Number(r.stdout.trim()) : null;
}

// One measured `wiki check`: wall clock plus the cache tiers' contribution —
// the aggregated `anchor_cache` event (plan decision 7: one per run, never
// per link) carrying hit/miss/bypass counts and the summed
// fingerprint/walk git-leg durations.
function anchorCheckRun(bin, corpus) {
  writeFileSync(logPath(corpus), "");
  const start = process.hrtime.bigint();
  const r = spawnSync(bin, ["check"], {
    cwd: corpus,
    encoding: "utf8",
    env: { ...process.env, WIKI_PERF: "1" },
  });
  const wall = Number(process.hrtime.bigint() - start) / 1e6;
  const events = readRunEvents(corpus);
  const agg = events.find((e) => e.event === "anchor_cache");
  const meta = agg?.meta ?? {};
  return {
    ok: r.status === 0 || r.status === 1,
    status: r.status,
    wall,
    legMs: (meta.fingerprint_ms ?? 0) + (meta.walk_ms ?? 0),
    hits: meta.hits ?? 0,
    misses: meta.misses ?? 0,
    bypasses: meta.bypasses ?? 0,
  };
}

// Cold-vs-warm anchor-cache measurement for one corpus: `--clear-cache`
// wipes the cache, one cold check pays both tiers, then `runs` warm checks
// should pay neither — the served-hit path runs no cache.* span at all.
function anchorMeasure(bin, corpus, runs) {
  const clear = spawnSync(bin, ["check", "--clear-cache"], { cwd: corpus, encoding: "utf8" });
  if (clear.status !== 0) {
    console.error(`warning: --clear-cache exited ${clear.status} in ${corpus}`);
  }
  const cold = anchorCheckRun(bin, corpus);
  const warm = [];
  for (let i = 0; i < runs; i++) warm.push(anchorCheckRun(bin, corpus));
  return {
    corpus,
    commits: commitCount(corpus),
    cold,
    warm: {
      wall: summarize(warm.map((r) => r.wall)),
      legMs: summarize(warm.map((r) => r.legMs)),
      hits: medianOf(warm.map((r) => r.hits)),
      misses: medianOf(warm.map((r) => r.misses)),
      bypasses: medianOf(warm.map((r) => r.bypasses)),
      failedStatus: warm.map((r) => r.failedStatus ?? (r.ok ? null : r.status)).find((s) => s != null) ?? null,
    },
  };
}

function printAnchorReport(env, real, synthetic) {
  const row = (label, m) => {
    const cold = m.cold;
    const w = m.warm;
    console.log(`${label.padEnd(11)} commits ${String(m.commits ?? "?").padStart(6)}`);
    console.log(
      `  cold  wall ${ms(cold.wall)}ms   cache legs ${ms(cold.legMs)}ms   ` +
        `(hits ${cold.hits}, misses ${cold.misses}, bypasses ${cold.bypasses})` +
        (cold.ok ? "" : `  [run status ${cold.status}]`),
    );
    console.log(
      `  warm  wall ${ms(w.wall.median)}ms   cache legs ${ms(w.legMs.median)}ms   ` +
        `(hits ${w.hits ?? "—"}, misses ${w.misses ?? "—"}, bypasses ${w.bypasses ?? 0})` +
        (w.failedStatus != null ? `  [run status ${w.failedStatus}]` : ""),
    );
  };
  console.log("");
  console.log(`anchor cache — ${env.binVersion}`);
  console.log(`fs:      ${env.fsClass}`);
  console.log(`runs:    1 cold after --clear-cache; ${env.anchorRuns} warm (median)`);
  console.log("");
  row("real", real);
  row("synthetic", synthetic);
  console.log("");
  console.log("The cold cache-leg sum is the O(commits × pages) blob-read cost the cache");
  console.log("memoizes; a fully warm run serves every row and pays no cache.* span at all.");
  console.log("Report-only: this mode always exits 0.");
}

function runAnchorCacheMode(opts, env) {
  const real = anchorMeasure(opts.bin, opts.corpus, opts.anchorRuns);
  const synthRoot = buildSyntheticCorpus(opts.anchorCommits);
  const synthetic = anchorMeasure(opts.bin, synthRoot, opts.anchorRuns);
  if (opts.json) {
    console.log(JSON.stringify({ env, anchor: { real, synthetic } }, null, 2));
  } else {
    printAnchorReport(env, real, synthetic);
  }
  console.error(`synthetic corpus kept at ${synthRoot} (delete when done)`);
  process.exit(0);
}

// ---- main ----------------------------------------------------------------

// One full per-op measurement pass: build the op set, then run warm + cold for
// every op. Returns a `{ warm, cold }` result set. Called once per trial.
function measureOnce(opts) {
  const ops = buildOps(opts.bin, opts.corpus);
  const touchFile = firstTrackedMarkdown(opts.corpus);
  const warm = ops.map((op) => benchOp(opts.bin, opts.corpus, op, opts.runs, opts.warmup));
  const cold = ops.map((op) => benchOpCold(opts.bin, opts.corpus, op, opts.coldRuns, touchFile));
  return { warm, cold };
}

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
    coldRuns: opts.coldRuns,
    trials: opts.trials,
    anchorRuns: opts.anchorRuns,
  };

  if (opts.anchorCache) runAnchorCacheMode(opts, env);

  // Run the entire measurement opts.trials times. With trials === 1 we use the
  // single result set directly so behaviour and output are identical to before.
  const perTrial = [];
  for (let t = 0; t < opts.trials; t++) perTrial.push(measureOnce(opts));
  const { warm, cold } =
    opts.trials === 1 ? perTrial[0] : aggregateAcrossTrials(perTrial);
  const perceived = perceivedLatency(warm, cold);

  if (opts.json) {
    const payload = { env, warm, cold, perceived };
    // Preserve the raw per-trial sets so aggregation never loses data.
    if (opts.trials > 1) payload.trials = perTrial;
    console.log(JSON.stringify(payload, null, 2));
  } else {
    printTable(warm, cold, perceived, env);
  }

  // Report-only: always succeed.
  process.exit(0);
}

main();
