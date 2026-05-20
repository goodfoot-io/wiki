/**
 * Logger factory with child logger support.
 *
 * Provides decorated LogOutputChannel instances that integrate with
 * LogBuffer for error reporting and LoggerDebugEmitter for test interception.
 *
 * @summary Logger factory with child logger support
 */

import { format } from 'node:util';
import type { LogOutputChannel } from 'vscode';
import { window } from 'vscode';
import { LoggerDebugEmitter } from './debugEmitter.js';
import { LogBuffer } from './logBuffer.js';

/**
 * Options for creating a logger.
 */
export interface LoggerOptions {
  /**
   * When enabled, prepends source location to messages.
   * Format: "[file:line:col] message"
   *
   * **WARNING: Very slow - development debugging only**
   */
  sourceLocationTracking?: boolean;
}

/**
 * Extension context interface for registering disposables.
 */
export interface ExtensionContextLike {
  subscriptions: { push(disposable: { dispose(): void }): void };
}

/**
 * Extended LogOutputChannel with child logger support and debug emitter access.
 */
export interface DecoratedLogger extends LogOutputChannel {
  /**
   * Creates a child logger with a hierarchical label.
   * Child labels are dot-separated: "Parent" -> "Parent.Child"
   * Child loggers are cached by full label path.
   * Child log messages include prefix: "[Parent.Child] message"
   *
   * @param options - Child logger options
   * @param options.label - Child label segment appended to the parent path
   * @returns Decorated child logger sharing parent's buffer and emitter
   */
  getChildLogger(options: { label: string }): DecoratedLogger;

  /**
   * Debug emitter for test interception.
   * When listeners are attached, log output is emitted instead of written to OutputChannel.
   */
  readonly debugEmitter: LoggerDebugEmitter;
}

/**
 * Global log buffer singleton for capturing recent log entries.
 */
const logBuffer = new LogBuffer(500);

/**
 * Gets the source location from the call stack.
 * Returns formatted string: "file:line:col"
 *
 * WARNING: This is very slow - only use for development debugging.
 *
 * @returns Caller location in `file:line:col` format, or `unknown:0:0` when unavailable
 */
function getSourceLocation(): string {
  const error = new Error();
  const stack = error.stack;
  if (!stack) {
    return 'unknown:0:0';
  }

  // Parse stack trace to find caller (skip getSourceLocation, decorateMethod, and the log method)
  const lines = stack.split('\n');
  // Stack format: "Error\n    at getSourceLocation (...)\n    at decorateMethod (...)\n    at Object.info (...)\n    at caller (...)"
  // We want the caller, which is typically at index 4 or later
  for (let i = 4; i < lines.length; i++) {
    const line = lines[i];
    if (!line) continue;
    // Match patterns like "at functionName (file:line:col)" or "at file:line:col"
    const match = line.match(/at\s+(?:.*?\s+\()?(.+?):(\d+):(\d+)\)?$/);
    if (match) {
      const file = match[1];
      const lineNum = match[2];
      const col = match[3];
      if (!file || !lineNum || !col) continue;
      // Extract just the filename from the path
      const fileName = file.split('/').pop() || file;
      return `${fileName}:${lineNum}:${col}`;
    }
  }

  return 'unknown:0:0';
}

/**
 * Stored references to the truly original (undecorated) channel methods.
 * These are captured once when the root logger is created and passed to all children.
 */
interface OriginalMethods {
  error: (message: string, ...args: unknown[]) => void;
  warn: (message: string, ...args: unknown[]) => void;
  info: (message: string, ...args: unknown[]) => void;
  debug: (message: string, ...args: unknown[]) => void;
  trace: (message: string, ...args: unknown[]) => void;
}

/**
 * Creates a wrapper logger that delegates to the channel but has its own log methods.
 * Each wrapper has its own label prefix and child cache, but shares the channel and originals.
 *
 * @param channel - Original OutputChannel (for non-log methods like dispose, show, etc.)
 * @param debugEmitter - Debug emitter for test interception
 * @param options - Runtime decoration flags, including source location tracking behavior
 * @param originals - Original undecorated methods (captured from channel once)
 * @param labelPrefix - Optional prefix for child logger messages
 * @returns Decorated logger wrapper
 */
