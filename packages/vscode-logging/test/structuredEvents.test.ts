/**
 * Tests for structured event logging (logRefTrackingEvent).
 *
 * @summary Tests for structured event logging (logRefTrackingEvent)
 */
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import {
  logRefTrackingEvent,
  type RefTrackingCategory,
  setStructuredEventsLogger,
  truncateHash
} from '../src/structuredEvents.js';
import { StubLogOutputChannel } from '../src/vscode-shim.js';

describe('truncateHash', () => {
  it('should truncate 40-character hex strings to 7 characters', () => {
    const fullHash = 'abc123def456abc123def456abc123def456ab00';
    expect(truncateHash(fullHash)).toBe('abc123d');
  });

  it('should truncate uppercase git hashes too', () => {
    const fullHash = 'ABC123DEF456ABC123DEF456ABC123DEF456AB00';
    expect(truncateHash(fullHash)).toBe('ABC123D');
  });

  it('should truncate mixed-case git hashes', () => {
    const fullHash = 'AbC123DeF456AbC123DeF456AbC123DeF456Ab00';
    expect(truncateHash(fullHash)).toBe('AbC123D');
  });

  it('should leave non-hash strings unchanged', () => {
    expect(truncateHash('main')).toBe('main');
    expect(truncateHash('feature/my-branch')).toBe('feature/my-branch');
    expect(truncateHash('reason')).toBe('reason');
  });

  it('should leave short hex strings unchanged', () => {
    expect(truncateHash('abc123d')).toBe('abc123d');
    expect(truncateHash('abc123')).toBe('abc123');
  });

  it('should leave 40-character non-hex strings unchanged', () => {
    const nonHex40Char = 'x'.repeat(40);
    expect(truncateHash(nonHex40Char)).toBe(nonHex40Char);
  });

  it('should leave empty strings unchanged', () => {
    expect(truncateHash('')).toBe('');
  });

  it('should leave numeric strings unchanged', () => {
    expect(truncateHash('12345')).toBe('12345');
    // Note: A 40-digit string will match the hex pattern (0-9 is valid hex)
    // So this test uses a string with non-hex characters
    expect(truncateHash('not-a-hash-this-is-1234567890-ok-to-keep')).toBe('not-a-hash-this-is-1234567890-ok-to-keep');
  });
});

