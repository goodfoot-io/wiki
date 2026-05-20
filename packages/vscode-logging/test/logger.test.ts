/**
 * Tests for the logger factory and child logger support.
 *
 * @summary Tests for the logger factory and child logger support
 */
import { describe, expect, it } from 'vitest';
import { LogBuffer } from '../src/logBuffer.js';
import { createLogger, type ExtensionContextLike, getLogBuffer, initializeLoggers } from '../src/logger.js';

/**
 * Helper to get first entry from buffer safely.
 *
 * @param buffer - Log buffer to read from
 * @returns First log entry in the buffer
 * @throws {Error} When the buffer has no entries
 */
function getFirstEntry(buffer: LogBuffer) {
  const entries = buffer.getEntries();
  const entry = entries[0];
  if (!entry) throw new Error('Expected at least one entry');
  return entry;
}

/**
 * Helper to get entry at index from buffer safely.
 *
 * @param buffer - Log buffer to read from
 * @param index - Zero-based entry index to retrieve
 * @returns Log entry at the requested index
 * @throws {Error} When the index is outside the buffered entries
 */
function getEntryAt(buffer: LogBuffer, index: number) {
  const entries = buffer.getEntries();
  const entry = entries[index];
  if (!entry) throw new Error(`Expected entry at index ${index}`);
  return entry;
}

describe('createLogger', () => {
  it('should create a decorated LogOutputChannel', () => {
    const logger = createLogger('Test Logger');

    expect(logger).toBeDefined();
    expect(logger.name).toBe('Test Logger');
    expect(typeof logger.error).toBe('function');
    expect(typeof logger.warn).toBe('function');
    expect(typeof logger.info).toBe('function');
    expect(typeof logger.debug).toBe('function');
    expect(typeof logger.trace).toBe('function');
  });

  it('should have getChildLogger method', () => {
    const logger = createLogger('Test Logger');

    expect(typeof logger.getChildLogger).toBe('function');
  });
});

describe('decorateLogger feeds LogBuffer', () => {
  it('should append error messages to log buffer', () => {
    const logger = createLogger('Test Logger');
    const buffer = getLogBuffer();
    buffer.clear();

    logger.error('test error message');

    const entries = buffer.getEntries();
    expect(entries.length).toBe(1);
    const entry = getFirstEntry(buffer);
    expect(entry.level).toBe('error');
    expect(entry.message).toBe('test error message');
  });

  it('should append Error objects to log buffer using message', () => {
    const logger = createLogger('Test Logger');
    const buffer = getLogBuffer();
    buffer.clear();

    logger.error(new Error('error object message'));

    const entries = buffer.getEntries();
    expect(entries.length).toBe(1);
    const entry = getFirstEntry(buffer);
    expect(entry.level).toBe('error');
    expect(entry.message).toBe('error object message');
  });

  it('should append warn messages to log buffer', () => {
    const logger = createLogger('Test Logger');
    const buffer = getLogBuffer();
    buffer.clear();

    logger.warn('test warning');

    const entries = buffer.getEntries();
    expect(entries.length).toBe(1);
    const entry = getFirstEntry(buffer);
    expect(entry.level).toBe('warn');
    expect(entry.message).toBe('test warning');
  });

  it('should append info messages to log buffer', () => {
    const logger = createLogger('Test Logger');
    const buffer = getLogBuffer();
    buffer.clear();

    logger.info('test info');

    const entries = buffer.getEntries();
    expect(entries.length).toBe(1);
    const entry = getFirstEntry(buffer);
    expect(entry.level).toBe('info');
    expect(entry.message).toBe('test info');
  });

  it('should append debug messages to log buffer', () => {
    const logger = createLogger('Test Logger');
    const buffer = getLogBuffer();
    buffer.clear();

    logger.debug('test debug');

    const entries = buffer.getEntries();
    expect(entries.length).toBe(1);
    const entry = getFirstEntry(buffer);
    expect(entry.level).toBe('debug');
    expect(entry.message).toBe('test debug');
  });

  it('should append trace messages to log buffer', () => {
    const logger = createLogger('Test Logger');
    const buffer = getLogBuffer();
    buffer.clear();

    logger.trace('test trace');

    const entries = buffer.getEntries();
    expect(entries.length).toBe(1);
    const entry = getFirstEntry(buffer);
    expect(entry.level).toBe('trace');
    expect(entry.message).toBe('test trace');
  });
});

