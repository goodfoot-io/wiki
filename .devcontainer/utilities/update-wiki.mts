#!/usr/bin/env -S node --experimental-transform-types

// Walks the output of `wiki scaffold --format json`, generates a "why" sentence
// for every mesh by spawning `claude -p` in stream-json mode, and applies the
// result with `git mesh add` + `git mesh why`. Mirrors the behaviour of
// scripts/update-wiki-mesh-coverage.sh, but renders the agent loop inline with
// ANSI colors and supports --dry-run.

import { execFileSync, spawn } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve as resolvePath } from "node:path";
import { createInterface } from "node:readline";

const ANSI = {
  reset: "\x1b[0m",
  bold: "\x1b[1m",
  gray: "\x1b[90m",
  white: "\x1b[37m",
  yellow: "\x1b[33m",
  cyan: "\x1b[36m",
  pink: "\x1b[95m",
  red: "\x1b[31m",
};

interface WikiScaffold { schemaVersion: number; parseErrors: unknown[]; pages: Page[]; }
interface Page { path: string; title: string; meshes: Mesh[]; }
interface Mesh { slug: string; headingChain: string[]; anchors: Anchor[]; }
interface Anchor { path: string; startLine: number; endLine: number; }

interface CliArgs { dryRun: boolean; limit: number | null; agents: number; globs: string[]; }

function parseArgs(argv: string[]): CliArgs {
  let dryRun = false;
  let limit: number | null = null;
  let agents = 1;
  const globs: string[] = [];
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i]!;
    if (arg === "--dry-run") dryRun = true;
    else if (arg === "-h" || arg === "--help") { printHelp(); process.exit(0); }
    else if (arg === "--limit") {
      const next = argv[++i];
      if (next === undefined) { console.error("--limit requires a value"); process.exit(2); }
      limit = parsePositiveInt("--limit", next);
    } else if (arg.startsWith("--limit=")) {
      limit = parsePositiveInt("--limit", arg.slice("--limit=".length));
    } else if (arg === "--agents") {
      const next = argv[++i];
      if (next === undefined) { console.error("--agents requires a value"); process.exit(2); }
      agents = parsePositiveInt("--agents", next);
    } else if (arg.startsWith("--agents=")) {
      agents = parsePositiveInt("--agents", arg.slice("--agents=".length));
    } else if (arg.startsWith("-")) { console.error(`unknown flag: ${arg}`); printHelp(); process.exit(2); }
    else globs.push(arg);
  }
  return { dryRun, limit, agents, globs };
}

function parsePositiveInt(flag: string, raw: string): number {
  const n = Number(raw);
  if (!Number.isInteger(n) || n <= 0) {
    console.error(`${flag} must be a positive integer (got: ${raw})`);
    process.exit(2);
  }
  return n;
}

function printHelp(): void {
  process.stderr.write(`usage: update-wiki [--dry-run] [--limit N] [--agents N] [<wiki-glob>...]

Runs \`wiki scaffold --format json\` over the given globs, generates a "why"
sentence for each mesh via a haiku \`claude\` invocation, and applies the
result with \`git mesh add\` + \`git mesh why\`.

  --dry-run    Generate the why sentences but only print the mutating
               git mesh commands; do not execute them.
  --limit N    Process only the first N meshes from the scaffold output.
  --agents N   Run N \`claude\` sessions concurrently (default: 1). The
               \`git mesh\` apply step is serialized, and each mesh's
               transcript is buffered and printed as a contiguous block.
`);
}

