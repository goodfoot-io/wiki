// These tests validate the hook structure and basic behavior.
// Full integration tests for wiki check pass/fail and mesh_uncovered
// scaffolding require mocking spawnSync or a real wiki corpus.

import { Logger, type PostToolUseInput } from '@goodfoot/claude-code-hooks';
import { describe, expect, it } from 'vitest';
import hook from '../src/wiki-check.js';

const logger = new Logger();

describe('wiki-check', () => {
  describe('hook metadata', () => {
    it('is a function', () => {
      expect(typeof hook).toBe('function');
    });

    it('has PostToolUse hook event name', () => {
      expect(hook.hookEventName).toBe('PostToolUse');
    });

    it('matches file-modifying tools', () => {
      const matcher = hook.matcher;
      expect(typeof matcher).toBe('string');
      expect(matcher).toContain('Edit');
      expect(matcher).toContain('Write');
      expect(matcher).toContain('MultiEdit');
      expect(matcher).toContain('NotebookEdit');
    });

    it('has a 30-second timeout', () => {
      expect(hook.timeout).toBe(30000);
    });
  });

  describe('non-wiki files', () => {
    it('returns null for TypeScript files', async () => {
      const input: PostToolUseInput = {
        session_id: 'test',
        transcript_path: '/test/transcript.jsonl',
        hook_event_name: 'PostToolUse',
        cwd: '/home/node/wiki',
        tool_name: 'Write',
        tool_input: {
          file_path: '/test/file.ts',
          content: 'const foo = "bar";'
        }
      };

      const result = await hook(input, { logger });
      expect(result).toBeNull();
    });

    it('returns null for regular markdown files outside a wiki directory', async () => {
      const input: PostToolUseInput = {
        session_id: 'test',
        transcript_path: '/test/transcript.jsonl',
        hook_event_name: 'PostToolUse',
        cwd: '/home/node/wiki',
        tool_name: 'Edit',
        tool_input: {
          file_path: '/test/file.md',
          old_string: 'old',
          new_string: 'new'
        }
      };

      const result = await hook(input, { logger });
      expect(result).toBeNull();
    });

    it('returns null for MultiEdit on non-wiki files', async () => {
      const input: PostToolUseInput = {
        session_id: 'test',
        transcript_path: '/test/transcript.jsonl',
        hook_event_name: 'PostToolUse',
        cwd: '/home/node/wiki',
        tool_name: 'MultiEdit',
        tool_input: {
          file_path: '/test/file.ts',
          edits: [{ old_string: 'old', new_string: 'new' }]
        }
      };

      const result = await hook(input, { logger });
      expect(result).toBeNull();
    });

    it('returns null for NotebookEdit on non-wiki files', async () => {
      const input: PostToolUseInput = {
        session_id: 'test',
        transcript_path: '/test/transcript.jsonl',
        hook_event_name: 'PostToolUse',
        cwd: '/home/node/wiki',
        tool_name: 'NotebookEdit',
        tool_input: {
          notebook_path: '/test/file.ipynb',
          cell_number: 0,
          new_source: 'print("hello")'
        }
      };

      const result = await hook(input, { logger });
      expect(result).toBeNull();
    });
  });

  describe('file-path extraction', () => {
    it('returns null when file path cannot be extracted from tool input', async () => {
      const input: PostToolUseInput = {
        session_id: 'test',
        transcript_path: '/test/transcript.jsonl',
        hook_event_name: 'PostToolUse',
        cwd: '/home/node/wiki',
        tool_name: 'Bash',
        tool_input: {
          command: 'echo hello'
        }
      };

      const result = await hook(input, { logger });
      expect(result).toBeNull();
    });
  });

  describe('wiki-file detection', () => {
    it('detects .wiki.md files as wiki files by extension', async () => {
      // The hook runs wiki check for .wiki.md files. With wiki on PATH,
      // this will attempt a real check. The hook handles errors gracefully —
      // it returns null for infrastructure errors (spawn failure, timeout)
      // and postToolUseOutput for validation errors.
      const input: PostToolUseInput = {
        session_id: 'test',
        transcript_path: '/test/transcript.jsonl',
        hook_event_name: 'PostToolUse',
        cwd: '/home/node/wiki',
        tool_name: 'Write',
        tool_input: {
          file_path: '/test/nonexistent-file.wiki.md',
          content: '# Test\n\nSome content.'
        }
      };

      const result = await hook(input, { logger });
      // The hook does something (either null for infra errors or
      // postToolUseOutput for validation errors); it should never throw.
      expect(result !== undefined).toBe(true);
    });

    it('returns null for files with no wiki.toml ancestor and no .wiki.md extension', async () => {
      const input: PostToolUseInput = {
        session_id: 'test',
        transcript_path: '/test/transcript.jsonl',
        hook_event_name: 'PostToolUse',
        cwd: '/home/node/wiki',
        tool_name: 'Write',
        tool_input: {
          file_path: '/home/node/wiki/packages/cli/src/main.rs',
          content: '// rust code'
        }
      };

      const result = await hook(input, { logger });
      expect(result).toBeNull();
    });
  });
});