describe('debug emitter interception', () => {
  it('should emit to debug emitter when listener attached for error', () => {
    const logger = createLogger('Test Logger');
    const errors: Array<{ message: string | Error; args: unknown[] }> = [];

    logger.debugEmitter.on('error', (message: string | Error, ...args: unknown[]) => {
      errors.push({ message, args });
    });

    logger.error('test error', 'arg1', 'arg2');

    expect(errors.length).toBe(1);
    const first = errors[0];
    expect(first).toBeDefined();
    expect(first?.message).toBe('test error');
    expect(first?.args).toEqual(['arg1', 'arg2']);

    logger.debugEmitter.removeAllListeners();
  });

  it('should emit to debug emitter when listener attached for warn', () => {
    const logger = createLogger('Test Logger');
    const warnings: Array<{ message: string; args: unknown[] }> = [];

    logger.debugEmitter.on('warn', (message: string | Error, ...args: unknown[]) => {
      warnings.push({ message: String(message), args });
    });

    logger.warn('test warning', 'extra');

    expect(warnings.length).toBe(1);
    const first = warnings[0];
    expect(first).toBeDefined();
    expect(first?.message).toBe('test warning');
    expect(first?.args).toEqual(['extra']);

    logger.debugEmitter.removeAllListeners();
  });

  it('should emit to debug emitter when listener attached for info', () => {
    const logger = createLogger('Test Logger');
    const infos: string[] = [];

    logger.debugEmitter.on('info', (message: string | Error) => {
      infos.push(String(message));
    });

    logger.info('test info');

    expect(infos.length).toBe(1);
    expect(infos[0]).toBe('test info');

    logger.debugEmitter.removeAllListeners();
  });

  it('should emit to debug emitter when listener attached for debug', () => {
    const logger = createLogger('Test Logger');
    const debugs: string[] = [];

    logger.debugEmitter.on('debug', (message: string | Error) => {
      debugs.push(String(message));
    });

    logger.debug('test debug');

    expect(debugs.length).toBe(1);
    expect(debugs[0]).toBe('test debug');

    logger.debugEmitter.removeAllListeners();
  });

  it('should emit to debug emitter when listener attached for trace', () => {
    const logger = createLogger('Test Logger');
    const traces: string[] = [];

    logger.debugEmitter.on('trace', (message: string | Error) => {
      traces.push(String(message));
    });

    logger.trace('test trace');

    expect(traces.length).toBe(1);
    expect(traces[0]).toBe('test trace');

    logger.debugEmitter.removeAllListeners();
  });

  it('should still feed buffer even when emitter has listeners', () => {
    const logger = createLogger('Test Logger');
    const buffer = getLogBuffer();
    buffer.clear();

    logger.debugEmitter.on('error', () => {});

    logger.error('test error');

    const entries = buffer.getEntries();
    expect(entries.length).toBe(1);
    const entry = getFirstEntry(buffer);
    expect(entry.message).toBe('test error');

    logger.debugEmitter.removeAllListeners();
  });
});

