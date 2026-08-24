import { readFileSync } from "node:fs";

function head(s, n = 600) {
  if (typeof s !== "string") return null;
  return s.length > n ? s.slice(0, n) + "..." : s;
}

export function resultFromToolResult(item) {
  const c = item.content;
  const text =
    typeof c === "string"
      ? c
      : Array.isArray(c)
        ? c.map((b) => (typeof b === "string" ? b : b?.text ?? "")).join("\n")
        : "";
  return { state: item.is_error ? "error" : "ok", exitCode: null, stdoutHead: head(text), stderrHead: null };
}

export function mergeOutcome(existing, tur) {
  if (!tur || typeof tur !== "object") return existing;
  const next = { ...existing };
  if (typeof tur.exitCode === "number") next.exitCode = tur.exitCode;
  if (typeof tur.stdout === "string") next.stdoutHead = head(tur.stdout);
  if (typeof tur.stderr === "string") next.stderrHead = head(tur.stderr);
  if (typeof next.exitCode === "number") next.state = next.exitCode === 0 ? "ok" : "error";
  else if (typeof tur.stdout === "string") next.state = "ok";
  return next;
}

export function* walkTranscript(absPath) {
  let lines;
  try {
    lines = readFileSync(absPath, "utf8").split("\n");
  } catch (e) {
    yield { error: `could not read ${absPath}: ${e.message}` };
    return;
  }
  const outcomesById = new Map();
  let lastCwd = null;
  let lastTs = null;
  let malformed = 0;

  for (let ordinal = 0; ordinal < lines.length; ordinal++) {
    const line = lines[ordinal];
    if (!line.trim()) continue;
    let rec;
    try {
      rec = JSON.parse(line);
    } catch {
      malformed++;
      continue;
    }
    if (typeof rec.cwd === "string") lastCwd = rec.cwd;
    if (typeof rec.timestamp === "string") lastTs = rec.timestamp;

    const content = rec.message?.content;
    if (Array.isArray(content)) {
      for (const item of content) {
        if (!item || typeof item !== "object") continue;
        if (item.type === "tool_use" && typeof item.input?.command === "string") {
          yield {
            ordinal,
            text: item.input.command,
            toolUseId: item.id ?? null,
            cwd: lastCwd,
            timestamp: lastTs,
            isSidechain: Boolean(rec.isSidechain),
            outcomeRef: item.id ?? null,
          };
        } else if (item.type === "tool_result") {
          const prev = outcomesById.get(item.tool_use_id);
          outcomesById.set(item.tool_use_id, mergeOutcome(prev ?? resultFromToolResult(item), rec.toolUseResult));
        }
      }
    } else if (typeof content === "string" && content.includes("<bash-input>")) {
      for (const m of content.matchAll(/<bash-input>([\s\S]*?)<\/bash-input>/g)) {
        yield { ordinal, text: m[1], toolUseId: null, cwd: lastCwd, timestamp: lastTs, isSidechain: false, outcomeRef: null };
      }
    }

    if (rec.type === "user" && rec.toolUseResult && typeof rec.toolUseResult === "object" && rec.sourceToolAssistantUUID) {
      const id = rec.sourceToolAssistantUUID;
      outcomesById.set(id, mergeOutcome(outcomesById.get(id) ?? { state: "unknown" }, rec.toolUseResult));
    }
  }

  yield { malformed, outcomesById };
}

export function lookupOutcome(outcomesById, id) {
  if (!id) return { state: "unknown" };
  return outcomesById?.get(id) ?? { state: "unknown" };
}
