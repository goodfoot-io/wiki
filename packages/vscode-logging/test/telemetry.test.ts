/**
 * Tests for telemetry integration with consent compliance.
 *
 * @summary Tests for telemetry integration with consent compliance
 */
import { beforeEach, describe, expect, it } from 'vitest';
import { createLogger, type DecoratedLogger, getLogBuffer } from '../src/logger.js';
import {
  type ErrorContext,
  formatError,
  logErrorWithTelemetry,
  sanitizeErrorMessage,
  setTelemetryLogger,
  setTelemetryService,
  type TelemetryService
} from '../src/telemetry.js';

/**
 * Mock TelemetryService for testing.
 */
class MockTelemetryService implements TelemetryService {
  public errors: Array<{ error: Error; context?: Partial<ErrorContext> }> = [];

  sendError(error: Error, context?: Partial<ErrorContext>): void {
    this.errors.push({ error, context });
  }

  clear(): void {
    this.errors = [];
  }
}

describe('setTelemetryService', () => {
  it('should set the telemetry service instance', () => {
    const service = new MockTelemetryService();
    setTelemetryService(service);

    // Verify by using it (no direct getter to test)
    const error = new Error('test');
    logErrorWithTelemetry('test message', error);

    expect(service.errors.length).toBe(1);
    expect(service.errors[0]?.error).toBe(error);

    // Cleanup
    setTelemetryService(undefined);
  });

  it('should allow unsetting the telemetry service', () => {
    const service = new MockTelemetryService();
    setTelemetryService(service);
    setTelemetryService(undefined);

    const error = new Error('test');
    logErrorWithTelemetry('test message', error);

    // Should not send to telemetry when service is unset
    expect(service.errors.length).toBe(0);
  });
});

describe('logErrorWithTelemetry', () => {
  let logger: DecoratedLogger;
  let service: MockTelemetryService;

  beforeEach(() => {
    logger = createLogger('Test Logger');
    setTelemetryLogger(logger);

    service = new MockTelemetryService();
    setTelemetryService(service);

    getLogBuffer().clear();
  });

  it('should log error to logger', () => {
    const messages: string[] = [];
    logger.debugEmitter.on('error', (message: string | Error) => {
      messages.push(String(message));
    });

    const error = new Error('test error');
    logErrorWithTelemetry('operation failed', error);

    expect(messages.length).toBe(1);
    expect(messages[0]).toBe('operation failed');

    logger.debugEmitter.removeAllListeners();
  });

  it('should send Error objects to telemetry', () => {
    const error = new Error('test error');
    logErrorWithTelemetry('operation failed', error, {
      errorType: 'TestError',
      errorCategory: 'user'
    });

    expect(service.errors.length).toBe(1);
    const sent = service.errors[0];
    expect(sent?.error).toBe(error);
    expect(sent?.context?.operationName).toBe('operation failed');
    expect(sent?.context?.errorType).toBe('TestError');
    expect(sent?.context?.errorCategory).toBe('user');
  });

  it('should include custom context in telemetry', () => {
    const error = new Error('git error');
    logErrorWithTelemetry('git operation', error, {
      errorCategory: 'git',
      gitAvailable: true
    });

    expect(service.errors.length).toBe(1);
    const sent = service.errors[0];
    expect(sent?.context?.operationName).toBe('git operation');
    expect(sent?.context?.errorCategory).toBe('git');
    expect(sent?.context?.gitAvailable).toBe(true);
  });

  it('should not send non-Error objects to telemetry', () => {
    logErrorWithTelemetry('operation failed', 'string error');

    expect(service.errors.length).toBe(0);
  });

  it('should handle null/undefined errors gracefully', () => {
    logErrorWithTelemetry('operation failed', null);
    logErrorWithTelemetry('operation failed', undefined);

    expect(service.errors.length).toBe(0);
  });

  it('should work without telemetry service (graceful degradation)', () => {
    setTelemetryService(undefined);

    const messages: string[] = [];
    logger.debugEmitter.on('error', (message: string | Error) => {
      messages.push(String(message));
    });

    const error = new Error('test error');
    logErrorWithTelemetry('operation failed', error);

    // Should still log to logger
    expect(messages.length).toBe(1);
    expect(messages[0]).toBe('operation failed');

    logger.debugEmitter.removeAllListeners();
  });

  it('should work without logger set (graceful degradation)', () => {
    setTelemetryLogger(undefined);

    const error = new Error('test error');
    logErrorWithTelemetry('operation failed', error);

    // Should still send to telemetry
    expect(service.errors.length).toBe(1);
    expect(service.errors[0]?.error).toBe(error);
  });

  it('should log to buffer even without debug emitter listeners', () => {
    const error = new Error('test error');
    logErrorWithTelemetry('operation failed', error);

    const entries = getLogBuffer().getEntries();
    expect(entries.length).toBe(1);
    const entry = entries[0];
    expect(entry?.level).toBe('error');
    expect(entry?.message).toBe('operation failed');
  });
});

