// src/opencode/index.ts
import { isAbsolute as isAbsolute2, resolve as resolvePath } from "node:path";

// src/common/wiki-check.ts
import { spawnSync } from "node:child_process";
import { closeSync, existsSync, openSync, readdirSync, readFileSync, readSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
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

// src/opencode/index.ts
var EDIT_TOOL_IDS = /* @__PURE__ */ new Set(["edit", "write"]);
var WIKI_CHECK_TIMEOUT_MS = 25e3;
function narrowFilePath(args, directory) {
  if (args === null || typeof args !== "object" || !("filePath" in args)) return null;
  const raw = args.filePath;
  if (typeof raw !== "string" || raw.length === 0) return null;
  return isAbsolute2(raw) ? raw : resolvePath(directory, raw);
}
function wikiContextBlock(output) {
  return `<wiki>
${output}
</wiki>`;
}
function assemblePlugin(deps = {}) {
  const directory = deps.directory ?? process.cwd();
  const resolveBinary = deps.resolveBinary ?? (() => resolveWikiBinary());
  const executeCheck = deps.executeCheck ?? ((filePath, options) => runWikiCheck(filePath, { binary: options.binary, timeoutMs: WIKI_CHECK_TIMEOUT_MS, cwd: directory }));
  const afterHandler = async (input, output) => {
    try {
      if (input === null || typeof input !== "object") return;
      const toolId = typeof input.tool === "string" ? input.tool : "";
      if (!EDIT_TOOL_IDS.has(toolId)) return;
      const filePath = narrowFilePath(input.args, directory);
      if (filePath === null) return;
      if (!isWikiFile(filePath, directory)) return;
      const result = executeCheck(filePath, { binary: resolveBinary() });
      if (result.status !== "residual") return;
      const diagnostics = result.output;
      if (!diagnostics) return;
      if (output === null || typeof output !== "object") return;
      const prior = typeof output.output === "string" ? output.output : "";
      output.output = `${prior}
${wikiContextBlock(diagnostics)}`;
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