function createLoggerWrapper(
  channel: LogOutputChannel,
  debugEmitter: LoggerDebugEmitter,
  options: LoggerOptions,
  originals: OriginalMethods,
  labelPrefix?: string
): DecoratedLogger {
  const { sourceLocationTracking = false } = options;

  // Cache for child loggers (each wrapper has its own cache)
  const childLoggerCache = new Map<string, DecoratedLogger>();

  /**
   * Matches printf-style format specifiers supported by util.format.
   * Used to detect when substitution is needed vs plain arg passthrough.
   */
  const FORMAT_SPECIFIER_RE = /%[sdifjoO]/;

  /**
   * Applies printf-style substitution when the message contains format specifiers.
   *
   * VSCode's LogOutputChannel does not perform util.format-style substitution;
   * it simply stringifies and appends extra args. This function resolves
   * placeholders like %s, %d, etc. and returns any unconsumed args for
   * passthrough to the channel.
   *
   * @param message - Log message, possibly containing format specifiers
   * @param args - Substitution values
   * @returns Formatted message and any remaining args
   */
  function formatArgs(message: string, args: unknown[]): { message: string; rest: unknown[] } {
    if (args.length > 0 && FORMAT_SPECIFIER_RE.test(message)) {
      return { message: format(message, ...args), rest: [] };
    }
    return { message, rest: args };
  }

  /**
   * Prepares the message with optional source location and label prefix.
   *
   * @param message - Raw log message before decoration
   * @returns Message decorated with label and optional source location prefixes
   */
  function prepareMessage(message: string): string {
    let result = message;

    // Add label prefix for child loggers
    if (labelPrefix) {
      result = `[${labelPrefix}] ${result}`;
    }

    // Add source location if enabled
    if (sourceLocationTracking) {
      const location = getSourceLocation();
      result = `[${location}] ${result}`;
    }

    return result;
  }

  // Create a wrapper object that delegates to the channel for non-log methods
  // but has its own log methods with the appropriate prefix
  const wrapper: DecoratedLogger = {
    // Delegate channel properties
    get name() {
      return channel.name;
    },
    get logLevel() {
      return channel.logLevel;
    },
    get onDidChangeLogLevel() {
      return channel.onDidChangeLogLevel;
    },

    // Delegate channel methods
    append: (value: string) => channel.append(value),
    appendLine: (value: string) => channel.appendLine(value),
    replace: (value: string) => channel.replace(value),
    clear: () => channel.clear(),
    show: channel.show.bind(channel),
    hide: () => channel.hide(),
    dispose: () => channel.dispose(),

    // Custom log methods with prefix support.
    // Uses formatArgs for printf-style substitution (%s, %d, etc.) since
    // VSCode's LogOutputChannel does not perform substitution itself.
    // Args without format specifiers pass through to the channel unchanged.
    error: (error: string | Error, ...args: unknown[]): void => {
      const rawMessage = error instanceof Error ? error.message : error;
      const { message: formatted, rest } = formatArgs(rawMessage, args);
      const message = prepareMessage(formatted);
      logBuffer.append('error', message);
      if (debugEmitter.listenerCount('error') > 0) {
        debugEmitter.emit('error', error instanceof Error ? error : message, ...rest);
      } else {
        originals.error(message, ...rest);
      }
    },

    warn: (message: string, ...args: unknown[]): void => {
      const { message: formatted, rest } = formatArgs(message, args);
      const prepared = prepareMessage(formatted);
      logBuffer.append('warn', prepared);
      if (debugEmitter.listenerCount('warn') > 0) {
        debugEmitter.emit('warn', prepared, ...rest);
      } else {
        originals.warn(prepared, ...rest);
      }
    },

    info: (message: string, ...args: unknown[]): void => {
      const { message: formatted, rest } = formatArgs(message, args);
      const prepared = prepareMessage(formatted);
      logBuffer.append('info', prepared);
      if (debugEmitter.listenerCount('info') > 0) {
        debugEmitter.emit('info', prepared, ...rest);
      } else {
        originals.info(prepared, ...rest);
      }
    },

    debug: (message: string, ...args: unknown[]): void => {
      const { message: formatted, rest } = formatArgs(message, args);
      const prepared = prepareMessage(formatted);
      logBuffer.append('debug', prepared);
      if (debugEmitter.listenerCount('debug') > 0) {
        debugEmitter.emit('debug', prepared, ...rest);
      } else {
        originals.debug(prepared, ...rest);
      }
    },

    trace: (message: string, ...args: unknown[]): void => {
      const { message: formatted, rest } = formatArgs(message, args);
      const prepared = prepareMessage(formatted);
      logBuffer.append('trace', prepared);
      if (debugEmitter.listenerCount('trace') > 0) {
        debugEmitter.emit('trace', prepared, ...rest);
      } else {
        originals.trace(prepared, ...rest);
      }
    },

    // Debug emitter accessor
    debugEmitter,

    // Child logger factory
    getChildLogger: (childOptions: { label: string }): DecoratedLogger => {
      const fullLabel = labelPrefix ? `${labelPrefix}.${childOptions.label}` : `${channel.name}.${childOptions.label}`;

      // Check cache first
      const cached = childLoggerCache.get(fullLabel);
      if (cached) {
        return cached;
      }

      // Create new wrapper with the full label, sharing the same originals
      const childLogger = createLoggerWrapper(channel, debugEmitter, options, originals, fullLabel);

      // Cache and return
      childLoggerCache.set(fullLabel, childLogger);
      return childLogger;
    }
  };

  return wrapper;
}

/**
 * Creates a new decorated logger.
 *
 * @param name - The name of the logger (displayed in VSCode Output panel)
 * @param options - Optional logger configuration
 * @returns Decorated logger with buffer integration and debug interception
 *
 * @example
 * ```typescript
 * const logger = createLogger('Cards');
 * logger.info('Extension activated');
 *
 * const gitLogger = logger.getChildLogger({ label: 'Git' });
 * gitLogger.debug('Fetching refs'); // Logs: "[Compare Branch.Git] Fetching refs"
 * ```
 */
export function createLogger(name: string, options?: LoggerOptions): DecoratedLogger {
  const channel = window.createOutputChannel(name, { log: true });
  const debugEmitter = new LoggerDebugEmitter();

  // Capture original methods once from the fresh channel
  const originals: OriginalMethods = {
    error: channel.error.bind(channel),
    warn: channel.warn.bind(channel),
    info: channel.info.bind(channel),
    debug: channel.debug.bind(channel),
    trace: channel.trace.bind(channel)
  };

  return createLoggerWrapper(channel, debugEmitter, options ?? {}, originals);
}

/**
 * Gets the global log buffer singleton.
 *
 * @returns The log buffer containing recent log entries
 */
export function getLogBuffer(): LogBuffer {
  return logBuffer;
}

/**
 * Registers loggers for proper cleanup when extension is deactivated.
 *
 * @param context - Extension context for managing disposables
 * @param loggers - One or more loggers to register
 */
export function initializeLoggers(context: ExtensionContextLike, ...loggers: DecoratedLogger[]): void {
  for (const logger of loggers) {
    context.subscriptions.push(logger);
  }
}