describe('child logger', () => {
  it('should create child logger with label', () => {
    const parent = createLogger('Parent');
    const child = parent.getChildLogger({ label: 'Child' });

    expect(child).toBeDefined();
    expect(typeof child.error).toBe('function');
    expect(typeof child.getChildLogger).toBe('function');
  });

  it('should cache child loggers by label', () => {
    const parent = createLogger('Parent');
    const child1 = parent.getChildLogger({ label: 'Child' });
    const child2 = parent.getChildLogger({ label: 'Child' });

    expect(child1).toBe(child2);
  });

  it('should create separate child loggers for different labels', () => {
    const parent = createLogger('Parent');
    const child1 = parent.getChildLogger({ label: 'Child1' });
    const child2 = parent.getChildLogger({ label: 'Child2' });

    expect(child1).not.toBe(child2);
  });

  it('should create hierarchical labels with dot separator', () => {
    const parent = createLogger('Cards');
    const child = parent.getChildLogger({ label: 'Git' });
    const grandchild = child.getChildLogger({ label: 'Branch' });

    // Verify hierarchy through message prefixing
    const buffer = getLogBuffer();
    buffer.clear();

    grandchild.info('test message');

    const entries = buffer.getEntries();
    expect(entries.length).toBe(1);
    const entry = getFirstEntry(buffer);
    expect(entry.message).toBe('[Cards.Git.Branch] test message');
  });

  it('should prefix child log messages with full label path', () => {
    const parent = createLogger('Cards');
    const child = parent.getChildLogger({ label: 'Git' });
    const buffer = getLogBuffer();
    buffer.clear();

    child.info('fetching refs');

    const entries = buffer.getEntries();
    expect(entries.length).toBe(1);
    const entry = getFirstEntry(buffer);
    expect(entry.message).toBe('[Cards.Git] fetching refs');
  });

  it('should share parent buffer with child loggers', () => {
    const parent = createLogger('Parent');
    const child = parent.getChildLogger({ label: 'Child' });
    const buffer = getLogBuffer();
    buffer.clear();

    parent.info('parent message');
    child.info('child message');

    const entries = buffer.getEntries();
    expect(entries.length).toBe(2);
    const first = getEntryAt(buffer, 0);
    const second = getEntryAt(buffer, 1);
    expect(first.message).toBe('parent message');
    expect(second.message).toBe('[Parent.Child] child message');
  });

  it('should share parent debug emitter with child loggers', () => {
    const parent = createLogger('Parent');
    const child = parent.getChildLogger({ label: 'Child' });
    const messages: string[] = [];

    parent.debugEmitter.on('info', (message: string | Error) => {
      messages.push(String(message));
    });

    parent.info('parent message');
    child.info('child message');

    expect(messages.length).toBe(2);
    expect(messages[0]).toBe('parent message');
    expect(messages[1]).toBe('[Parent.Child] child message');

    parent.debugEmitter.removeAllListeners();
  });

  it('should handle child logger error with Error objects', () => {
    const parent = createLogger('Parent');
    const child = parent.getChildLogger({ label: 'Child' });
    const buffer = getLogBuffer();
    buffer.clear();

    child.error(new Error('child error'));

    const entries = buffer.getEntries();
    expect(entries.length).toBe(1);
    const entry = getFirstEntry(buffer);
    expect(entry.level).toBe('error');
    expect(entry.message).toBe('[Parent.Child] child error');
  });
});

describe('getLogBuffer', () => {
  it('should return a LogBuffer instance', () => {
    const buffer = getLogBuffer();
    expect(buffer).toBeInstanceOf(LogBuffer);
  });

  it('should return the same singleton instance', () => {
    const buffer1 = getLogBuffer();
    const buffer2 = getLogBuffer();
    expect(buffer1).toBe(buffer2);
  });
});

describe('initializeLoggers', () => {
  it('should register logger for disposal', () => {
    const logger = createLogger('Test Logger');
    const subscriptions: Array<{ dispose(): void }> = [];
    const context: ExtensionContextLike = {
      subscriptions: {
        push(disposable: { dispose(): void }) {
          subscriptions.push(disposable);
        }
      }
    };

    initializeLoggers(context, logger);

    expect(subscriptions.length).toBe(1);
    expect(subscriptions[0]).toBe(logger);
  });

  it('should register multiple loggers for disposal', () => {
    const logger1 = createLogger('Logger 1');
    const logger2 = createLogger('Logger 2');
    const subscriptions: Array<{ dispose(): void }> = [];
    const context: ExtensionContextLike = {
      subscriptions: {
        push(disposable: { dispose(): void }) {
          subscriptions.push(disposable);
        }
      }
    };

    initializeLoggers(context, logger1, logger2);

    expect(subscriptions.length).toBe(2);
    expect(subscriptions[0]).toBe(logger1);
    expect(subscriptions[1]).toBe(logger2);
  });
});

