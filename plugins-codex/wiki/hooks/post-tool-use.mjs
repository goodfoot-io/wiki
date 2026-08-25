#!/usr/bin/env -S node --enable-source-maps
// ../../node_modules/@goodfoot/codex-hooks/dist/constants.js
var EVENTS_WITH_TEXT_OUTPUT = /* @__PURE__ */ new Set(["SessionStart", "UserPromptSubmit", "SubagentStart"]);

// ../../node_modules/@goodfoot/codex-hooks/dist/hooks.js
function attachMetadata(hookEventName, config, handler) {
  const hook = handler;
  hook.hookEventName = hookEventName;
  hook.timeout = config.timeout;
  hook.statusMessage = config.statusMessage;
  hook.unexpectedError = config.unexpectedError;
  hook.onUnexpectedError = config.onUnexpectedError;
  if ("matcher" in config && typeof config.matcher === "string") {
    hook.matcher = config.matcher;
  }
  return hook;
}
function postToolUseHook(config, handler) {
  return attachMetadata("PostToolUse", config, handler);
}

// ../../node_modules/@goodfoot/codex-hooks/dist/logger.js
import { closeSync, existsSync, mkdirSync, openSync, writeSync } from "node:fs";
import { dirname } from "node:path";
var DEFAULT_LOG_ENV_VAR = "CODEX_HOOKS_LOG_FILE";
var Logger = class {
  handlers = /* @__PURE__ */ new Map();
  fileInitialized = false;
  logFileFd = null;
  logFilePath = null;
  currentHookType;
  currentInput;
  constructor(config = {}) {
    this.logFilePath = config.logFilePath ?? process.env[config.logEnvVar ?? DEFAULT_LOG_ENV_VAR] ?? null;
  }
  setContext(hookType, input) {
    this.currentHookType = hookType;
    this.currentInput = input;
  }
  clearContext() {
    this.currentHookType = void 0;
    this.currentInput = void 0;
  }
  on(level, handler) {
    const existing = this.handlers.get(level) ?? /* @__PURE__ */ new Set();
    existing.add(handler);
    this.handlers.set(level, existing);
    return () => {
      existing.delete(handler);
      if (existing.size === 0) {
        this.handlers.delete(level);
      }
    };
  }
  debug(message, context) {
    this.emit("debug", message, context);
  }
  info(message, context) {
    this.emit("info", message, context);
  }
  warn(message, context) {
    this.emit("warn", message, context);
  }
  error(message, context) {
    this.emit("error", message, context);
  }
  logError(error, message, context) {
    this.emit("error", `${message}: ${error instanceof Error ? error.message : String(error)}`, context);
  }
  close() {
    if (this.logFileFd !== null) {
      closeSync(this.logFileFd);
      this.logFileFd = null;
    }
  }
  emit(level, message, context) {
    const event = {
      timestamp: (/* @__PURE__ */ new Date()).toISOString(),
      level,
      hookType: this.currentHookType,
      message,
      ...this.currentInput !== void 0 ? { input: this.currentInput } : {},
      ...context !== void 0 ? { context } : {}
    };
    this.writeToFile(event);
    this.handlers.get(level)?.forEach((handler) => {
      handler(event);
    });
  }
  writeToFile(event) {
    if (this.logFilePath === null) {
      return;
    }
    if (!this.fileInitialized) {
      this.fileInitialized = true;
      const logDir = dirname(this.logFilePath);
      if (!existsSync(logDir)) {
        mkdirSync(logDir, { recursive: true });
      }
      this.logFileFd = openSync(this.logFilePath, "a");
    }
    if (this.logFileFd !== null) {
      writeSync(this.logFileFd, `${JSON.stringify(event)}
`);
    }
  }
};
var logger = new Logger();

