import { Logger, type StopInput } from '@goodfoot/claude-code-hooks';
import { describe, expect, it } from 'vitest';
import hook from '../src/stop.js';

const logger = new Logger();

describe('stop', () => {
  describe('hook metadata', () => {
    it('is a function', () => {
      expect(typeof hook).toBe('function');
    });

    it('has Stop hook event name', () => {
      expect(hook.hookEventName).toBe('Stop');
    });
  });

  describe('no tracked files', () => {
    it('returns null when no wiki files were edited in the session', async () => {
      const input: StopInput = {
        session_id: 'test-no-files',
        transcript_path: '/test/transcript.jsonl',
        hook_event_name: 'Stop',
        cwd: '/home/node/wiki'
      };

      const result = await hook(input, { logger });
      expect(result).toBeNull();
    });
  });
});
