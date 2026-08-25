// src/opencode/index.ts
import { isAbsolute as isAbsolute2, resolve as resolvePath } from "node:path";

// src/common/wiki-check.ts
import { spawnSync } from "node:child_process";
import { closeSync, existsSync, openSync, readdirSync, readSync } from "node:fs";
import { homedir } from "node:os";
import { isAbsolute, join, resolve } from "node:path";
var FRONTMATTER_SCAN_BYTES = 4096;
var FRONTMATTER_SCAN_LINES = 30;
var DEFAULT_WIKI_CHECK_TIMEOUT_MS = 25e3;
function readFrontmatterPrefix(absPath) {
  const buf = Buffer.alloc(FRONTMATTER_SCAN_BYTES);
  const fd = openSync(absPath, "r");
  try {
    const bytesRead = readSync(fd, buf, 0, FRONTMATTER_SCAN_BYTES, 0);
    let head = buf.toString("utf-8", 0, bytesRead);
    if (bytesRead === FRONTMATTER_SCAN_BYTES) {
      const lastNewline = head.lastIndexOf("\n");
      if (lastNewline !== -1) head = head.slice(0, lastNewline);
    }
    return head;
  } finally {
    closeSync(fd);
  }
}
function isWikiFile(filePath, cwd) {
  if (!filePath.endsWith(".md")) return false;
  const absPath = isAbsolute(filePath) ? filePath : resolve(cwd, filePath);
  if (!existsSync(absPath)) return false;
  const head = readFrontmatterPrefix(absPath).split("\n").slice(0, FRONTMATTER_SCAN_LINES);
  if (head[0]?.trim() !== "---") return false;
  const closeIdx = head.slice(1).findIndex((l) => l.trim() === "---");
  if (closeIdx === -1) return false;
  const fmLines = head.slice(1, closeIdx + 1);
  let title = "";
  let summary = "";
  for (const line of fmLines) {
    const titleMatch = line.match(/^title\s*:\s*(.+)$/);
    if (titleMatch) title = titleMatch[1].trim().replace(/^['"]|['"]$/g, "");
    const summaryMatch = line.match(/^summary\s*:\s*(.+)$/);
    if (summaryMatch) summary = summaryMatch[1].trim().replace(/^['"]|['"]$/g, "");
  }
  return title.length > 0 && summary.length > 0;
}
var WIKI_EXECUTABLE = process.platform === "win32" ? "wiki.exe" : "wiki";
function compareSemver(a, b) {
  const pa = a.split(".").map((n) => Number.parseInt(n, 10));
  const pb = b.split(".").map((n) => Number.parseInt(n, 10));
  for (let i = 0; i < Math.max(pa.length, pb.length); i += 1) {
    const da = pa[i] ?? 0;
    const db = pb[i] ?? 0;
    if (Number.isNaN(da) || Number.isNaN(db)) return a.localeCompare(b);
    if (da !== db) return da - db;
  }
  return 0;
}
function vscodeGlobalStorageRoots() {
  const home = homedir();
  const roots = [
    join(home, ".vscode-server", "data", "User", "globalStorage"),
    join(home, ".vscode-server-insiders", "data", "User", "globalStorage"),
    join(home, ".config", "Code", "User", "globalStorage"),
    join(home, ".config", "Code - Insiders", "User", "globalStorage"),
    join(home, "Library", "Application Support", "Code", "User", "globalStorage"),
    join(home, "Library", "Application Support", "Code - Insiders", "User", "globalStorage")
  ];
  const appData = process.env.APPDATA;
  if (appData) {
    roots.push(join(appData, "Code", "User", "globalStorage"));
    roots.push(join(appData, "Code - Insiders", "User", "globalStorage"));
  }
  return roots;
}
function findManagedWikiBinary() {
  for (const root of vscodeGlobalStorageRoots()) {
    const binRoot = join(root, "goodfoot.wiki-extension", "bin");
    if (!existsSync(binRoot)) continue;
    let versions;
    try {
      versions = readdirSync(binRoot);
    } catch {
      continue;
    }
    versions.sort((a, b) => compareSemver(b, a));
    for (const version of versions) {
      const versionDir = join(binRoot, version);
      let targets;
      try {
        targets = readdirSync(versionDir);
      } catch {
        continue;
      }
      for (const target of targets) {
        const candidate = join(versionDir, target, WIKI_EXECUTABLE);
        if (existsSync(candidate)) return candidate;
      }
    }
  }
  return null;
}
function resolveWikiBinary(logger) {
  const override = process.env.WIKI_BIN;
  if (override && existsSync(override)) return override;
  if (override) {
    logger?.warn("WIKI_BIN override rejected \u2014 path does not exist", { wikiBin: override });
  }
  const whichCmd = process.platform === "win32" ? "where" : "which";
  const onPath = spawnSync(whichCmd, [WIKI_EXECUTABLE], { encoding: "utf8" });
  if (onPath.status === 0 && onPath.stdout) {
    const first = onPath.stdout.split(/\r?\n/).map((l) => l.trim()).filter(Boolean)[0];
    if (first && existsSync(first)) return first;
  }
  const managed = findManagedWikiBinary();
  if (managed) {
    logger?.info("resolved wiki binary from VS Code globalStorage", { path: managed });
    return managed;
  }
  return WIKI_EXECUTABLE;
}
function runWikiCheck(filePath, options) {
  let result;
  try {
    result = spawnSync(options.binary, ["check", "--fix", filePath], {
      cwd: options.cwd,
      encoding: "utf8",
      timeout: options.timeoutMs ?? DEFAULT_WIKI_CHECK_TIMEOUT_MS,
      env: { ...process.env }
    });
  } catch (err) {
    return { status: "unavailable", output: err instanceof Error ? err.message : String(err) };
  }
  if (isLaunchFailure(result)) {
    return { status: "unavailable", output: result.error?.message ?? "spawn failed" };
  }
  if (result.status !== 0) {
    const output = [result.stdout, result.stderr].filter(Boolean).join("\n").trim();
    return output ? { status: "residual", output } : { status: "residual" };
  }
  return { status: "clean" };
}
function isLaunchFailure(result) {
  return result.error != null;
}
function wikiContextBlock(output) {
  return `<wiki>
${output}
</wiki>`;
}
function wikiUnavailableBlock(filePath, wikiBin, detail) {
  const message = `wiki validation was SKIPPED \u2014 the \`wiki\` binary could not be launched (${detail}).
Resolved binary: ${wikiBin}
Fragment links and line-range drift for ${filePath} were NOT validated.
Install the wiki CLI on PATH, or set WIKI_BIN to its absolute path, then re-save the file.`;
  return wikiContextBlock(message);
}
function extractPatchedFilePaths(patchText) {
  const paths = [];
  for (const match of patchText.matchAll(/^\*\*\* (?:Add|Update|Delete) File: (.+)$/gm)) {
    const path = match[1].trim();
    if (path.length > 0 && !paths.includes(path)) paths.push(path);
  }
  return paths;
}

// src/opencode/index.ts
var WRITE_TOOL_IDS = /* @__PURE__ */ new Set(["edit", "write"]);
var PATCH_TOOL_ID = "apply_patch";
var WIKI_CHECK_TIMEOUT_MS = 25e3;
function narrowFilePath(args, directory) {
  if (args === null || typeof args !== "object" || !("filePath" in args)) return null;
  const raw = args.filePath;
  if (typeof raw !== "string" || raw.length === 0) return null;
  return isAbsolute2(raw) ? raw : resolvePath(directory, raw);
}
function narrowPatchTextArgs(args) {
  if (args === null || typeof args !== "object" || !("patchText" in args)) return null;
  const raw = args.patchText;
  if (typeof raw !== "string" || raw.length === 0) return null;
  return raw;
}
function candidatePaths(toolId, args, directory) {
  if (toolId === PATCH_TOOL_ID) {
    const patchText = narrowPatchTextArgs(args);
    return patchText === null ? [] : extractPatchedFilePaths(patchText);
  }
  const filePath = narrowFilePath(args, directory);
  return filePath === null ? [] : [filePath];
}
function assemblePlugin(deps = {}) {
  const directory = deps.directory ?? process.cwd();
  const resolveBinary = deps.resolveBinary ?? (() => resolveWikiBinary());
  const executeCheck = deps.executeCheck ?? ((filePath, options) => runWikiCheck(filePath, { binary: options.binary, timeoutMs: WIKI_CHECK_TIMEOUT_MS, cwd: directory }));
  const afterHandler = async (input, output) => {
    try {
      if (input === null || typeof input !== "object") return;
      const toolId = typeof input.tool === "string" ? input.tool : "";
      if (toolId !== PATCH_TOOL_ID && !WRITE_TOOL_IDS.has(toolId)) return;
      const paths = candidatePaths(toolId, input.args, directory).map((p) => isAbsolute2(p) ? p : resolvePath(directory, p));
      const wikiPaths = paths.filter((p) => isWikiFile(p, directory));
      if (wikiPaths.length === 0) return;
      const wikiBin = resolveBinary();
      const sections = [];
      let unavailableDetail = null;
      for (const filePath of wikiPaths) {
        const result = executeCheck(filePath, { binary: wikiBin });
        if (result.status === "unavailable") {
          unavailableDetail ??= result.output ?? "spawn failed";
          continue;
        }
        if (result.status === "residual" && result.output) sections.push(result.output);
      }
      if (sections.length === 0 && unavailableDetail === null) return;
      if (output === null || typeof output !== "object") return;
      const payload = unavailableDetail !== null ? wikiUnavailableBlock(wikiPaths[0], wikiBin, unavailableDetail) : wikiContextBlock(sections.join("\n\n"));
      const prior = typeof output.output === "string" ? output.output : "";
      output.output = `${prior}
${payload}`;
    } catch {
    }
  };
  return {
    "tool.execute.after": async (input, output) => {
      await afterHandler(input, output);
    },
    dispose: () => {
    }
  };
}
async function wikiOpencode(input) {
  return assemblePlugin({
    directory: typeof input?.directory === "string" && input.directory.length > 0 ? input.directory : void 0
  });
}
export {
  assemblePlugin,
  wikiOpencode as default
};