// ../../node_modules/@goodfoot/codex-hooks/dist/outputs.js
var EXIT_CODES = {
  SUCCESS: 0,
  ERROR: 1,
  BLOCK: 2
};
var BlockError = class extends Error {
  reason;
  constructor(reason) {
    super(reason);
    this.name = "BlockError";
    this.reason = reason;
  }
};
function omitUndefined(value) {
  return Object.fromEntries(Object.entries(value).filter(([, entry]) => entry !== void 0));
}
function buildOutput(type, stdout, stderr) {
  return {
    _type: type,
    stdout: omitUndefined(stdout),
    ...stderr !== void 0 ? { stderr } : {}
  };
}
function postToolUseOutput(options = {}) {
  const hasSpecific = options.additionalContext !== void 0 || options.updatedMCPToolOutput !== void 0;
  const hookSpecificOutput = hasSpecific ? omitUndefined({
    hookEventName: "PostToolUse",
    additionalContext: options.additionalContext,
    updatedMCPToolOutput: options.updatedMCPToolOutput
  }) : void 0;
  return buildOutput("PostToolUse", {
    continue: options.continue,
    stopReason: options.stopReason,
    suppressOutput: options.suppressOutput,
    systemMessage: options.systemMessage,
    decision: options.decision,
    reason: options.reason,
    hookSpecificOutput
  });
}
function userPromptSubmitOutput(options = {}) {
  const hookSpecificOutput = options.additionalContext !== void 0 ? {
    hookEventName: "UserPromptSubmit",
    additionalContext: options.additionalContext
  } : void 0;
  return buildOutput("UserPromptSubmit", {
    continue: options.continue,
    stopReason: options.stopReason,
    suppressOutput: options.suppressOutput,
    systemMessage: options.systemMessage,
    decision: options.decision,
    reason: options.reason,
    hookSpecificOutput
  });
}
function sessionStartOutput(options = {}) {
  const hookSpecificOutput = options.additionalContext !== void 0 ? {
    hookEventName: "SessionStart",
    additionalContext: options.additionalContext
  } : void 0;
  return buildOutput("SessionStart", {
    continue: options.continue,
    stopReason: options.stopReason,
    suppressOutput: options.suppressOutput,
    systemMessage: options.systemMessage,
    hookSpecificOutput
  });
}
function subagentStartOutput(options = {}) {
  const hookSpecificOutput = options.additionalContext !== void 0 ? {
    hookEventName: "SubagentStart",
    additionalContext: options.additionalContext
  } : void 0;
  return buildOutput("SubagentStart", {
    continue: options.continue,
    stopReason: options.stopReason,
    suppressOutput: options.suppressOutput,
    systemMessage: options.systemMessage,
    hookSpecificOutput
  });
}

// ../../node_modules/@goodfoot/codex-hooks/dist/runtime.js
var EMPTY_OUTPUT = { stdout: {} };
async function readStdin() {
  return new Promise((resolve2, reject) => {
    const chunks = [];
    process.stdin.setEncoding("utf-8");
    process.stdin.on("data", (chunk) => chunks.push(chunk));
    process.stdin.on("end", () => resolve2(chunks.join("")));
    process.stdin.on("error", reject);
  });
}
function parseStdinInput(stdinContent) {
  return JSON.parse(stdinContent);
}
function serializeStdout(output) {
  return JSON.stringify(output.stdout);
}
function normalizeStringOutput(hookEventName, result) {
  if (!EVENTS_WITH_TEXT_OUTPUT.has(hookEventName)) {
    throw new Error(`${hookEventName} hooks cannot return plain text`);
  }
  if (hookEventName === "SessionStart") {
    return sessionStartOutput({ additionalContext: result });
  }
  if (hookEventName === "SubagentStart") {
    return subagentStartOutput({ additionalContext: result });
  }
  return userPromptSubmitOutput({ additionalContext: result });
}
function convertToHookOutput(output) {
  return output.stderr !== void 0 ? { stdout: output.stdout, stderr: output.stderr } : { stdout: output.stdout };
}
function writeStderr(error) {
  if (error instanceof Error) {
    process.stderr.write(`${error.stack ?? error.message}
`);
  } else {
    process.stderr.write(`${String(error)}
`);
  }
}
function reportUnexpectedError(onUnexpectedError, error, phase) {
  try {
    onUnexpectedError?.(error, phase);
  } catch {
  }
  try {
    logger.logError(error, `Unexpected error in ${phase} phase (fail-open)`, { phase });
  } catch {
  }
}
function cleanup(policy, onUnexpectedError) {
  try {
    logger.clearContext();
    logger.close();
  } catch (error) {
    if (policy !== "continue") {
      throw error;
    }
    reportUnexpectedError(onUnexpectedError, error, "cleanup");
  }
}
async function execute(hookFn) {
  const policy = hookFn.unexpectedError ?? "error";
  const onUnexpectedError = hookFn.onUnexpectedError;
  let phase = "read";
  let output;
  try {
    const stdinContent = await readStdin();
    phase = "parse";
    const input = parseStdinInput(stdinContent);
    logger.setContext(hookFn.hookEventName, input);
    const context = { logger };
    phase = "handler";
    const result = await hookFn(input, context);
    phase = "serialize";
    if (typeof result === "string") {
      output = convertToHookOutput(normalizeStringOutput(hookFn.hookEventName, result));
    } else if (result !== void 0) {
      output = convertToHookOutput(result);
    } else {
      output = EMPTY_OUTPUT;
    }
    serializeStdout(output);
  } catch (error) {
    if (error instanceof BlockError) {
      cleanup(policy, onUnexpectedError);
      process.stderr.write(`${error.reason}
`);
      process.exit(EXIT_CODES.BLOCK);
    }
    if (policy !== "continue") {
      cleanup(policy, onUnexpectedError);
      writeStderr(error);
      process.exit(EXIT_CODES.ERROR);
    }
    reportUnexpectedError(onUnexpectedError, error, phase);
    output = EMPTY_OUTPUT;
  }
  phase = "write";
  try {
    process.stdout.write(serializeStdout(output));
  } catch (error) {
    if (policy !== "continue") {
      cleanup(policy, onUnexpectedError);
      writeStderr(error);
      process.exit(EXIT_CODES.ERROR);
    }
    reportUnexpectedError(onUnexpectedError, error, "write");
  }
  cleanup(policy, onUnexpectedError);
  process.exit(EXIT_CODES.SUCCESS);
}

