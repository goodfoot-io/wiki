#!/usr/bin/env -S node --enable-source-maps
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

// ../../node_modules/@goodfoot/agent-hooks/dist/agents/codex/constants.js
var EVENTS_WITH_TEXT_OUTPUT = /* @__PURE__ */ new Set(["SessionStart", "UserPromptSubmit", "SubagentStart"]);

// ../../node_modules/@goodfoot/agent-hooks/dist/agents/codex/events.js
var HOOK_EVENT_NAMES = [
  "PreToolUse",
  "PostToolUse",
  "PermissionRequest",
  "UserPromptSubmit",
  "SessionStart",
  "SubagentStart",
  "Stop",
  "SubagentStop",
  "PreCompact",
  "PostCompact"
];
var EXCLUDED_FROM_ADVISORY = [
  "PreToolUse",
  "PostToolUse",
  "PermissionRequest",
  "Stop",
  "SubagentStop",
  "PreCompact",
  "PostCompact"
];
var ADVISORY_EVENTS = HOOK_EVENT_NAMES.filter((eventName) => !EXCLUDED_FROM_ADVISORY.includes(eventName));

// ../../node_modules/@goodfoot/agent-hooks/dist/core/define-hook.js
function defineHook(eventName, config, handler, policyGate) {
  if (policyGate !== void 0) {
    let accepted;
    try {
      accepted = policyGate(eventName, config.unexpectedError);
    } catch (error) {
      throw new Error(`Policy gate rejected "${eventName}": ${error instanceof Error ? error.message : String(error)}`);
    }
    if (!accepted) {
      throw new Error(`Policy gate rejected "${eventName}"`);
    }
  }
  const hookFn = async (input, context) => {
    return await handler(input, context);
  };
  hookFn.eventName = eventName;
  hookFn.matcher = config.matcher;
  hookFn.timeout = config.timeout;
  hookFn.unexpectedError = config.unexpectedError;
  hookFn.onUnexpectedError = config.onUnexpectedError;
  hookFn.createContext = config.createContext;
  return hookFn;
}

// ../../node_modules/@goodfoot/agent-hooks/dist/agents/codex/hooks.js
var advisoryPolicyGate = (eventName, policy) => policy !== "continue" || ADVISORY_EVENTS.includes(eventName);
function createHookFunction(hookEventName, config, handler) {
  const coreConfig = {
    matcher: "matcher" in config ? config.matcher : void 0,
    timeout: config.timeout,
    unexpectedError: config.unexpectedError,
    onUnexpectedError: config.onUnexpectedError
  };
  const hookFn = defineHook(hookEventName, coreConfig, handler, advisoryPolicyGate);
  const codexFn = hookFn;
  codexFn.hookEventName = hookEventName;
  codexFn.statusMessage = config.statusMessage;
  return codexFn;
}
function postToolUseHook(config, handler) {
  return createHookFunction("PostToolUse", config, handler);
}

// ../../node_modules/@goodfoot/agent-hooks/dist/core/stdin.js
async function readStdin() {
  return new Promise((resolve2, reject) => {
    const chunks = [];
    process.stdin.setEncoding("utf-8");
    process.stdin.on("data", (chunk) => {
      chunks.push(chunk);
    });
    process.stdin.on("end", () => {
      resolve2(chunks.join(""));
    });
    process.stdin.on("error", (error) => {
      reject(error);
    });
  });
}
function parseStdinJson(stdinContent) {
  return JSON.parse(stdinContent);
}