function renderReadTranscript(ref: string): string {
  const hashIdx = ref.indexOf("#");
  const path = hashIdx === -1 ? ref : ref.slice(0, hashIdx);
  const range = hashIdx === -1 ? "" : ref.slice(hashIdx + 1);

  let offset: number | null = null;
  let endLine: number | null = null;
  if (range !== "") {
    const rangeMatch = range.match(/^L(\d+)-L(\d+)$/);
    const lineMatch = range.match(/^L(\d+)$/);
    if (rangeMatch) {
      offset = Number(rangeMatch[1]);
      endLine = Number(rangeMatch[2]);
    } else if (lineMatch) {
      offset = Number(lineMatch[1]);
      endLine = offset;
    } else {
      throw new Error(`unrecognized range '${range}' in '${ref}'`);
    }
  }

  const absolutePath = resolvePath(process.cwd(), path);
  const allLines = readFileSync(absolutePath, "utf8").split("\n");
  const startIdx = offset === null ? 0 : offset - 1;
  const endIdx = endLine === null ? allLines.length : endLine;
  const slice = allLines.slice(startIdx, endIdx);
  const numbered = slice
    .map((line, i) => `${String(startIdx + 1 + i).padStart(6)}\t${line}`)
    .join("\n");

  const params: string[] = [`<parameter name="file_path">${absolutePath}</parameter>`];
  if (offset !== null && endLine !== null) {
    params.push(`<parameter name="offset">${offset}</parameter>`);
    params.push(`<parameter name="limit">${endLine - offset + 1}</parameter>`);
  }

  return `<function_calls>
<invoke name="Read">
${params.join("\n")}
</invoke>
</function_calls>

<function_results>
${numbered}
</function_results>`;
}

type ParsedRef = { path: string; range: { start: number; end: number } | null };

function parseRef(ref: string): ParsedRef {
  const hashIdx = ref.indexOf("#");
  const path = hashIdx === -1 ? ref : ref.slice(0, hashIdx);
  const rangeStr = hashIdx === -1 ? "" : ref.slice(hashIdx + 1);
  if (rangeStr === "") return { path, range: null };
  const rangeMatch = rangeStr.match(/^L(\d+)-L(\d+)$/);
  if (rangeMatch) return { path, range: { start: Number(rangeMatch[1]), end: Number(rangeMatch[2]) } };
  const lineMatch = rangeStr.match(/^L(\d+)$/);
  if (lineMatch) {
    const n = Number(lineMatch[1]);
    return { path, range: { start: n, end: n } };
  }
  throw new Error(`unrecognized range '${rangeStr}' in '${ref}'`);
}

function formatRef({ path, range }: ParsedRef): string {
  return range === null ? path : `${path}#L${range.start}-L${range.end}`;
}

function mergeAnchorRefs(refs: string[]): string[] {
  const order: string[] = [];
  const byPath = new Map<string, ParsedRef[]>();
  for (const ref of refs) {
    const parsed = parseRef(ref);
    if (!byPath.has(parsed.path)) {
      order.push(parsed.path);
      byPath.set(parsed.path, []);
    }
    byPath.get(parsed.path)!.push(parsed);
  }

  const result: string[] = [];
  for (const path of order) {
    const items = byPath.get(path)!;
    if (items.some((it) => it.range === null)) {
      result.push(path);
      continue;
    }
    const ranges = items.map((it) => it.range!).sort((a, b) => a.start - b.start);
    const merged: { start: number; end: number }[] = [];
    for (const r of ranges) {
      const last = merged[merged.length - 1];
      if (last && r.start <= last.end) {
        last.end = Math.max(last.end, r.end);
      } else {
        merged.push({ ...r });
      }
    }
    for (const r of merged) {
      result.push(formatRef({ path, range: r }));
    }
  }
  return result;
}

function buildSystemPrompt(): string {
  return `User message: a wiki excerpt followed by the contents of its anchored line ranges. Read tool available if you need more.

Produce one sentence describing the passage and its anchors.

<artifact> of <subject> that <static verb> <what flows> to <named consumer>. The <what flows> slot names a falsifiable noun the subject produces or guarantees (a bundle, a type, a surface, a vocabulary) — never omit it. For static-acceptance verbs (consumes, embeds), the subject itself is what flows, and the slot collapses: <artifact> of <subject> that <named consumer> <static verb>. Subject = the most specific export the prose centers on. Artifact ∈ {reference, map, contract, surface, vocabulary, fixture, explanation, demonstration} — never the subject itself. Static verb ∈ {returns, exposes, satisfies, consumes, embeds, describes, answers} — contractual, not procedural (no builds, computes, bundles, reads, processes). Named consumer = an entity (worker, signer, package, handler), not an activity (signer ✓, signing ✗). Predicate must break when the subject is swapped for any sibling the wiki contrasts. Tenseless everywhere — no nominalized verbs in any slot or "for X" phrase.

Final message = the sentence. No preamble, no quotes, no commentary.`;
}

