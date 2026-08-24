#!/usr/bin/env node
import { readFileSync, existsSync } from "node:fs";
import { argv, stdout, stderr } from "node:process";
import { dirname, resolve, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const flags = {};
for (const a of argv.slice(2)) {
  if (!a.startsWith("--")) continue;
  const [k, v] = a.replace(/^--/, "").split("=");
  flags[k] = v ?? true;
}
const IN_PATH = flags.in && flags.in !== true ? resolve(String(flags.in)) : resolve(here, "invocations.jsonl");
const TOP = Number(flags.top && flags.top !== true ? flags.top : 15);

if (!existsSync(IN_PATH)) {
  stderr.write(`error: ${IN_PATH} not found — run collect.mjs first\n`);
  process.exit(1);
}

const invocations = readFileSync(IN_PATH, "utf8")
  .split("\n")
  .filter((l) => l.trim())
  .map((l) => JSON.parse(l));

function table(pairs, n = TOP) {
  const rows = [...pairs].sort((a, b) => b[1] - a[1]).slice(0, n);
  const w = Math.max(...rows.map(([k]) => k.length), 8);
  for (const [k, v] of rows) stdout.write(`${String(k).padEnd(w)}  ${v}\n`);
}

function styleOf(inv) {
  if (inv.bin === "wiki") return "bare (PATH)";
  if (/\/target\/debug\//.test(inv.command)) return "target/debug build";
  if (/\/target\/release\//.test(inv.command)) return "target/release build";
  return `other path (${inv.bin})`;
}

const bySource = new Map();
const byMentionKind = new Map();
const bySub = new Map();
const byStyle = new Map();
const byFlag = new Map();
const byOutcome = new Map();
const bySession = new Set();
const exactCmds = new Map();
const queries = [];
const errors = [];
const timestamps = [];

for (const inv of invocations) {
  bySource.set(inv.source, (bySource.get(inv.source) ?? 0) + 1);
  if (inv.source === "cards-json") {
    const k = inv.mentionKind ?? "unclassified";
    byMentionKind.set(k, (byMentionKind.get(k) ?? 0) + 1);
  }
  bySub.set(inv.sub ?? "(none)", (bySub.get(inv.sub ?? "(none)") ?? 0) + 1);
  const style = styleOf(inv);
  byStyle.set(style, (byStyle.get(style) ?? 0) + 1);
  byOutcome.set(inv.outcome?.state ?? "unknown", (byOutcome.get(inv.outcome?.state ?? "unknown") ?? 0) + 1);
  for (const f of inv.flags ?? []) byFlag.set(f, (byFlag.get(f) ?? 0) + 1);
  bySession.add(inv.file);
  if (inv.timestamp) timestamps.push(inv.timestamp);
  const norm = `${inv.bin} ${(inv.sub ?? "")}`.trim() + (inv.flags ?? []).join(",");
  exactCmds.set(norm, (exactCmds.get(norm) ?? 0) + 1);
  if (inv.query) queries.push({ query: inv.query, file: inv.file, timestamp: inv.timestamp });
  if ((inv.outcome?.state ?? "unknown") === "error" && errors.length < TOP * 2) {
    errors.push({ segment: inv.segment, exitCode: inv.outcome.exitCode, stdoutHead: inv.outcome.stderrHead ?? inv.outcome.stdoutHead });
  }
}

timestamps.sort();

if (flags.json) {
  stdout.write(
    JSON.stringify(
      {
        total: invocations.length,
        distinctFiles: bySession.size,
        timeRange: { first: timestamps[0] ?? null, last: timestamps[timestamps.length - 1] ?? null },
        bySource: Object.fromEntries(bySource),
        bySubcommand: Object.fromEntries([...bySub].sort((a, b) => b[1] - a[1])),
        byStyle: Object.fromEntries(byStyle),
        byOutcome: Object.fromEntries(byOutcome),
        byFlag: Object.fromEntries([...byFlag].sort((a, b) => b[1] - a[1])),
        queries: [...new Set(queries.map((q) => q.query))],
      },
      null,
      2,
    ) + "\n",
  );
  process.exit(0);
}

stdout.write(`=== wiki CLI usage review ===\n`);
stdout.write(`invocations: ${invocations.length} across ${bySession.size} files\n`);
if (timestamps.length) stdout.write(`time range:  ${timestamps[0]} .. ${timestamps[timestamps.length - 1]}\n`);

stdout.write(`\n-- by source --\n`);
table(bySource, 10);
if (byMentionKind.size) {
  stdout.write(`   cards-json split: ${[...byMentionKind].map(([k, v]) => `${k}=${v}`).join(", ")}\n`);
}

stdout.write(`\n-- invocation style --\n`);
table(byStyle, 10);

stdout.write(`\n-- subcommands --\n`);
table(bySub);

stdout.write(`\n-- flags --\n`);
table(byFlag);

stdout.write(`\n-- outcomes --\n`);
table(byOutcome, 5);

stdout.write(`\n-- most repeated (bin+sub+flags) --\n`);
table(exactCmds);

if (queries.length) {
  stdout.write(`\n-- search-style queries (no subcommand) --\n`);
  for (const q of [...new Set(queries.map((q) => q.query))].slice(0, TOP)) stdout.write(`"${q}"\n`);
}

if (errors.length) {
  stdout.write(`\n-- error samples (last output line) --\n`);
  for (const e of errors.slice(0, TOP)) {
    stdout.write(`[${e.exitCode ?? "?"}] ${e.segment.slice(0, 140)}\n`);
    const lines = (e.stdoutHead ?? "").split("\n").map((l) => l.trim()).filter(Boolean);
    if (lines.length > 1) stdout.write(`    -> ${lines[lines.length - 1].slice(0, 160)}\n`);
  }
}

const idxPath = resolve(here, "index.json");
if (!flags.json && existsSync(idxPath)) {
  try {
    const idx = JSON.parse(readFileSync(idxPath, "utf8"));
    stdout.write(`\ncorpus: ${idx.totals.filesScanned} files scanned, ${idx.totals.invocations} invocations, ${idx.totals.jsonParseErrors} unparseable JSON\n`);
    const withHits = idx.files.filter((f) => f.invocations > 0).sort((a, b) => b.invocations - a.invocations);
    if (withHits.length) {
      stdout.write(`top files by invocations:\n`);
      for (const f of withHits.slice(0, 8)) stdout.write(`  ${f.invocations}\t${f.file}\n`);
    }
  } catch {}
}