describe('formatError', () => {
  it('should format Error objects to their message', () => {
    const error = new Error('test error message');
    expect(formatError(error)).toBe('test error message');
  });

  it('should convert non-Error objects to string', () => {
    expect(formatError('string error')).toBe('string error');
    expect(formatError(123)).toBe('123');
    expect(formatError({ message: 'object' })).toBe('[object Object]');
  });

  it('should handle null and undefined', () => {
    expect(formatError(null)).toBe('null');
    expect(formatError(undefined)).toBe('undefined');
  });
});

describe('sanitizeErrorMessage', () => {
  it('should strip email addresses', () => {
    expect(sanitizeErrorMessage('Error from user@example.com')).toBe('Error from [email]');
    expect(sanitizeErrorMessage('Contact: john.doe@company.co.uk')).toBe('Contact: [email]');
    expect(sanitizeErrorMessage('Multiple: a@b.com and c@d.org')).toBe('Multiple: [email] and [email]');
  });

  it('should strip usernames in paths', () => {
    expect(sanitizeErrorMessage('/Users/john/Documents/file.txt')).toBe('/Users/[user]/Documents/file.txt');
    expect(sanitizeErrorMessage('/home/alice/.config')).toBe('/home/[user]/.config');
    expect(sanitizeErrorMessage('C:\\Users\\bob\\AppData')).toBe('C:\\Users\\[user]\\AppData');
  });

  it('should strip API keys with sk- prefix', () => {
    expect(sanitizeErrorMessage('Key: sk-1234567890abcdef')).toBe('Key: [API_KEY]');
    expect(sanitizeErrorMessage('sk-proj-abcd1234efgh5678')).toBe('[API_KEY]');
  });

  it('should strip GitHub personal access tokens', () => {
    expect(sanitizeErrorMessage('Token: ghp_1234567890abcdefghijklmnopqrst')).toBe('Token: [API_KEY]');
    expect(sanitizeErrorMessage('ghp_abcdefghijklmnopqrstuvwxyz1234567890')).toBe('[API_KEY]');
  });

  it('should strip GitHub OAuth tokens', () => {
    expect(sanitizeErrorMessage('Token: gho_1234567890abcdefghijklmnopqrst')).toBe('Token: [API_KEY]');
  });

  it('should strip GitHub user-to-server tokens', () => {
    expect(sanitizeErrorMessage('Token: ghu_1234567890abcdefghijklmnopqrst')).toBe('Token: [API_KEY]');
  });

  it('should strip GitHub server-to-server tokens', () => {
    expect(sanitizeErrorMessage('Token: ghs_1234567890abcdefghijklmnopqrst')).toBe('Token: [API_KEY]');
  });

  it('should strip GitHub refresh tokens', () => {
    expect(sanitizeErrorMessage('Token: ghr_1234567890abcdefghijklmnopqrst')).toBe('Token: [API_KEY]');
  });

  it('should handle multiple PII patterns in same message', () => {
    const message = 'Error at /Users/john/file.txt from user@example.com with key sk-abc123';
    const sanitized = sanitizeErrorMessage(message);
    expect(sanitized).toBe('Error at /Users/[user]/file.txt from [email] with key [API_KEY]');
  });

  it('should not modify messages without PII', () => {
    const message = 'Error: file not found';
    expect(sanitizeErrorMessage(message)).toBe(message);
  });

  it('should handle empty strings', () => {
    expect(sanitizeErrorMessage('')).toBe('');
  });
});
