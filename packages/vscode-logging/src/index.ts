/**
 * Structured logging for VSCode extensions covering logger creation and
 * initialization, in-memory log buffering, telemetry error reporting,
 * structured ref-tracking events, and pluggable time sources for
 * deterministic testing.
 *
 * @summary Provide structured logging, telemetry integration, and time-source abstractions for VSCode extensions
 */

// ============================================================================
// Logger Factory and Types
// ============================================================================

export {
  createLogger,
  type DecoratedLogger,
  type ExtensionContextLike,
  getLogBuffer,
  initializeLoggers,
  type LoggerOptions
} from './logger.js';

// ============================================================================
// Log Buffer and Entries
// ============================================================================

export { LogBuffer, type LogEntry } from './logBuffer.js';

// ============================================================================
// Debug Emitter for Test Interception
// ============================================================================

export { type LogEventListener, LoggerDebugEmitter } from './debugEmitter.js';

// ============================================================================
// Telemetry Integration
// ============================================================================

export {
  type ErrorCategory,
  type ErrorContext,
  formatError,
  logErrorWithTelemetry,
  sanitizeErrorMessage,
  setTelemetryLogger,
  setTelemetryService,
  type TelemetryService
} from './telemetry.js';

// ============================================================================
// Structured Event Logging
// ============================================================================

export {
  logRefTrackingEvent,
  type RefTrackingCategory,
  setStructuredEventsLogger,
  truncateHash
} from './structuredEvents.js';

// ============================================================================
// Time Source Types and Implementations
// ============================================================================

export {
  FixedTimeSource,
  type PerformanceTimeSource,
  systemPerformanceTimeSource,
  systemTimeSource,
  type TimeSource
} from './timeSource.js';
