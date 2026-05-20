/**
 * Structured event logging for observability and debugging.
 *
 * Provides logRefTrackingEvent for tracking the RefService control flow
 * with structured key=value logging and automatic git hash truncation.
 *
 * @summary Structured event logging for observability and debugging
 */

import type { LogOutputChannel } from 'vscode';

/**
 * Categories for ref tracking log events.
 */
export type RefTrackingCategory =
  | 'REF_CHANGE'
  | 'REF_INVALID'
  | 'REF_RECOVER'
  | 'REF_VALIDATION'
  | 'SESSION_UPDATE'
  | 'REFRESH_DECISION'
  | 'UI_UPDATE';

/**
 * Pattern to match full 40-character git commit hashes for truncation.
 */
const FULL_HASH_PATTERN = /^[0-9a-f]{40}$/i;

/**
 * Logger instance for structured events.
 * Set via setStructuredEventsLogger.
 */
let structuredEventsLogger: LogOutputChannel | undefined;

/**
 * Sets the logger to use for structured event logging.
 * If not set, events are silently skipped (graceful degradation).
 *
 * @param logger - Logger instance or undefined to disable
 */
export function setStructuredEventsLogger(logger: LogOutputChannel | undefined): void {
  structuredEventsLogger = logger;
}

/**
 * Truncates git commit hashes to standard 7-character short form.
 * Only truncates values that match the full 40-character SHA-1 pattern.
 *
 * @param value - String value that may be a git hash
 * @returns Truncated value if it's a 40-char hex string, otherwise original value
 */
export function truncateHash(value: string): string {
  return FULL_HASH_PATTERN.test(value) ? value.substring(0, 7) : value;
}

/**
 * Logs a structured ref tracking event for debugging the RefService control flow.
 *
 * Events are logged with structured `[CATEGORY] key=value` format for easy
 * filtering and tracing.
 *
 * **Log Level Strategy**:
 * - `warn`: REF_INVALID (error state that needs attention)
 * - `info`: REF_RECOVER, SESSION_UPDATE, UI_UPDATE (noteworthy state transitions)
 * - `debug`: REF_CHANGE, REF_VALIDATION, REFRESH_DECISION (high-frequency tracing)
 *
 * **Hash Truncation**: Git commit hashes (40-char hex) are automatically truncated
 * to 7 characters for readability.
 *
 * @param category - The event category for filtering
 * @param fields - Key-value pairs to include in the log entry
 *
 * @example
 * ```typescript
 * logRefTrackingEvent('REF_CHANGE', {
 *   ref: 'main',
 *   oldHash: undefined,
 *   newHash: 'abc1234567890...',  // Will be truncated to 'abc1234'
 *   reason: 'initial'
 * });
 * // Output: [REF_CHANGE] ref=main oldHash=undefined newHash=abc1234 reason=initial
 * ```
 */
export function logRefTrackingEvent(
  category: RefTrackingCategory,
  fields: Record<string, string | number | boolean | undefined>
): void {
  // Gracefully skip if no logger is set
  if (!structuredEventsLogger) {
    return;
  }

  // Build the key=value pairs, truncating hash values
  const pairs = Object.entries(fields)
    .map(([key, value]) => {
      if (value === undefined) {
        return `${key}=undefined`;
      }
      const stringValue = String(value);
      const truncatedValue = truncateHash(stringValue);
      return `${key}=${truncatedValue}`;
    })
    .join(' ');

  const message = `[${category}] ${pairs}`;

  // Apply log level strategy per plan assessment
  switch (category) {
    case 'REF_INVALID':
      // Error state - use warn level
      structuredEventsLogger.warn(message);
      break;
    case 'REF_RECOVER':
    case 'SESSION_UPDATE':
    case 'UI_UPDATE':
      // Noteworthy state transitions - use info level
      structuredEventsLogger.info(message);
      break;
    case 'REF_CHANGE':
    case 'REF_VALIDATION':
    case 'REFRESH_DECISION':
      // High-frequency tracing - use debug level
      structuredEventsLogger.debug(message);
      break;
  }
}