// src/common/wiki-check.ts
import { spawnSync } from "node:child_process";
import { closeSync as closeSync2, existsSync as existsSync2, openSync as openSync2, readdirSync, readSync } from "node:fs";
import { homedir } from "node:os";
import { isAbsolute, join, resolve } from "node:path";
var FRONTMATTER_SCAN_BYTES = 4096;
var FRONTMATTER_SCAN_LINES = 30;
var DEFAULT_WIKI_CHECK_TIMEOUT_MS = 25e3;
function readFrontmatterPrefix(absPath) {
  const buf = Buffer.alloc(FRONTMATTER_SCAN_BYTES);
  const fd = openSync2(absPath, "r");
  try {
    const bytesRead = readSync(fd, buf, 0, FRONTMATTER_SCAN_BYTES, 0);
    let head = buf.toString("utf-8", 0, bytesRead);
    if (bytesRead === FRONTMATTER_SCAN_BYTES) {
      const lastNewline = head.lastIndexOf("\n");
      if (lastNewline !== -1) head = head.slice(0, lastNewline);
    }
    return head;
  } finally {
    closeSync2(fd);
  }
}
function isWikiFile(filePath, cwd) {
  if (!filePath.endsWith(".md")) return false;
  const absPath = isAbsolute(filePath) ? filePath : resolve(cwd, filePath);
  if (!existsSync2(absPath)) return false;
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
    if (!existsSync2(binRoot)) continue;
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
        if (existsSync2(candidate)) return candidate;
      }
    }
  }
  return null;
}
function resolveWikiBinary(logger2) {
  const override = process.env.WIKI_BIN;
  if (override && existsSync2(override)) return override;
  if (override) {
    logger2?.warn("WIKI_BIN override rejected \u2014 path does not exist", { wikiBin: override });
  }
  const whichCmd = process.platform === "win32" ? "where" : "which";
  const onPath = spawnSync(whichCmd, [WIKI_EXECUTABLE], { encoding: "utf8" });
  if (onPath.status === 0 && onPath.stdout) {
    const first = onPath.stdout.split(/\r?\n/).map((l) => l.trim()).filter(Boolean)[0];
    if (first && existsSync2(first)) return first;
  }
  const managed = findManagedWikiBinary();
  if (managed) {
    logger2?.info("resolved wiki binary from VS Code globalStorage", { path: managed });
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

// src/codex/post-tool-use.ts
var WIKI_CHECK_TIMEOUT_MS = 25e3;
function narrowPatchText(toolInput) {
  if (toolInput !== null && typeof toolInput !== "undefined" && typeof toolInput === "object" && "command" in toolInput) {
    const command = toolInput.command;
    if (typeof command === "string") return command;
  }
  return null;
}
function createHandler() {
  return async (input, { logger: logger2 }) => {
    const patchText = narrowPatchText(input.tool_input);
    if (patchText === null) return void 0;
    const filePaths = extractPatchedFilePaths(patchText);
    if (filePaths.length === 0) return void 0;
    const wikiBin = resolveWikiBinary(logger2);
    const sections = [];
    let unavailableDetail = null;
    for (const filePath of filePaths) {
      if (!isWikiFile(filePath, input.cwd)) continue;
      const result = runWikiCheck(filePath, { binary: wikiBin, timeoutMs: WIKI_CHECK_TIMEOUT_MS, cwd: input.cwd });
      if (result.status === "unavailable") {
        unavailableDetail ??= result.output ?? "spawn failed";
        continue;
      }
      if (result.status === "residual" && result.output) sections.push(result.output);
    }
    if (unavailableDetail !== null) {
      logger2.warn("wiki check execution error", { error: unavailableDetail, wikiBin });
      return postToolUseOutput({
        additionalContext: wikiUnavailableBlock(filePaths[0], wikiBin, unavailableDetail)
      });
    }
    if (sections.length === 0) return void 0;
    return postToolUseOutput({ additionalContext: wikiContextBlock(sections.join("\n\n")) });
  };
}
var post_tool_use_default = postToolUseHook(
  { matcher: "apply_patch|exec_command|exec|shell|local_shell", timeout: 6e4 },
  createHandler()
);

// src/codex/post-tool-use-entry.ts
execute(post_tool_use_default);
