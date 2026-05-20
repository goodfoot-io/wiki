import { beforeEach, describe, expect, it } from 'vitest';
import { type LogEventListener, LoggerDebugEmitter, type LogLevel } from '../src/debugEmitter.js';

/**
 * Exercises debug emitter behavior in the test area through focused scenarios.
 * The cases lock in edge handling and regression coverage so refactors preserve expected state
 * transitions and output.
 *
 * @summary Tests debug emitter behavior in test
 */

describe('LoggerDebugEmitter', () => {
  let emitter: LoggerDebugEmitter;

  beforeEach(() => {
    emitter = new LoggerDebugEmitter();
  });

  describe('on/emit pattern', () => {
    it('should emit and receive error level messages with string', () => {
      const messages: unknown[] = [];
      const listener: LogEventListener = (message, ...args) => {
        messages.push({ message, args });
      };

      emitter.on('error', listener);
      emitter.emit('error', 'Error message', 'arg1', 'arg2');

      expect(messages).toEqual([{ message: 'Error message', args: ['arg1', 'arg2'] }]);
    });

    it('should emit and receive error level messages with Error object', () => {
      const messages: unknown[] = [];
      const listener: LogEventListener = (message, ...args) => {
        messages.push({ message, args });
      };

      const error = new Error('Test error');
      emitter.on('error', listener);
      emitter.emit('error', error, 'context');

      expect(messages).toEqual([{ message: error, args: ['context'] }]);
    });

    it('should emit and receive warn level messages', () => {
      const warnings: string[] = [];

      emitter.on('warn', (message) => {
        warnings.push(String(message));
      });

      emitter.emit('warn', 'Warning 1');
      emitter.emit('warn', 'Warning 2');

      expect(warnings).toEqual(['Warning 1', 'Warning 2']);
    });

    it('should emit and receive info level messages', () => {
      const infos: string[] = [];

      emitter.on('info', (message) => {
        infos.push(String(message));
      });

      emitter.emit('info', 'Info message');

      expect(infos).toEqual(['Info message']);
    });

    it('should emit and receive debug level messages', () => {
      const debugs: string[] = [];

      emitter.on('debug', (message) => {
        debugs.push(String(message));
      });

      emitter.emit('debug', 'Debug message');

      expect(debugs).toEqual(['Debug message']);
    });

    it('should emit and receive trace level messages', () => {
      const traces: string[] = [];

      emitter.on('trace', (message) => {
        traces.push(String(message));
      });

      emitter.emit('trace', 'Trace message');

      expect(traces).toEqual(['Trace message']);
    });

    it('should pass variadic arguments to listeners', () => {
      const captured: unknown[][] = [];

      emitter.on('info', (message, ...args) => {
        captured.push([message, ...args]);
      });

      emitter.emit('info', 'Message', 'arg1', 42, { key: 'value' });

      expect(captured).toEqual([['Message', 'arg1', 42, { key: 'value' }]]);
    });
  });

  describe('once pattern', () => {
    it('should only receive one message when using once', () => {
      const messages: string[] = [];

      emitter.once('warn', (message) => {
        messages.push(String(message));
      });

      emitter.emit('warn', 'First warning');
      emitter.emit('warn', 'Second warning');

      expect(messages).toEqual(['First warning']);
    });
  });

  describe('off/removeListener pattern', () => {
    it('should stop receiving messages after off', () => {
      const messages: string[] = [];
      const listener: LogEventListener = (message) => {
        messages.push(String(message));
      };

      emitter.on('info', listener);
      emitter.emit('info', 'First');

      emitter.off('info', listener);
      emitter.emit('info', 'Second');

      expect(messages).toEqual(['First']);
    });

    it('should stop receiving messages after removeListener', () => {
      const messages: string[] = [];
      const listener: LogEventListener = (message) => {
        messages.push(String(message));
      };

      emitter.on('debug', listener);
      emitter.emit('debug', 'First');

      emitter.removeListener('debug', listener);
      emitter.emit('debug', 'Second');

      expect(messages).toEqual(['First']);
    });
  });

  describe('listenerCount', () => {
    it('should return 0 when no listeners attached', () => {
      expect(emitter.listenerCount('error')).toBe(0);
    });

    it('should return correct count after adding listeners', () => {
      const listener1: LogEventListener = () => {};
      const listener2: LogEventListener = () => {};

      emitter.on('warn', listener1);
      expect(emitter.listenerCount('warn')).toBe(1);

      emitter.on('warn', listener2);
      expect(emitter.listenerCount('warn')).toBe(2);
    });

    it('should return correct count after removing listeners', () => {
      const listener: LogEventListener = () => {};

      emitter.on('info', listener);
      expect(emitter.listenerCount('info')).toBe(1);

      emitter.off('info', listener);
      expect(emitter.listenerCount('info')).toBe(0);
    });

    it('should track different log levels independently', () => {
      emitter.on('error', () => {});
      emitter.on('warn', () => {});
      emitter.on('warn', () => {});

      expect(emitter.listenerCount('error')).toBe(1);
      expect(emitter.listenerCount('warn')).toBe(2);
      expect(emitter.listenerCount('info')).toBe(0);
    });
  });

  describe('removeAllListeners', () => {
    it('should remove all listeners for a specific event', () => {
      emitter.on('warn', () => {});
      emitter.on('warn', () => {});
      emitter.on('error', () => {});

      emitter.removeAllListeners('warn');

      expect(emitter.listenerCount('warn')).toBe(0);
      expect(emitter.listenerCount('error')).toBe(1);
    });

    it('should remove all listeners for all events when no event specified', () => {
      emitter.on('error', () => {});
      emitter.on('warn', () => {});
      emitter.on('info', () => {});

      emitter.removeAllListeners();

      expect(emitter.listenerCount('error')).toBe(0);
      expect(emitter.listenerCount('warn')).toBe(0);
      expect(emitter.listenerCount('info')).toBe(0);
    });
  });

  describe('addListener', () => {
    it('should work as alias for on', () => {
      const messages: string[] = [];
      const listener: LogEventListener = (message) => {
        messages.push(String(message));
      };

      emitter.addListener('trace', listener);
      emitter.emit('trace', 'Trace message');

      expect(messages).toEqual(['Trace message']);
    });
  });

  describe('type safety', () => {
    it('should accept valid LogLevel values', () => {
      const levels: LogLevel[] = ['error', 'warn', 'info', 'debug', 'trace'];

      levels.forEach((level) => {
        const listener: LogEventListener = () => {};
        emitter.on(level, listener);
        expect(emitter.listenerCount(level)).toBe(1);
        emitter.removeAllListeners(level);
      });
    });
  });
});
