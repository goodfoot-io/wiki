import { describe, expect, it } from 'vitest';
import { LogBuffer } from '../src/logBuffer.js';
import type { PerformanceTimeSource } from '../src/timeSource.js';

/**
 * Fixed performance time source for deterministic testing.
 *
 * @summary Tests log buffer behavior in test
 */
class FixedPerformanceTimeSource implements PerformanceTimeSource {
  private currentMs: number;
  private readonly stepMs: number;

  constructor(startMs: number, stepMs = 0) {
    this.currentMs = startMs;
    this.stepMs = stepMs;
  }

  now(): number {
    const value = this.currentMs;
    this.currentMs += this.stepMs;
    return value;
  }
}

describe('LogBuffer', () => {
  describe('constructor', () => {
    it('should create a buffer with valid capacity', () => {
      const buffer = new LogBuffer(100);
      expect(buffer.getCapacity()).toBe(100);
      expect(buffer.getSize()).toBe(0);
    });

    it('should accept a custom time source', () => {
      const timeSource = new FixedPerformanceTimeSource(1000);
      const buffer = new LogBuffer(10, timeSource);
      expect(buffer.getCapacity()).toBe(10);
    });

    it('should throw TypeError for non-integer capacity', () => {
      expect(() => new LogBuffer(10.5)).toThrow(TypeError);
      expect(() => new LogBuffer(10.5)).toThrow('Capacity must be a positive integer');
    });

    it('should throw TypeError for zero capacity', () => {
      expect(() => new LogBuffer(0)).toThrow(TypeError);
      expect(() => new LogBuffer(0)).toThrow('Capacity must be a positive integer');
    });

    it('should throw TypeError for negative capacity', () => {
      expect(() => new LogBuffer(-5)).toThrow(TypeError);
      expect(() => new LogBuffer(-5)).toThrow('Capacity must be a positive integer');
    });
  });

  describe('append', () => {
    it('should add entries to buffer', () => {
      const timeSource = new FixedPerformanceTimeSource(1000, 100);
      const buffer = new LogBuffer(5, timeSource);

      buffer.append('info', 'First message');
      expect(buffer.getSize()).toBe(1);

      buffer.append('error', 'Second message');
      expect(buffer.getSize()).toBe(2);
    });

    it('should capture timestamp from time source', () => {
      const timeSource = new FixedPerformanceTimeSource(1000, 100);
      const buffer = new LogBuffer(5, timeSource);

      buffer.append('info', 'Message 1');
      buffer.append('warn', 'Message 2');

      const entries = buffer.getEntries();
      expect(entries[0]!.timestamp).toBe(1000);
      expect(entries[1]!.timestamp).toBe(1100);
    });

    it('should store all log entry fields correctly', () => {
      const timeSource = new FixedPerformanceTimeSource(5000);
      const buffer = new LogBuffer(5, timeSource);

      buffer.append('debug', 'Debug message');

      const entries = buffer.getEntries();
      expect(entries[0]).toEqual({
        level: 'debug',
        message: 'Debug message',
        timestamp: 5000
      });
    });

    it('should handle all log levels', () => {
      const timeSource = new FixedPerformanceTimeSource(1000, 100);
      const buffer = new LogBuffer(10, timeSource);

      buffer.append('error', 'Error message');
      buffer.append('warn', 'Warning message');
      buffer.append('info', 'Info message');
      buffer.append('debug', 'Debug message');
      buffer.append('trace', 'Trace message');

      const entries = buffer.getEntries();
      expect(entries[0]!.level).toBe('error');
      expect(entries[1]!.level).toBe('warn');
      expect(entries[2]!.level).toBe('info');
      expect(entries[3]!.level).toBe('debug');
      expect(entries[4]!.level).toBe('trace');
    });
  });

  describe('ring buffer behavior', () => {
    it('should not exceed capacity', () => {
      const timeSource = new FixedPerformanceTimeSource(1000, 100);
      const buffer = new LogBuffer(3, timeSource);

      buffer.append('info', 'Message 1');
      buffer.append('info', 'Message 2');
      buffer.append('info', 'Message 3');
      expect(buffer.getSize()).toBe(3);

      buffer.append('info', 'Message 4');
      expect(buffer.getSize()).toBe(3);
    });

    it('should wrap around when capacity is reached', () => {
      const timeSource = new FixedPerformanceTimeSource(1000, 100);
      const buffer = new LogBuffer(3, timeSource);

      buffer.append('info', 'Message 1');
      buffer.append('info', 'Message 2');
      buffer.append('info', 'Message 3');
      buffer.append('info', 'Message 4');

      const entries = buffer.getEntries();
      expect(entries.length).toBe(3);
      expect(entries[0]!.message).toBe('Message 2');
      expect(entries[1]!.message).toBe('Message 3');
      expect(entries[2]!.message).toBe('Message 4');
    });

    it('should maintain chronological order after wraparound', () => {
      const timeSource = new FixedPerformanceTimeSource(1000, 100);
      const buffer = new LogBuffer(3, timeSource);

      buffer.append('info', 'Message 1');
      buffer.append('info', 'Message 2');
      buffer.append('info', 'Message 3');
      buffer.append('info', 'Message 4');
      buffer.append('info', 'Message 5');

      const entries = buffer.getEntries();
      expect(entries.length).toBe(3);
      expect(entries[0]!.message).toBe('Message 3');
      expect(entries[0]!.timestamp).toBe(1200);
      expect(entries[1]!.message).toBe('Message 4');
      expect(entries[1]!.timestamp).toBe(1300);
      expect(entries[2]!.message).toBe('Message 5');
      expect(entries[2]!.timestamp).toBe(1400);
    });

    it('should handle multiple wraparounds correctly', () => {
      const timeSource = new FixedPerformanceTimeSource(1000, 100);
      const buffer = new LogBuffer(2, timeSource);

      for (let i = 1; i <= 7; i++) {
        buffer.append('info', `Message ${i}`);
      }

      const entries = buffer.getEntries();
      expect(entries.length).toBe(2);
      expect(entries[0]!.message).toBe('Message 6');
      expect(entries[1]!.message).toBe('Message 7');
    });
  });

  describe('getEntries', () => {
    it('should return empty array for new buffer', () => {
      const buffer = new LogBuffer(5);
      const entries = buffer.getEntries();
      expect(entries).toEqual([]);
    });

    it('should return entries in chronological order', () => {
      const timeSource = new FixedPerformanceTimeSource(1000, 100);
      const buffer = new LogBuffer(5, timeSource);

      buffer.append('info', 'First');
      buffer.append('info', 'Second');
      buffer.append('info', 'Third');

      const entries = buffer.getEntries();
      expect(entries.length).toBe(3);
      expect(entries[0]!.message).toBe('First');
      expect(entries[1]!.message).toBe('Second');
      expect(entries[2]!.message).toBe('Third');
    });

    it('should return a copy of entries (readonly)', () => {
      const timeSource = new FixedPerformanceTimeSource(1000);
      const buffer = new LogBuffer(5, timeSource);

      buffer.append('info', 'Message');
      const entries1 = buffer.getEntries();
      const entries2 = buffer.getEntries();

      expect(entries1).toEqual(entries2);
      expect(entries1).not.toBe(entries2); // Different array instances
    });

    it('should return entries in order after wraparound', () => {
      const timeSource = new FixedPerformanceTimeSource(1000, 100);
      const buffer = new LogBuffer(3, timeSource);

      buffer.append('info', 'A');
      buffer.append('info', 'B');
      buffer.append('info', 'C');
      buffer.append('info', 'D');
      buffer.append('info', 'E');

      const entries = buffer.getEntries();
      expect(entries.map((e) => e.message)).toEqual(['C', 'D', 'E']);
      // Verify chronological order by timestamp
      expect(entries[0]!.timestamp).toBe(1200);
      expect(entries[1]!.timestamp).toBe(1300);
      expect(entries[2]!.timestamp).toBe(1400);
    });
  });

  describe('getRecentLogs', () => {
    it('should return "(no logs available)" for empty buffer', () => {
      const buffer = new LogBuffer(5);
      expect(buffer.getRecentLogs()).toBe('(no logs available)');
    });

    it('should format log entries with level and message', () => {
      const timeSource = new FixedPerformanceTimeSource(1000);
      const buffer = new LogBuffer(5, timeSource);

      buffer.append('info', 'Info message');
      buffer.append('error', 'Error message');

      const result = buffer.getRecentLogs();
      expect(result).toBe('[INFO] Info message\n[ERROR] Error message');
    });

    it('should uppercase log levels', () => {
      const timeSource = new FixedPerformanceTimeSource(1000);
      const buffer = new LogBuffer(5, timeSource);

      buffer.append('debug', 'Debug message');
      buffer.append('trace', 'Trace message');

      const result = buffer.getRecentLogs();
      expect(result).toContain('[DEBUG]');
      expect(result).toContain('[TRACE]');
    });

    it('should join entries with newlines', () => {
      const timeSource = new FixedPerformanceTimeSource(1000);
      const buffer = new LogBuffer(5, timeSource);

      buffer.append('info', 'First');
      buffer.append('warn', 'Second');
      buffer.append('error', 'Third');

      const result = buffer.getRecentLogs();
      const lines = result.split('\n');
      expect(lines.length).toBe(3);
      expect(lines[0]).toBe('[INFO] First');
      expect(lines[1]).toBe('[WARN] Second');
      expect(lines[2]).toBe('[ERROR] Third');
    });

    it('should show entries in chronological order after wraparound', () => {
      const timeSource = new FixedPerformanceTimeSource(1000);
      const buffer = new LogBuffer(2, timeSource);

      buffer.append('info', 'First');
      buffer.append('warn', 'Second');
      buffer.append('error', 'Third');

      const result = buffer.getRecentLogs();
      const lines = result.split('\n');
      expect(lines.length).toBe(2);
      expect(lines[0]).toBe('[WARN] Second');
      expect(lines[1]).toBe('[ERROR] Third');
    });
  });

  describe('clear', () => {
    it('should reset buffer to empty state', () => {
      const timeSource = new FixedPerformanceTimeSource(1000);
      const buffer = new LogBuffer(5, timeSource);

      buffer.append('info', 'Message 1');
      buffer.append('info', 'Message 2');
      expect(buffer.getSize()).toBe(2);

      buffer.clear();

      expect(buffer.getSize()).toBe(0);
      expect(buffer.getEntries()).toEqual([]);
      expect(buffer.getRecentLogs()).toBe('(no logs available)');
    });

    it('should allow new entries after clear', () => {
      const timeSource = new FixedPerformanceTimeSource(1000, 100);
      const buffer = new LogBuffer(3, timeSource);

      buffer.append('info', 'Before clear 1');
      buffer.append('info', 'Before clear 2');
      buffer.clear();

      buffer.append('info', 'After clear 1');
      buffer.append('info', 'After clear 2');

      const entries = buffer.getEntries();
      expect(entries.length).toBe(2);
      expect(entries[0]!.message).toBe('After clear 1');
      expect(entries[1]!.message).toBe('After clear 2');
    });

    it('should reset write index correctly', () => {
      const timeSource = new FixedPerformanceTimeSource(1000, 100);
      const buffer = new LogBuffer(3, timeSource);

      // Fill buffer and wrap around
      buffer.append('info', 'A');
      buffer.append('info', 'B');
      buffer.append('info', 'C');
      buffer.append('info', 'D');

      buffer.clear();

      // Add new entries
      buffer.append('info', 'X');
      buffer.append('info', 'Y');

      const entries = buffer.getEntries();
      expect(entries.length).toBe(2);
      expect(entries[0]!.message).toBe('X');
      expect(entries[1]!.message).toBe('Y');
    });
  });

  describe('getSize', () => {
    it('should return 0 for new buffer', () => {
      const buffer = new LogBuffer(10);
      expect(buffer.getSize()).toBe(0);
    });

    it('should increment as entries are added', () => {
      const buffer = new LogBuffer(10);
      expect(buffer.getSize()).toBe(0);

      buffer.append('info', 'Message 1');
      expect(buffer.getSize()).toBe(1);

      buffer.append('info', 'Message 2');
      expect(buffer.getSize()).toBe(2);
    });

    it('should not exceed capacity', () => {
      const buffer = new LogBuffer(3);

      buffer.append('info', 'Message 1');
      buffer.append('info', 'Message 2');
      buffer.append('info', 'Message 3');
      expect(buffer.getSize()).toBe(3);

      buffer.append('info', 'Message 4');
      expect(buffer.getSize()).toBe(3);

      buffer.append('info', 'Message 5');
      expect(buffer.getSize()).toBe(3);
    });

    it('should return 0 after clear', () => {
      const buffer = new LogBuffer(5);

      buffer.append('info', 'Message 1');
      buffer.append('info', 'Message 2');
      expect(buffer.getSize()).toBe(2);

      buffer.clear();
      expect(buffer.getSize()).toBe(0);
    });
  });

  describe('getCapacity', () => {
    it('should return the configured capacity', () => {
      expect(new LogBuffer(5).getCapacity()).toBe(5);
      expect(new LogBuffer(100).getCapacity()).toBe(100);
      expect(new LogBuffer(1).getCapacity()).toBe(1);
    });

    it('should remain constant throughout buffer lifetime', () => {
      const buffer = new LogBuffer(3);
      expect(buffer.getCapacity()).toBe(3);

      buffer.append('info', 'Message');
      expect(buffer.getCapacity()).toBe(3);

      buffer.clear();
      expect(buffer.getCapacity()).toBe(3);
    });
  });
});
