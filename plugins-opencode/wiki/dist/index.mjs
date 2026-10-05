// src/opencode/index.ts
import { isAbsolute as isAbsolute2, resolve as resolvePath } from "node:path";

// ../../node_modules/@goodfoot/agent-hooks/dist/core/logger.js
import { closeSync, existsSync, mkdirSync, openSync, writeSync } from "node:fs";
import { dirname } from "node:path";
var LOG_LEVELS = ["debug", "info", "warn", "error"];
var Logger = class {
  /**
   * Registered event handlers by log level.
   */
  handlers = /* @__PURE__ */ new Map();
  /**
   * File descriptor for log file output.
   * Lazily initialized on first write.
   */
  logFileFd = null;
  /**
   * Path to the log file, if configured.
   */
  logFilePath = null;
  /**
   * Whether file initialization has been attempted.
   */
  fileInitialized = false;
  /**
   * Current hook context for enriching log events.
   */
  currentHookType;
  /**
   * Current hook input for enriching log events.
   */
  currentInput;
  /**
   * Creates a new Logger instance.
   *
   * Typically you should use the exported `logger` singleton rather than
   * creating new instances.
   * @param config - Optional configuration
   * @example
   * ```typescript
   * // Use singleton (recommended)
   * import { logger } from '@goodfoot/agent-hooks';
   *
   * // Or create custom instance
   * const customLogger = new Logger({ logFilePath: '/var/log/hooks.log' });
   * ```
   */
  constructor(config = {}) {
    for (const level of LOG_LEVELS) {
      this.handlers.set(level, /* @__PURE__ */ new Set());
    }
    this.logFilePath = config.logFilePath ?? (config.logEnvVar ? process.env[config.logEnvVar] : void 0) ?? null;
  }
  /**
   * Logs a debug message.
   *
   * Use for detailed debugging information that is typically only useful
   * during development or troubleshooting.
   * @param message - The debug message
   * @param context - Optional additional context
   * @example
   * ```typescript
   * logger.debug('Processing tool input', { toolName: 'Bash', inputSize: 256 });
   * ```
   */
  debug(message, context) {
    this.emit("debug", message, context);
  }
  /**
   * Logs an info message.
   *
   * Use for general operational events like hook invocations, successful
   * completions, or state changes.
   * @param message - The info message
   * @param context - Optional additional context
   * @example
   * ```typescript
   * logger.info('Session started', { source: 'startup', sessionId: 'abc123' });
   * ```
   */
  info(message, context) {
    this.emit("info", message, context);
  }
  /**
   * Logs a warning message.
   *
   * Use for conditions that may indicate issues but don't prevent
   * operation, such as deprecated patterns or performance concerns.
   * @param message - The warning message
   * @param context - Optional additional context
   * @example
   * ```typescript
   * logger.warn('Deprecated hook pattern detected', { pattern: 'legacyMatcher' });
   * ```
   */
  warn(message, context) {
    this.emit("warn", message, context);
  }
  /**
   * Logs an error message.
   *
   * Use for error conditions that require attention but were handled
   * gracefully. For exceptions, prefer {@link logError}.
   * @param message - The error message
   * @param context - Optional additional context
   * @example
   * ```typescript
   * logger.error('Failed to validate tool input', { toolName: 'Bash', reason: 'empty command' });
   * ```
   */
  error(message, context) {
    this.emit("error", message, context);
  }
  /**
   * Logs a structured error with full error details.
   *
   * Use this method when logging caught exceptions to capture the full
   * error context including name, message, stack trace, and cause chain.
   * @param error - The error to log
   * @param message - Human-readable description of what failed
   * @param context - Optional additional context
   * @example
   * ```typescript
   * try {
   *   await dangerousOperation();
   * } catch (err) {
   *   logger.logError(err, 'Failed to execute dangerous operation', {
   *     operation: 'delete',
   *     target: '/important/file.txt'
   *   });
   * }
   * ```
   */
  logError(error, message, context) {
    const errorInfo = this.extractErrorInfo(error);
    const event = {
      timestamp: (/* @__PURE__ */ new Date()).toISOString(),
      level: "error",
      hookType: this.currentHookType,
      message,
      input: this.currentInput,
      error: errorInfo,
      context
    };
    this.deliverEvent(event);
  }
  /**
   * Subscribes a handler to log events at the specified level.
   *
   * The handler will be called for every log event at the specified level.
   * Returns an unsubscribe function that should be called when the handler
   * is no longer needed.
   * @param level - The log level to subscribe to
   * @param handler - The handler function to call for each event
   * @returns A function to unsubscribe the handler
   * @example
   * ```typescript
   * // Subscribe to error events
   * const unsubscribe = logger.on('error', (event) => {
   *   console.error(`[${event.hookType}] ${event.message}`);
   *   if (event.error) {
   *     console.error(event.error.stack);
   *   }
   * });
   *
   * // Later, clean up
   * unsubscribe();
   * ```
   * @example
   * ```typescript
   * // Forward to external logging library
   * import pino from 'pino';
   * const pinoLogger = pino();
   *
   * logger.on('info', (event) => pinoLogger.info(event, event.message));
   * logger.on('warn', (event) => pinoLogger.warn(event, event.message));
   * logger.on('error', (event) => pinoLogger.error(event, event.message));
   * ```
   */
  on(level, handler) {
    const levelHandlers = this.handlers.get(level);
    if (levelHandlers) {
      levelHandlers.add(handler);
    }
    return () => {
      levelHandlers?.delete(handler);
    };
  }
  /**
   * Sets the current hook context for enriching log events.
   *
   * This is called internally by the runtime before invoking hook handlers.
   * You typically don't need to call this directly.
   * @param hookType - The agent event name being executed
   * @param input - The hook input data
   * @internal
   */
  setContext(hookType, input) {
    this.currentHookType = hookType;
    this.currentInput = input;
  }
  /**
   * Clears the current hook context.
   *
   * Called internally by the runtime after hook execution completes.
   * @internal
   */
  clearContext() {
    this.currentHookType = void 0;
    this.currentInput = void 0;
  }
  /**
   * Configures the log file path at runtime.
   *
   * Call this to enable or change file logging. Setting to `null` disables
   * file logging (but doesn't close existing file handle immediately).
   * @param filePath - Path to the log file, or null to disable
   * @example
   * ```typescript
   * // Enable file logging at runtime
   * logger.setLogFile('/var/log/agent-hooks.log');
   *
   * // Disable file logging
   * logger.setLogFile(null);
   * ```
   */
  setLogFile(filePath) {
    if (this.logFileFd !== null) {
      try {
        closeSync(this.logFileFd);
      } catch (closeError) {
        process.stderr.write(`[agent-hooks] Failed to close log file: ${String(closeError)}
`);
      }
      this.logFileFd = null;
    }
    this.logFilePath = filePath;
    this.fileInitialized = false;
  }
  /**
   * Closes all resources held by the logger.
   *
   * Call this during graceful shutdown to ensure all log data is flushed.
   * @example
   * ```typescript
   * process.on('exit', () => {
   *   logger.close();
   * });
   * ```
   */
  close() {
    if (this.logFileFd !== null) {
      try {
        closeSync(this.logFileFd);
      } catch (closeError) {
        process.stderr.write(`[agent-hooks] Failed to close log file: ${String(closeError)}
`);
      }
      this.logFileFd = null;
    }
    this.fileInitialized = false;
  }
  /**
   * Checks if there are any active handlers or destinations.
   *
   * Returns true if any handlers are registered or file logging is enabled.
   * @returns Whether the logger has any active output destinations
   */
  hasDestinations() {
    for (const handlers of this.handlers.values()) {
      if (handlers.size > 0)
        return true;
    }
    return this.logFilePath !== null;
  }
  // ============================================================================
  // Private Methods
  // ============================================================================
  /**
   * Emits a log event.
   * @param level - The severity level of the event
   * @param message - The log message
   * @param context - Optional additional context data
   */
  emit(level, message, context) {
    const event = {
      timestamp: (/* @__PURE__ */ new Date()).toISOString(),
      level,
      hookType: this.currentHookType,
      message,
      input: this.currentInput,
      context
    };
    this.deliverEvent(event);
  }
  /**
   * Delivers an event to all registered destinations.
   * @param event - The log event to deliver
   */
  deliverEvent(event) {
    const levelHandlers = this.handlers.get(event.level);
    if (levelHandlers) {
      for (const handler of levelHandlers) {
        try {
          handler(event);
        } catch (handlerError) {
          process.stderr.write(`[agent-hooks] Log handler error: ${String(handlerError)}
`);
        }
      }
    }
    this.writeToFile(event);
  }
  /**
   * Writes an event to the log file.
   * @param event - The log event to write
   */
  writeToFile(event) {
    if (!this.logFilePath)
      return;
    if (!this.fileInitialized) {
      this.initializeFile();
    }
    if (this.logFileFd === null)
      return;
    try {
      const line = `${JSON.stringify(event)}
`;
      writeSync(this.logFileFd, line);
    } catch (writeError) {
      this.logFileFd = null;
      this.fileInitialized = false;
      process.stderr.write(`[agent-hooks] Log file write failed: ${String(writeError)}
`);
    }
  }
  /**
   * Initializes the log file for writing.
   */
  initializeFile() {
    this.fileInitialized = true;
    if (!this.logFilePath)
      return;
    try {
      const dir = dirname(this.logFilePath);
      if (!existsSync(dir)) {
        mkdirSync(dir, { recursive: true });
      }
      this.logFileFd = openSync(this.logFilePath, "a");
    } catch {
      this.logFileFd = null;
    }
  }
  /**
   * Extracts structured error information from an unknown error.
   * @param error - The error to extract information from
   * @returns Structured error information
   */
  extractErrorInfo(error) {
    if (error instanceof Error) {
      const info = {
        name: error.name,
        message: error.message,
        stack: error.stack
      };
      if (error.cause !== void 0) {
        info.cause = this.extractErrorInfo(error.cause);
      }
      return info;
    }
    return {
      name: "UnknownError",
      message: String(error)
    };
  }
};
var logger = new Logger({
  logEnvVar: process.env.AGENT_HOOKS_LOG_ENV_VAR ?? "AGENT_HOOKS_LOG_FILE"
});

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
    const titleValue = line.match(/^title\s*:\s*(.+)$/)?.[1];
    if (titleValue !== void 0) title = unquoteScalar(titleValue);
    const summaryValue = line.match(/^summary\s*:\s*(.+)$/)?.[1];
    if (summaryValue !== void 0) summary = unquoteScalar(summaryValue);
  }
  return title.length > 0 && summary.length > 0;
}
function unquoteScalar(raw) {
  return raw.trim().replace(/^['"]|['"]$/g, "");
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
function findManagedWikiBinary(logger2) {
  for (const root of vscodeGlobalStorageRoots()) {
    const binRoot = join(root, "goodfoot.wiki-extension", "bin");
    if (!existsSync2(binRoot)) continue;
    const versions = listSubdirectories(binRoot, logger2);
    versions.sort((a, b) => compareSemver(b, a));
    for (const version of versions) {
      const versionDir = join(binRoot, version);
      for (const target of listSubdirectories(versionDir, logger2)) {
        const candidate = join(versionDir, target, WIKI_EXECUTABLE);
        if (existsSync2(candidate)) return candidate;
      }
    }
  }
  return null;
}
var VANISHED_DIRECTORY_CODES = /* @__PURE__ */ new Set(["ENOENT", "ENOTDIR"]);
function listSubdirectories(dir, logger2) {
  try {
    return readdirSync(dir, { withFileTypes: true }).filter((entry) => entry.isDirectory() || entry.isSymbolicLink()).map((entry) => entry.name);
  } catch (error) {
    const code = errorCode(error);
    if (code !== void 0 && VANISHED_DIRECTORY_CODES.has(code)) return [];
    logger2?.warn("managed wiki binary directory unreadable \u2014 skipped", {
      dir,
      error: error instanceof Error ? error.message : String(error)
    });
    return [];
  }
}
function errorCode(error) {
  if (typeof error !== "object" || error === null || !("code" in error)) return void 0;
  const { code } = error;
  return typeof code === "string" ? code : void 0;
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
  const managed = findManagedWikiBinary(logger2);
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
    const path = match[1]?.trim();
    if (path !== void 0 && path.length > 0 && !paths.includes(path)) paths.push(path);
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
  const logger2 = deps.logger ?? logger;
  const resolveBinary = deps.resolveBinary ?? (() => resolveWikiBinary(logger2));
  const executeCheck = deps.executeCheck ?? ((filePath, options) => runWikiCheck(filePath, { binary: options.binary, timeoutMs: WIKI_CHECK_TIMEOUT_MS, cwd: directory }));
  const afterHandler = async (input, output) => {
    try {
      if (input === null || typeof input !== "object") return;
      const toolId = typeof input.tool === "string" ? input.tool : "";
      if (toolId !== PATCH_TOOL_ID && !WRITE_TOOL_IDS.has(toolId)) return;
      const paths = candidatePaths(toolId, input.args, directory).map(
        (p) => isAbsolute2(p) ? p : resolvePath(directory, p)
      );
      const wikiPaths = paths.filter((p) => isWikiFile(p, directory));
      if (wikiPaths.length === 0) return;
      const wikiBin = resolveBinary();
      const sections = [];
      let unavailable = null;
      for (const filePath of wikiPaths) {
        const result = executeCheck(filePath, { binary: wikiBin });
        if (result.status === "unavailable") {
          unavailable ??= { filePath, detail: result.output ?? "spawn failed" };
          continue;
        }
        if (result.status === "residual" && result.output) sections.push(result.output);
      }
      if (unavailable !== null) {
        logger2.warn("wiki check execution error", { error: unavailable.detail, wikiBin });
      }
      if (sections.length === 0 && unavailable === null) return;
      if (output === null || typeof output !== "object") return;
      const payload = unavailable !== null ? wikiUnavailableBlock(unavailable.filePath, wikiBin, unavailable.detail) : wikiContextBlock(sections.join("\n\n"));
      const prior = typeof output.output === "string" ? output.output : "";
      output.output = `${prior}
${payload}`;
    } catch (error) {
      logger2.logError(error, "wiki opencode tool.execute.after failed; wiki check skipped");
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
var pluginModule = {
  id: "@goodfoot/opencode-wiki",
  server: wikiOpencode
};
var index_default = pluginModule;
export {
  assemblePlugin,
  index_default as default,
  wikiOpencode
};