describe('printf-style format substitution', () => {
  it('should substitute %s in info messages', () => {
    const logger = createLogger('Test Logger');
    const buffer = getLogBuffer();
    buffer.clear();

    logger.info('Loaded from: %s', 'project');

    const entry = getFirstEntry(buffer);
    expect(entry.message).toBe('Loaded from: project');
  });

  it('should substitute %d in info messages', () => {
    const logger = createLogger('Test Logger');
    const buffer = getLogBuffer();
    buffer.clear();

    logger.info('Port: %d', 3000);

    const entry = getFirstEntry(buffer);
    expect(entry.message).toBe('Port: 3000');
  });

  it('should substitute multiple placeholders', () => {
    const logger = createLogger('Test Logger');
    const buffer = getLogBuffer();
    buffer.clear();

    logger.info('Server on %s:%d', 'localhost', 8080);

    const entry = getFirstEntry(buffer);
    expect(entry.message).toBe('Server on localhost:8080');
  });

  it('should substitute %s in child logger messages', () => {
    const parent = createLogger('Cards');
    const child = parent.getChildLogger({ label: 'Runtime' });
    const buffer = getLogBuffer();
    buffer.clear();

    child.info('[SettingsLoader] Loaded settings from: %s', 'project');

    const entry = getFirstEntry(buffer);
    expect(entry.message).toBe('[Cards.Runtime] [SettingsLoader] Loaded settings from: project');
  });

  it('should substitute %s in warn messages', () => {
    const logger = createLogger('Test Logger');
    const buffer = getLogBuffer();
    buffer.clear();

    logger.warn('File not found: %s', '/tmp/settings.json');

    const entry = getFirstEntry(buffer);
    expect(entry.message).toBe('File not found: /tmp/settings.json');
  });

  it('should substitute %s in error messages', () => {
    const logger = createLogger('Test Logger');
    const buffer = getLogBuffer();
    buffer.clear();

    logger.error('Failed: %s', 'connection refused');

    const entry = getFirstEntry(buffer);
    expect(entry.message).toBe('Failed: connection refused');
  });

  it('should leave message unchanged when no args provided', () => {
    const logger = createLogger('Test Logger');
    const buffer = getLogBuffer();
    buffer.clear();

    logger.info('No substitution needed');

    const entry = getFirstEntry(buffer);
    expect(entry.message).toBe('No substitution needed');
  });
});

describe('source location tracking', () => {
  it('should include source location when enabled', () => {
    const logger = createLogger('Test Logger', { sourceLocationTracking: true });
    const buffer = getLogBuffer();
    buffer.clear();

    logger.info('test message');

    const entries = buffer.getEntries();
    expect(entries.length).toBe(1);
    const entry = getFirstEntry(buffer);
    // Source location format: [file:line:col] message
    expect(entry.message).toMatch(/^\[.+:\d+:\d+\] test message$/);
  });

  it('should not include source location when disabled', () => {
    const logger = createLogger('Test Logger', { sourceLocationTracking: false });
    const buffer = getLogBuffer();
    buffer.clear();

    logger.info('test message');

    const entries = buffer.getEntries();
    expect(entries.length).toBe(1);
    const entry = getFirstEntry(buffer);
    expect(entry.message).toBe('test message');
  });

  it('should not include source location by default', () => {
    const logger = createLogger('Test Logger');
    const buffer = getLogBuffer();
    buffer.clear();

    logger.info('test message');

    const entries = buffer.getEntries();
    expect(entries.length).toBe(1);
    const entry = getFirstEntry(buffer);
    expect(entry.message).toBe('test message');
  });

  it('should include source location in child logger messages when enabled', () => {
    const parent = createLogger('Parent', { sourceLocationTracking: true });
    const child = parent.getChildLogger({ label: 'Child' });
    const buffer = getLogBuffer();
    buffer.clear();

    child.info('child message');

    const entries = buffer.getEntries();
    expect(entries.length).toBe(1);
    const entry = getFirstEntry(buffer);
    // Format: [file:line:col] [Parent.Child] child message
    expect(entry.message).toMatch(/^\[.+:\d+:\d+\] \[Parent\.Child\] child message$/);
  });
});
