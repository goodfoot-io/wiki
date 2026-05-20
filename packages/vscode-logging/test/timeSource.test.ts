import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  FixedTimeSource,
  type PerformanceTimeSource,
  systemPerformanceTimeSource,
  systemTimeSource,
  type TimeSource
} from '../src/timeSource.js';

/**
 * Exercises time source behavior in the test area through focused scenarios.
 * The cases lock in edge handling and regression coverage so refactors preserve expected state
 * transitions and output.
 *
 * @summary Tests time source behavior in test
 */

describe('TimeSource interface', () => {
  it('should define TimeSource interface with now and toISOString methods', () => {
    const mockTimeSource: TimeSource = {
      now: () => 1000,
      toISOString: (date: Date) => date.toISOString()
    };

    expect(typeof mockTimeSource.now).toBe('function');
    expect(typeof mockTimeSource.toISOString).toBe('function');
  });
});

describe('PerformanceTimeSource interface', () => {
  it('should define PerformanceTimeSource interface with now method', () => {
    const mockPerformanceTimeSource: PerformanceTimeSource = {
      now: () => 1000
    };

    expect(typeof mockPerformanceTimeSource.now).toBe('function');
  });
});

describe('systemTimeSource', () => {
  let dateNowSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    dateNowSpy = vi.spyOn(Date, 'now');
  });

  afterEach(() => {
    dateNowSpy.mockRestore();
  });

  it('should return current timestamp from Date.now()', () => {
    const testTimestamp = 1234567890;
    dateNowSpy.mockReturnValue(testTimestamp);

    const result = systemTimeSource.now();

    expect(result).toBe(testTimestamp);
    expect(dateNowSpy).toHaveBeenCalled();
  });

  it('should return ISO string from date', () => {
    const testDate = new Date('2023-01-15T12:34:56.789Z');
    const expected = testDate.toISOString();

    const result = systemTimeSource.toISOString(testDate);

    expect(result).toBe(expected);
    expect(result).toMatch(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/);
  });

  it('should be a singleton with predictable behavior', () => {
    dateNowSpy.mockReturnValue(100);
    const result1 = systemTimeSource.now();

    dateNowSpy.mockReturnValue(200);
    const result2 = systemTimeSource.now();

    expect(result1).toBe(100);
    expect(result2).toBe(200);
  });
});

describe('systemPerformanceTimeSource', () => {
  let performanceNowSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    performanceNowSpy = vi.spyOn(performance, 'now');
  });

  afterEach(() => {
    performanceNowSpy.mockRestore();
  });

  it('should return performance.now() value', () => {
    const testValue = 12345.67;
    performanceNowSpy.mockReturnValue(testValue);

    const result = systemPerformanceTimeSource.now();

    expect(result).toBe(testValue);
    expect(performanceNowSpy).toHaveBeenCalled();
  });

  it('should return a number greater than or equal to zero', () => {
    performanceNowSpy.mockReturnValue(0);
    expect(systemPerformanceTimeSource.now()).toBeGreaterThanOrEqual(0);

    performanceNowSpy.mockReturnValue(999999.999);
    expect(systemPerformanceTimeSource.now()).toBeGreaterThanOrEqual(0);
  });
});

describe('FixedTimeSource', () => {
  it('should initialize with start time', () => {
    const startMs = 1000;
    const timeSource = new FixedTimeSource(startMs);

    const result = timeSource.now();

    expect(result).toBe(startMs);
  });

  it('should return the same time on first call with no step', () => {
    const startMs = 5000;
    const timeSource = new FixedTimeSource(startMs, 0);

    const result1 = timeSource.now();
    const result2 = timeSource.now();

    expect(result1).toBe(startMs);
    expect(result2).toBe(startMs);
  });

  it('should increment by step value on each call', () => {
    const startMs = 1000;
    const stepMs = 100;
    const timeSource = new FixedTimeSource(startMs, stepMs);

    expect(timeSource.now()).toBe(1000);
    expect(timeSource.now()).toBe(1100);
    expect(timeSource.now()).toBe(1200);
    expect(timeSource.now()).toBe(1300);
  });

  it('should support negative step values', () => {
    const startMs = 1000;
    const stepMs = -50;
    const timeSource = new FixedTimeSource(startMs, stepMs);

    expect(timeSource.now()).toBe(1000);
    expect(timeSource.now()).toBe(950);
    expect(timeSource.now()).toBe(900);
  });

  it('should default to stepMs of 0 when not provided', () => {
    const startMs = 2000;
    const timeSource = new FixedTimeSource(startMs);

    const result1 = timeSource.now();
    const result2 = timeSource.now();

    expect(result1).toBe(startMs);
    expect(result2).toBe(startMs);
  });

  it('should convert date to ISO string correctly', () => {
    const timeSource = new FixedTimeSource(1000);
    const testDate = new Date('2023-06-15T10:30:45.123Z');

    const result = timeSource.toISOString(testDate);

    expect(result).toBe(testDate.toISOString());
    expect(result).toBe('2023-06-15T10:30:45.123Z');
  });

  it('should implement TimeSource interface correctly', () => {
    const timeSource = new FixedTimeSource(1000, 10);
    const implementsTimeSource: TimeSource = timeSource;

    expect(typeof implementsTimeSource.now).toBe('function');
    expect(typeof implementsTimeSource.toISOString).toBe('function');

    const result1 = implementsTimeSource.now();
    expect(result1).toBe(1000);

    const result2 = implementsTimeSource.now();
    expect(result2).toBe(1010);
  });

  it('should handle zero start time with positive steps', () => {
    const timeSource = new FixedTimeSource(0, 50);

    expect(timeSource.now()).toBe(0);
    expect(timeSource.now()).toBe(50);
    expect(timeSource.now()).toBe(100);
  });

  it('should handle large time values', () => {
    const startMs = Number.MAX_SAFE_INTEGER - 100;
    const timeSource = new FixedTimeSource(startMs, 10);

    const result1 = timeSource.now();
    expect(result1).toBe(startMs);
  });
});