// ../../node_modules/@goodfoot/agent-hooks/dist/core/transport.js
var HookBlockError = class extends Error {
  /**
   * Optional structured fields carried alongside the block reason (e.g.
   * extra wire fields the agent's translation may forward).
   */
  fields;
  /**
   * @param message - The block reason; becomes the error `message`.
   * @param fields - Optional additional structured fields.
   */
  constructor(message, fields) {
    super(message);
    this.name = "HookBlockError";
    this.fields = fields;
  }
};
var FALLBACK_EXIT_ERROR = 1;
var FALLBACK_EXIT_SUCCESS = 0;
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
function classify(error, phase, policy, onUnexpectedError) {
  if (error instanceof HookBlockError) {
    return { kind: "block", error };
  }
  if (policy === "continue") {
    reportUnexpectedError(onUnexpectedError, error, phase);
    return { kind: "response", output: void 0 };
  }
  return { kind: "handlerError", error, phase };
}
function writeUnexpectedErrorStderr(error) {
  if (error instanceof Error) {
    process.stderr.write(`${error.stack ?? error.message}
`);
  } else {
    process.stderr.write(`${String(error)}
`);
  }
}
function cleanupQuietly() {
  try {
    logger.clearContext();
    logger.close();
  } catch {
  }
}
async function drive(transport, hookFn) {
  const policy = hookFn.unexpectedError ?? "error";
  const onUnexpectedError = hookFn.onUnexpectedError;
  const outcome = await (async () => {
    let stdinContent;
    try {
      stdinContent = await readStdin();
    } catch (error) {
      logger.logError(error, "Failed to read stdin");
      return classify(error, "read", policy, onUnexpectedError);
    }
    let input;
    try {
      input = parseStdinJson(stdinContent);
    } catch (error) {
      logger.logError(error, "Failed to parse stdin JSON");
      return classify(error, "parse", policy, onUnexpectedError);
    }
    logger.setContext(hookFn.eventName, input);
    const context = hookFn.createContext?.(input) ?? { logger };
    try {
      const result = await hookFn(input, context);
      if (result === null || result === void 0) {
        return { kind: "response", output: void 0 };
      }
      const raw = transport.rawStdout?.(result);
      return raw !== void 0 ? { kind: "rawStdout", stdout: raw } : { kind: "response", output: result };
    } catch (error) {
      return classify(error, "handler", policy, onUnexpectedError);
    }
  })();
  let finalized;
  try {
    finalized = transport.finalize(outcome);
  } catch (error) {
    if (policy === "continue") {
      reportUnexpectedError(onUnexpectedError, error, "serialize");
      cleanupQuietly();
      process.exit(FALLBACK_EXIT_SUCCESS);
    }
    writeUnexpectedErrorStderr(error);
    cleanupQuietly();
    process.exit(FALLBACK_EXIT_ERROR);
  }
  try {
    cleanup(policy, onUnexpectedError);
  } catch (error) {
    writeUnexpectedErrorStderr(error);
    process.exit(FALLBACK_EXIT_ERROR);
  }
  if (finalized.stderr !== void 0) {
    process.stderr.write(finalized.stderr);
  }
  if (finalized.stdout !== void 0) {
    try {
      process.stdout.write(finalized.stdout);
    } catch (error) {
      if (policy === "continue") {
        reportUnexpectedError(onUnexpectedError, error, "write");
        cleanupQuietly();
        process.exit(FALLBACK_EXIT_SUCCESS);
      }
      writeUnexpectedErrorStderr(error);
      cleanupQuietly();
      process.exit(FALLBACK_EXIT_ERROR);
    }
  }
  process.exit(finalized.exitCode);
}

// ../../node_modules/@goodfoot/agent-hooks/dist/agents/codex/outputs.js
var EXIT_CODES = {
  SUCCESS: 0,
  ERROR: 1,
  BLOCK: 2
};
var BlockError = class extends HookBlockError {
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

// ../../node_modules/@goodfoot/agent-hooks/dist/agents/codex/transport.js
function convertToHookOutput(output) {
  return output.stderr !== void 0 ? { stdout: output.stdout, stderr: output.stderr } : { stdout: output.stdout };
}
function formatErrorText(error) {
  return error instanceof Error ? `${error.stack ?? error.message}
` : `${String(error)}
`;
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
function createCodexTransport() {
  return {
    finalize(outcome) {
      switch (outcome.kind) {
        case "response":
        case "rawStdout": {
          const stdoutJson = outcome.kind === "response" && outcome.output !== null && outcome.output !== void 0 ? JSON.stringify(convertToHookOutput(outcome.output).stdout) : "{}";
          return { stdout: stdoutJson, exitCode: EXIT_CODES.SUCCESS };
        }
        case "block": {
          const reason = outcome.error instanceof BlockError ? outcome.error.reason : outcome.error.message;
          return { stderr: `${reason}
`, exitCode: EXIT_CODES.BLOCK };
        }
        case "handlerError": {
          return { stderr: formatErrorText(outcome.error), exitCode: EXIT_CODES.ERROR };
        }
      }
    }
  };
}
async function execute(hookFn) {
  const eventName = hookFn.hookEventName;
  const composed = (input, context) => {
    const result = hookFn(input, context);
    const normalize = (value) => {
      if (typeof value === "string") {
        return normalizeStringOutput(eventName, value);
      }
      return value;
    };
    return result instanceof Promise ? result.then(normalize) : normalize(result);
  };
  composed.eventName = hookFn.eventName ?? eventName;
  composed.matcher = hookFn.matcher;
  composed.timeout = hookFn.timeout;
  composed.unexpectedError = hookFn.unexpectedError;
  composed.onUnexpectedError = hookFn.onUnexpectedError;
  composed.createContext = hookFn.createContext;
  await drive(createCodexTransport(), composed);
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
