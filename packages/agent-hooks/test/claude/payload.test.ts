import { describe, expect, it } from 'vitest';
import { parsePostToolUsePayload } from '../../src/claude/payload.js';

const base = {
  session_id: 'payload-test',
  transcript_path: '/test/transcript.jsonl',
  hook_event_name: 'PostToolUse',
  tool_name: 'Write'
};

describe('parsePostToolUsePayload', () => {
  it('returns the typed shape for a valid payload', () => {
    const payload = parsePostToolUsePayload({
      ...base,
      cwd: '/home/node/wiki',
      tool_input: { file_path: '/home/node/wiki/page.md', content: 'x' }
    });
    expect(payload).toEqual({ cwd: '/home/node/wiki', filePath: '/home/node/wiki/page.md' });
  });

  it('returns null when cwd is missing', () => {
    expect(parsePostToolUsePayload({ ...base, tool_input: { file_path: '/a.md' } })).toBeNull();
  });

  it('returns null when cwd is not a string', () => {
    expect(parsePostToolUsePayload({ ...base, cwd: 42, tool_input: { file_path: '/a.md' } })).toBeNull();
  });

  it('returns null when cwd is an empty string', () => {
    expect(parsePostToolUsePayload({ ...base, cwd: '', tool_input: { file_path: '/a.md' } })).toBeNull();
  });

  it('returns null when the input is not an object', () => {
    expect(parsePostToolUsePayload(null)).toBeNull();
    expect(parsePostToolUsePayload('cwd')).toBeNull();
  });

  it('yields filePath null when the tool input carries no string file_path', () => {
    const cwd = '/home/node/wiki';
    expect(parsePostToolUsePayload({ ...base, cwd })).toEqual({ cwd, filePath: null });
    expect(parsePostToolUsePayload({ ...base, cwd, tool_input: null })).toEqual({ cwd, filePath: null });
    expect(parsePostToolUsePayload({ ...base, cwd, tool_input: { notebook_path: '/n.ipynb' } })).toEqual({
      cwd,
      filePath: null
    });
    expect(parsePostToolUsePayload({ ...base, cwd, tool_input: { file_path: 7 } })).toEqual({ cwd, filePath: null });
    expect(parsePostToolUsePayload({ ...base, cwd, tool_input: { file_path: '' } })).toEqual({ cwd, filePath: null });
  });
});