describe('logRefTrackingEvent', () => {
  let stubLogger: StubLogOutputChannel;
  let capturedMessages: Array<{ level: string; message: string }> = [];

  beforeEach(() => {
    stubLogger = new StubLogOutputChannel('Test Logger');
    capturedMessages = [];

    // Override the stub methods to capture calls
    stubLogger.debug = (message: string) => {
      capturedMessages.push({ level: 'debug', message });
    };
    stubLogger.info = (message: string) => {
      capturedMessages.push({ level: 'info', message });
    };
    stubLogger.warn = (message: string) => {
      capturedMessages.push({ level: 'warn', message });
    };
    stubLogger.error = (message: string | Error) => {
      capturedMessages.push({ level: 'error', message: String(message) });
    };

    setStructuredEventsLogger(stubLogger);
  });

  afterEach(() => {
    setStructuredEventsLogger(undefined);
    capturedMessages = [];
  });

  it('should format message correctly with category and fields', () => {
    logRefTrackingEvent('REF_CHANGE', {
      ref: 'main',
      reason: 'initial'
    });

    expect(capturedMessages).toHaveLength(1);
    expect(capturedMessages[0]!.message).toMatch(/^\[REF_CHANGE\]/);
    expect(capturedMessages[0]!.message).toContain('ref=main');
    expect(capturedMessages[0]!.message).toContain('reason=initial');
  });

  it('should truncate git hashes in fields', () => {
    const fullHash = 'abc123def456abc123def456abc123def456ab';
    logRefTrackingEvent('REF_CHANGE', {
      oldHash: fullHash,
      newHash: fullHash
    });

    expect(capturedMessages).toHaveLength(1);
    expect(capturedMessages[0]!.message).toContain('oldHash=abc123d');
    expect(capturedMessages[0]!.message).toContain('newHash=abc123d');
  });

  it('should handle undefined values', () => {
    logRefTrackingEvent('REF_CHANGE', {
      ref: 'main',
      oldHash: undefined,
      newHash: 'abc123d'
    });

    expect(capturedMessages).toHaveLength(1);
    expect(capturedMessages[0]!.message).toContain('oldHash=undefined');
  });

  it('should handle numeric values', () => {
    logRefTrackingEvent('SESSION_UPDATE', {
      sessionId: 123,
      timestamp: 1234567890,
      active: true
    });

    expect(capturedMessages).toHaveLength(1);
    expect(capturedMessages[0]!.message).toContain('sessionId=123');
    expect(capturedMessages[0]!.message).toContain('timestamp=1234567890');
    expect(capturedMessages[0]!.message).toContain('active=true');
  });

  it('should handle boolean values', () => {
    logRefTrackingEvent('UI_UPDATE', {
      hasChanges: true,
      isValid: false
    });

    expect(capturedMessages).toHaveLength(1);
    expect(capturedMessages[0]!.message).toContain('hasChanges=true');
    expect(capturedMessages[0]!.message).toContain('isValid=false');
  });

  it('should use debug level for REF_CHANGE', () => {
    logRefTrackingEvent('REF_CHANGE', { ref: 'main' });

    expect(capturedMessages).toHaveLength(1);
    expect(capturedMessages[0]!.level).toBe('debug');
  });

  it('should use debug level for REF_VALIDATION', () => {
    logRefTrackingEvent('REF_VALIDATION', { valid: true });

    expect(capturedMessages).toHaveLength(1);
    expect(capturedMessages[0]!.level).toBe('debug');
  });

  it('should use debug level for REFRESH_DECISION', () => {
    logRefTrackingEvent('REFRESH_DECISION', { decision: 'skip' });

    expect(capturedMessages).toHaveLength(1);
    expect(capturedMessages[0]!.level).toBe('debug');
  });

  it('should use warn level for REF_INVALID', () => {
    logRefTrackingEvent('REF_INVALID', { reason: 'deleted' });

    expect(capturedMessages).toHaveLength(1);
    expect(capturedMessages[0]!.level).toBe('warn');
  });

  it('should use info level for REF_RECOVER', () => {
    logRefTrackingEvent('REF_RECOVER', { recovered: true });

    expect(capturedMessages).toHaveLength(1);
    expect(capturedMessages[0]!.level).toBe('info');
  });

  it('should use info level for SESSION_UPDATE', () => {
    logRefTrackingEvent('SESSION_UPDATE', { sessionId: 123 });

    expect(capturedMessages).toHaveLength(1);
    expect(capturedMessages[0]!.level).toBe('info');
  });

  it('should use info level for UI_UPDATE', () => {
    logRefTrackingEvent('UI_UPDATE', { component: 'tree' });

    expect(capturedMessages).toHaveLength(1);
    expect(capturedMessages[0]!.level).toBe('info');
  });

  it('should gracefully degrade when no logger is set', () => {
    setStructuredEventsLogger(undefined);
    capturedMessages = [];

    // Should not throw
    expect(() => {
      logRefTrackingEvent('REF_CHANGE', { ref: 'main' });
    }).not.toThrow();

    // Should not capture anything
    expect(capturedMessages).toHaveLength(0);
  });

  it('should handle all category types', () => {
    const categories: RefTrackingCategory[] = [
      'REF_CHANGE',
      'REF_INVALID',
      'REF_RECOVER',
      'REF_VALIDATION',
      'SESSION_UPDATE',
      'REFRESH_DECISION',
      'UI_UPDATE'
    ];

    for (const category of categories) {
      capturedMessages = [];
      expect(() => {
        logRefTrackingEvent(category, { test: 'value' });
      }).not.toThrow();
      expect(capturedMessages).toHaveLength(1);
    }
  });

  it('should format fields in key=value pairs separated by spaces', () => {
    logRefTrackingEvent('REF_CHANGE', {
      ref: 'feature',
      oldHash: 'abc1234567890123456789012345678901234567',
      newHash: 'def1234567890123456789012345678901234567',
      reason: 'checkout'
    });

    expect(capturedMessages).toHaveLength(1);
    const msg = capturedMessages[0]!.message;
    // Should have format: [REF_CHANGE] key1=value1 key2=value2 key3=value3 key4=value4
    const parts = msg.split('] ')[1]!.split(' ');
    expect(parts.length).toBe(4); // 4 key=value pairs
    expect(parts[0]).toMatch(/^[a-z]+=/);
  });

  it('should work correctly with empty fields object', () => {
    logRefTrackingEvent('REF_CHANGE', {});

    expect(capturedMessages).toHaveLength(1);
    expect(capturedMessages[0]!.message).toBe('[REF_CHANGE] ');
  });

  it('should preserve category in message', () => {
    const categories: RefTrackingCategory[] = [
      'REF_CHANGE',
      'REF_INVALID',
      'REF_RECOVER',
      'REF_VALIDATION',
      'SESSION_UPDATE',
      'REFRESH_DECISION',
      'UI_UPDATE'
    ];

    for (const category of categories) {
      capturedMessages = [];
      logRefTrackingEvent(category, { test: 'value' });
      expect(capturedMessages[0]!.message).toContain(`[${category}]`);
    }
  });
});