function buildPrompt(anchorRefs: string[]): string {
  return anchorRefs.map(renderReadTranscript).join("\n\n");
}

type ReadInput = { file_path: string; offset?: number; limit?: number };

function renderRead(input: ReadInput, content: unknown, log: (s: string) => void): void {
  const text =
    typeof content === "string"
      ? content
      : Array.isArray(content)
        ? content.map((c: any) => (typeof c === "string" ? c : c?.text ?? JSON.stringify(c))).join("")
        : JSON.stringify(content);

  const parts: string[] = [];
  if (input.offset !== undefined) parts.push(`starting at line ${input.offset}`);
  if (input.limit !== undefined) parts.push(`for ${input.limit} lines`);
  const suffix = parts.length > 0 ? ` ${parts.join(" ")}` : "";

  log(`${ANSI.yellow}Read ${input.file_path}${suffix}${ANSI.reset}`);
  log(`${ANSI.cyan}${text}${ANSI.reset}`);
}

function generateWhy(anchorRefs: string[], log: (s: string) => void): Promise<string> {
  const prompt = buildPrompt(anchorRefs);
  const systemPrompt = buildSystemPrompt();

  log(`${ANSI.gray}${systemPrompt}${ANSI.reset}`);

  return new Promise((resolvePromise, rejectPromise) => {
    const proc = spawn(
      "claude",
      [
        "-p",
        "--input-format", "stream-json",
        "--output-format", "stream-json",
        "--replay-user-messages",
        "--verbose",
        "--effort", "medium",
        "--model", "opus",
        "--setting-sources", "",
        "--no-session-persistence",
        "--allowedTools", "Read",
        "--system-prompt", systemPrompt,
      ],
      {
        env: {
          ...process.env,
          CLAUDE_CODE_DISABLE_CLAUDE_MDS: "1",
          ENABLE_CLAUDEAI_MCP_SERVERS: "false",
          CLAUDE_CODE_DISABLE_POLICY_SKILLS: "1",
          CLAUDE_CODE_DISABLE_AUTO_MEMORY: "1",
        },
        stdio: ["pipe", "pipe", "pipe"],
      },
    );

    proc.stdin.write(
      JSON.stringify({
        type: "user",
        message: { role: "user", content: [{ type: "text", text: prompt }] },
      }) + "\n",
    );
    proc.stdin.end();

    let finalAnswer = "";
    const pendingReads = new Map<string, ReadInput>();

    const rl = createInterface({ input: proc.stdout });
    rl.on("line", (line) => {
      if (!line.trim()) return;
      let event: any;
      try { event = JSON.parse(line); } catch { return; }

      if (event.type === "user") {
        for (const block of event.message?.content ?? []) {
          if (event.isReplay && block.type === "text") {
            log(`${ANSI.bold}${block.text}${ANSI.reset}`);
          } else if (block.type === "tool_result") {
            const pending = pendingReads.get(block.tool_use_id);
            if (pending) {
              renderRead(pending, block.content, log);
              pendingReads.delete(block.tool_use_id);
            }
          }
        }
        return;
      }

      if (event.type === "assistant") {
        for (const block of event.message?.content ?? []) {
          if (block.type === "thinking") {
            log(`${ANSI.gray}${block.thinking}${ANSI.reset}`);
          } else if (block.type === "text") {
            log(`${ANSI.white}${block.text}${ANSI.reset}`);
          } else if (block.type === "tool_use" && block.name === "Read") {
            pendingReads.set(block.id, block.input as ReadInput);
          }
        }
        return;
      }

      if (event.type === "result" && event.subtype === "success" && typeof event.result === "string") {
        finalAnswer = event.result;
      }
    });

    proc.stderr.on("data", () => { /* discard verbose claude logs */ });
    proc.on("error", rejectPromise);
    proc.on("close", (code) => {
      if (code !== 0) {
        rejectPromise(new Error(`claude exited with code ${code}`));
        return;
      }
      const lines = finalAnswer.split(/\r?\n/).map((l) => l.trim()).filter((l) => l.length > 0);
      const lastLine = lines[lines.length - 1] ?? "";
      const normalized = lastLine.replace(/\s+/g, " ").trim();
      resolvePromise(normalized);
    });
  });
}

