/**
 * Shared Wiki output channel logger.
 *
 * Creates a single `LogOutputChannel` named "Wiki" backed by the
 * `@goodfoot/vscode-logging` package. Levels follow VS Code's output panel
 * log-level selector — set to Trace via "Developer: Set Log Level…" or the
 * gear icon on the Wiki channel to see install/spawn/duration detail.
 *
 * @summary Shared Wiki output channel logger.
 */

import { createLogger, type DecoratedLogger, initializeLoggers } from '@goodfoot/vscode-logging';
import type * as vscode from 'vscode';

let logger: DecoratedLogger | undefined;

/**
 * Return the global Wiki logger, creating it on first use.
 *
 * @returns The shared Wiki `DecoratedLogger`.
 */
export function getWikiLogger(): DecoratedLogger {
  if (logger == null) {
    logger = createLogger('Wiki');
  }
  return logger;
}

/**
 * Register the Wiki logger for disposal when the extension deactivates.
 *
 * @param context - VS Code extension context.
 */
export function registerWikiLogger(context: vscode.ExtensionContext): void {
  initializeLoggers(context, getWikiLogger());
}

/**
 * Format an unknown thrown value for log messages.
 *
 * Preserves Error stacks where present so they appear in the Output panel.
 *
 * @param error - Unknown thrown value.
 * @returns A human-readable string with stack trace when available.
 */
export function formatLogError(error: unknown): string {
  if (error instanceof Error) {
    return error.stack ?? `${error.name}: ${error.message}`;
  }
  return String(error);
}
