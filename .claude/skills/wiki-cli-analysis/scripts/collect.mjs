import { readFileSync, writeFileSync, globSync, statSync } from "node:fs";
import { argv, stderr } from "node:process";
import { homedir } from "node:os";
import { dirname, resolve, join } from "node:path";
import { fileURLToPath } from "node:url";
import { extractInvocations } from "./lib/detect.mjs";
import { walkTranscript, lookupOutcome } from "./lib/transcripts.mjs";

const flags = {};
for (const a of argv.slice(2)) {
  if (!a.startsWith("--")) continue;
  const [k, v] = a.replace(/^--/, "").split("=");
  flags[k] = v ?? true;
}

const here = dirname(fileURLToPath(import.meta.url));
const CLAUDE_ROOT = flags["claude-root"] && flags["claude-root"] !== true ? resolve(String(flags["claude-root"])) : resolve(homedir(), ".claude");
const CARDS_ROOT = flags["cards-root"] && flags["cards-root"] !== true ? resolve(String(flags["cards-root"])) : resolve(homedir(), ".cards");
const OUT_PATH = flags.out && flags.out !== true ? resolve(String(flags.out)) : resolve(here, "invocations.jsonl");
const INDEX_OUT = flags["index-out"] && flags["index-out"] !== true ? resolve(String(flags["index-out"])) : resolve(here, "index.json");

const invocations = [];
const fileStats = [];

function head(s, n = 400) {
  if (typeof s !== "string") return null;
  return s.length > n ? s.slice(0, n) + "..." : s;
}

function push(source, meta, fields) {
  for (const hit of extractInvocations(fields.text)) {
    invocations.push({
      source,
      file: meta.file,
      cwd: fields.cwd ?? null,
      timestamp: fields.timestamp ?? null,
      ordinal: fields.ordinal ?? null,
      toolUseId: fields.toolUseId ?? null,
      isSidechain: fields.isSidechain ?? false,
      jsonPath: fields.jsonPath ?? null,
      mentionKind: fields.mentionKind ?? null,
      command: head(fields.text, 2000),
      segment: hit.segment.slice(0, 500),
      bin: hit.bin,
      sub: hit.sub,
      flags: hit.flags,
      positionalCount: hit.positional.length,
      query: hit.query,
      outcome: fields.outcome ?? { state: "unknown" },
    });
    meta.invocations++;
  }
}

const claudeFiles = globSync("**/*.jsonl", { cwd: CLAUDE_ROOT }).sort();
if (claudeFiles.length === 0) stderr.write(`warning: no *.jsonl found under ${CLAUDE_ROOT}\n`);

for (const rel of claudeFiles) {
  const abs = join(CLAUDE_ROOT, rel);
  const meta = { file: `claude:${rel}`, kind: "transcript", invocations: 0 };
  let malformedLines = 0;
  let tail;
  for (const ev of walkTranscript(abs)) {
    if (ev.error) {
      stderr.write(`warning: ${ev.error}\n`);
      meta.error = ev.error;
      continue;
    }
    if (ev.malformed !== undefined) {
      malformedLines = ev.malformed;
      tail = ev;
      continue;
    }
    push("claude-transcript", meta, ev);
  }
  for (const inv of invocations) {
    if (inv.file !== meta.file || !inv.toolUseId || inv.outcome?.state !== "unknown") continue;
    const o = lookupOutcome(tail?.outcomesById, inv.toolUseId);
    if (o) inv.outcome = o;
  }
  meta.malformedLines = malformedLines;
  fileStats.push(meta);
}

const cardFiles = globSync("**/*.json", { cwd: CARDS_ROOT }).sort();
let jsonParseErrors = 0;

for (const rel of cardFiles) {
  const abs = join(CARDS_ROOT, rel);
  const meta = { file: `cards:${rel}`, kind: "card-json", invocations: 0 };
  let data;
  try {
    data = JSON.parse(readFileSync(abs, "utf8"));
  } catch {
    jsonParseErrors++;
    stderr.write(`warning: unparseable JSON ${abs}\n`);
    continue;
  }

  const PROSE_FIELDS = /(^|\.)(title|summary|description)$/;
  const walk = (node, path) => {
    if (typeof node === "string") {
      const t = node.trim();
      if (
        t.includes("wiki") &&
        /\s/.test(t) &&
        !/^https?:\/\//.test(t) &&
        !/^[\w./@{}$-]+$/.test(t)
      ) {
        push("cards-json", meta, {
          text: node,
          jsonPath: path,
          mentionKind: PROSE_FIELDS.test(path) ? "prose-mention" : "embedded-command",
        });
      }
      return;
    }
    if (Array.isArray(node)) {
      node.forEach((v, i) => walk(v, `${path}[${i}]`));
    } else if (node && typeof node === "object") {
      for (const [k, v] of Object.entries(node)) walk(v, path ? `${path}.${k}` : k);
    }
  };
  walk(data, "");
  fileStats.push(meta);
}

writeFileSync(OUT_PATH, invocations.map((i) => JSON.stringify(i)).join("\n") + (invocations.length ? "\n" : ""));
writeFileSync(
  INDEX_OUT,
  JSON.stringify(
    {
      generatedAt: new Date().toISOString(),
      claudeRoot: CLAUDE_ROOT,
      cardsRoot: CARDS_ROOT,
      totals: { filesScanned: fileStats.length, invocations: invocations.length, jsonParseErrors },
      files: fileStats.filter((f) => f.invocations > 0 || f.kind === "transcript" || f.malformedLines > 0),
    },
    null,
    2,
  ),
);

stderr.write(`${fileStats.length} files scanned (${claudeFiles.length} transcripts, ${cardFiles.length} cards JSON), ${jsonParseErrors} unparseable, ${invocations.length} wiki CLI invocations written\n`);
stderr.write(`invocations: ${OUT_PATH}\nindex: ${INDEX_OUT}\n`);
