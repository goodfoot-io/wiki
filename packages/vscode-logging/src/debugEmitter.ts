import { EventEmitter } from 'node:events';

/**
 * Log levels supported by the debug emitter.
 *
 * @summary Debug Emitter logic for src
 */
export type LogLevel = 'error' | 'warn' | 'info' | 'debug' | 'trace';

/**
 * Listener type for log events.
 * First argument is the message (string for most levels, string | Error for error level).
 * Remaining arguments are the variadic args passed to the log method.
 */
export type LogEventListener = (message: string | Error, ...args: unknown[]) => void;

/**
 * Typed EventEmitter for intercepting logger output during tests.
 *
 * When listeners are attached to a log level, log messages at that level
 * are emitted to listeners instead of being written to the VSCode OutputChannel.
 * The log buffer always captures entries regardless of listener state.
 *
 * @example
 * // Intercept warnings during a test
 * const warnings: string[] = [];
 * loggerDebugEmitter.on('warn', (msg) => warnings.push(String(msg)));
 *
 * // Run code that logs warnings
 * someOperation();
 *
 * // Verify warnings were logged
 * assert.strictEqual(warnings.length, 1);
 *
 * // Clean up after test
 * loggerDebugEmitter.removeAllListeners();
 */
export class LoggerDebugEmitter extends EventEmitter {
  override emit(event: LogLevel, message: string | Error, ...args: unknown[]): boolean;
  override emit(event: string | symbol, ...args: unknown[]): boolean {
    return super.emit(event, ...args);
  }

  override on(event: LogLevel, listener: LogEventListener): this;
  override on(event: string | symbol, listener: LogEventListener | ((...args: unknown[]) => void)): this {
    return super.on(event, listener);
  }

  override addListener(event: LogLevel, listener: LogEventListener): this;
  override addListener(event: string | symbol, listener: LogEventListener | ((...args: unknown[]) => void)): this {
    return super.addListener(event, listener);
  }

  override once(event: LogLevel, listener: LogEventListener): this;
  override once(event: string | symbol, listener: LogEventListener | ((...args: unknown[]) => void)): this {
    return super.once(event, listener);
  }

  override off(event: LogLevel, listener: LogEventListener): this;
  override off(event: string | symbol, listener: LogEventListener | ((...args: unknown[]) => void)): this {
    return super.off(event, listener);
  }

  override removeListener(event: LogLevel, listener: LogEventListener): this;
  override removeListener(event: string | symbol, listener: LogEventListener | ((...args: unknown[]) => void)): this {
    return super.removeListener(event, listener);
  }
}
