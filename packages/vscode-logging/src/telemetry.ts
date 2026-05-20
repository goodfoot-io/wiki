/**
 * Telemetry integration with consent compliance.
 *
 * Provides integration between logging and telemetry services.
 * The actual TelemetryService is injected via interface.
 *
 * @summary Telemetry integration with consent compliance
 */

import type { LogOutputChannel } from 'vscode';

/**
 * Error category for telemetry classification.
 */
export type ErrorCategory = 'user' | 'config' | 'extension' | 'git' | 'vscode';

/**
 * Context information for error telemetry.
 */
export interface ErrorContext {
  errorType?: string;
  errorCategory?: ErrorCategory;
  operationName?: string;
  gitAvailable?: boolean;
  [key: string]: string | boolean | undefined;
}

/**
 * Telemetry service interface for dependency injection.
 */
export interface TelemetryService {
  sendError(error: Error, context?: Partial<ErrorContext>): void;
}

/**
 * Module-level telemetry service instance.
 */
let telemetryServiceInstance: TelemetryService | undefined;

/**
 * Module-level logger instance.
 */
let loggerInstance: LogOutputChannel | undefined;

/**
 * Sets the telemetry service instance.
 *
 * @param service - Telemetry service instance, or undefined to unset
 */
export function setTelemetryService(service: TelemetryService | undefined): void {
  telemetryServiceInstance = service;
}

/**
 * Sets the logger instance for telemetry logging.
 *
 * @param logger - Logger instance, or undefined to unset
 */
export function setTelemetryLogger(logger: LogOutputChannel | undefined): void {
  loggerInstance = logger;
}

/**
 * Logs an error and sends it to telemetry if available.
 *
 * This function:
 * 1. Logs the error to the logger (if set)
 * 2. Sends Error objects to telemetry (if service is set)
 * 3. Gracefully degrades if either logger or telemetry is unavailable
 *
 * Non-Error objects are only logged, not sent to telemetry.
 *
 * @param message - Error message to log
 * @param error - Error object or other value
 * @param context - Additional context for telemetry
 */
export function logErrorWithTelemetry(message: string, error: unknown, context?: Partial<ErrorContext>): void {
  // Log to logger if available
  if (loggerInstance) {
    loggerInstance.error(message, error);
  }

  // Send to telemetry only if error is an Error object and service is available
  if (error instanceof Error && telemetryServiceInstance) {
    telemetryServiceInstance.sendError(error, {
      operationName: message,
      ...context
    });
  }
}

/**
 * Formats an error to a string.
 *
 * @param error - Error object or other value
 * @returns Formatted error string
 */
export function formatError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * Sanitizes error messages to remove PII (Personally Identifiable Information).
 *
 * Removes:
 * - Email addresses (user@example.com → [email])
 * - Usernames in paths (/Users/john/ → /Users/[user]/)
 * - API keys and tokens (sk-..., ghp_..., gho_..., ghu_..., ghs_..., ghr_...)
 *
 * @param message - Error message to sanitize
 * @returns Sanitized message
 */
export function sanitizeErrorMessage(message: string): string {
  let sanitized = message;

  // Strip email addresses
  sanitized = sanitized.replace(/[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}/g, '[email]');

  // Strip usernames in Unix paths (/Users/username/ or /home/username/)
  sanitized = sanitized.replace(/\/(Users|home)\/[^/\s]+\//g, '/$1/[user]/');

  // Strip usernames in Windows paths (C:\Users\username\)
  sanitized = sanitized.replace(/([A-Z]:\\Users\\)[^\\]+\\/gi, '$1[user]\\');

  // Strip API keys and tokens (common patterns)
  // OpenAI: sk-...
  // GitHub PAT: ghp_...
  // GitHub OAuth: gho_...
  // GitHub user-to-server: ghu_...
  // GitHub server-to-server: ghs_...
  // GitHub refresh: ghr_...
  sanitized = sanitized.replace(/\b(sk-|ghp_|gho_|ghu_|ghs_|ghr_)[a-zA-Z0-9_-]+/g, '[API_KEY]');

  return sanitized;
}
