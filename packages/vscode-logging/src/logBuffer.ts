/**
 * Log buffer service for capturing recent log entries.
 *
 * Implements a fixed-size ring buffer that captures the last N log entries
 * from the extension's output channels. Used for generating error reports
 * with recent diagnostic context.
 *
 * @summary Log buffer service for capturing recent log entries
 */

import { type PerformanceTimeSource, systemPerformanceTimeSource } from './timeSource.js';

export type LogLevel = 'error' | 'warn' | 'info' | 'debug' | 'trace';

export interface LogEntry {
  level: LogLevel;
  message: string;
  timestamp: number;
}

/**
 * Ring buffer for storing recent log entries.
 */
export class LogBuffer {
  private readonly entries: LogEntry[] = [];
  private readonly capacity: number;
  private writeIndex = 0;
  private size = 0;
  private readonly timeSource: PerformanceTimeSource;

  constructor(capacity: number, timeSource: PerformanceTimeSource = systemPerformanceTimeSource) {
    if (!Number.isInteger(capacity) || capacity <= 0) {
      throw new TypeError('Capacity must be a positive integer');
    }
    this.capacity = capacity;
    this.timeSource = timeSource;
  }

  append(level: LogLevel, message: string): void {
    const entry: LogEntry = {
      level,
      message,
      timestamp: this.timeSource.now()
    };

    if (this.size < this.capacity) {
      this.entries.push(entry);
      this.size++;
    } else {
      this.entries[this.writeIndex] = entry;
    }

    this.writeIndex = (this.writeIndex + 1) % this.capacity;
  }

  getEntries(): readonly LogEntry[] {
    if (this.size < this.capacity) {
      return [...this.entries];
    } else {
      const oldEntries = this.entries.slice(this.writeIndex);
      const newEntries = this.entries.slice(0, this.writeIndex);
      return [...oldEntries, ...newEntries];
    }
  }

  getRecentLogs(): string {
    const entries = this.getEntries();
    if (entries.length === 0) {
      return '(no logs available)';
    }

    return entries.map((entry) => `[${entry.level.toUpperCase()}] ${entry.message}`).join('\n');
  }

  clear(): void {
    this.entries.length = 0;
    this.writeIndex = 0;
    this.size = 0;
  }

  getSize(): number {
    return this.size;
  }

  getCapacity(): number {
    return this.capacity;
  }
}