function shellQuote(parts: string[]): string {
  return parts
    .map((p) => (/^[A-Za-z0-9_./:#@%+=-]+$/.test(p) ? p : `'${p.replace(/'/g, `'\\''`)}'`))
    .join(" ");
}

function formatGitMesh(args: string[]): string {
  return `${ANSI.pink}git ${shellQuote(args)}${ANSI.reset}`;
}

async function main(): Promise<void> {
  const { dryRun, limit, agents, globs } = parseArgs(process.argv.slice(2));

  const scaffoldArgs = ["scaffold", "--format", "json", ...globs];
  const scaffoldRaw = execFileSync("wiki", scaffoldArgs, { encoding: "utf8" });
  const scaffold: WikiScaffold = JSON.parse(scaffoldRaw);

  const items: { mesh: Mesh; anchorRefs: string[] }[] = [];
  outer: for (const page of scaffold.pages) {
    for (const mesh of page.meshes) {
      if (limit !== null && items.length >= limit) break outer;
      const anchorRefs = mergeAnchorRefs(
        mesh.anchors.map((a) => `${a.path}#L${a.startLine}-L${a.endLine}`),
      );
      items.push({ mesh, anchorRefs });
    }
  }

  let nextIdx = 0;
  let applyChain: Promise<void> = Promise.resolve();

  async function worker(): Promise<void> {
    while (true) {
      const idx = nextIdx++;
      if (idx >= items.length) return;
      const { mesh, anchorRefs } = items[idx]!;

      const buffered: string[] = [];
      const log = (s: string) => { buffered.push(s); };

      let why = "";
      let error: unknown = null;
      try {
        why = await generateWhy(anchorRefs, log);
      } catch (err) {
        error = err;
      }

      // Serialize the apply step so transcripts print contiguously and
      // git mesh mutations do not race against each other.
      const previous = applyChain;
      let release!: () => void;
      applyChain = new Promise<void>((r) => { release = r; });
      await previous;
      try {
        for (const line of buffered) console.log(line);
        if (error) {
          const msg = error instanceof Error ? error.message : String(error);
          console.error(`${ANSI.red}skipping ${mesh.slug}: ${msg}${ANSI.reset}`);
          continue;
        }
        if (!why) {
          console.error(`${ANSI.red}skipping ${mesh.slug}: empty why${ANSI.reset}`);
          continue;
        }

        const addArgs = ["mesh", "add", mesh.slug, ...anchorRefs];
        const whyArgs = ["mesh", "why", mesh.slug, "-m", why];

        console.log(formatGitMesh(addArgs));
        console.log(formatGitMesh(whyArgs));

        if (!dryRun) {
          execFileSync("git", addArgs, { stdio: "inherit" });
          execFileSync("git", whyArgs, { stdio: "inherit" });
        }
      } finally {
        release();
      }
    }
  }

  const poolSize = Math.min(agents, items.length);
  const pool = Array.from({ length: poolSize }, () => worker());
  await Promise.all(pool);

  const suffix = dryRun ? " (dry run)" : "";
  console.log(`${ANSI.pink}processed ${items.length} mesh(es)${suffix}${ANSI.reset}`);
}

main().catch((err) => {
  console.error(`${ANSI.red}${err instanceof Error ? err.message : String(err)}${ANSI.reset}`);
  process.exit(1);
});
