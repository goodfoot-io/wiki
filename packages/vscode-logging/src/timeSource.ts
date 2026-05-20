/**
 * Shared time source abstractions for deterministic timestamps and metrics.
 *
 * @summary Shared time source abstractions for deterministic timestamps and metrics
 */
export interface TimeSource {
  now(): number;
  toISOString(date: Date): string;
}

export interface PerformanceTimeSource {
  now(): number;
}

export const systemTimeSource: TimeSource = {
  now: () => Date.now(),
  toISOString: (date: Date) => date.toISOString()
};

export const systemPerformanceTimeSource: PerformanceTimeSource = {
  now: () => (typeof performance !== 'undefined' ? performance.now() : Date.now())
};

export class FixedTimeSource implements TimeSource {
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

  toISOString(date: Date): string {
    return date.toISOString();
  }
}
