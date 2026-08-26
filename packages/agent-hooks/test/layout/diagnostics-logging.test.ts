import { spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../../../..');

function makeWikiFixture(): { cwd: string; filePath: string } {
  const cwd = mkdtempSync(join(tmpdir(), 'diagnostics-logging-'));
  const filePath = join(cwd, 'page.md');
  writeFileSync(filePath, '---\ntitle: T\nsummary: S\n---\nbody', 'utf-8');
  return { cwd, filePath };
}

// Isolates resolution from this machine's real environment: an empty PATH
// alone is not enough to force the 'unavailable' branch here, because
// resolveWikiBinary also falls back to a managed VS Code globalStorage
// install keyed off HOME, which this environment happens to have.
function isolatedEnv(extra: Record<string, string>): Record<string, string> {
  const emptyPath = mkdtempSync(join(tmpdir(), 'empty-path-'));
  const emptyHome = mkdtempSync(join(tmpdir(), 'empty-home-'));
  return { PATH: emptyPath, HOME: emptyHome, ...extra };
}

function readJsonlLines(logFile: string): unknown[] {
  const content = readFileSync(logFile, 'utf-8');
  expect(content.length).toBeGreaterThan(0);
  return content
    .split('\n')
    .filter((line) => line.trim().length > 0)
    .map((line) => JSON.parse(line));
}

describe('emitted bundles write non-empty diagnostics through AGENT_HOOKS_LOG_FILE', () => {
  it('claude bundle logs a wiki check execution error when the wiki binary cannot be resolved', () => {
    const { cwd, filePath } = makeWikiFixture();
    const logFile = join(cwd, 'claude.log.jsonl');

    const input = {
      session_id: 'diagnostics-e2e',
      transcript_path: '/test/transcript.jsonl',
      hook_event_name: 'PostToolUse',
      cwd,
      tool_name: 'Write',
      tool_input: { file_path: filePath, content: 'rewritten' }
    };

    const result = spawnSync(process.execPath, [resolve(repoRoot, 'plugins-claude/wiki/hooks/bin/post-tool-use.mjs')], {
      input: JSON.stringify(input),
      encoding: 'utf-8',
      env: isolatedEnv({ AGENT_HOOKS_LOG_FILE: logFile })
    });

    expect(result.error).toBeUndefined();
    const lines = readJsonlLines(logFile);
    expect(lines.length).toBeGreaterThan(0);
    expect(lines.some((line) => JSON.stringify(line).includes('wiki check execution error'))).toBe(true);
  });

  it('codex bundle logs a wiki check execution error when the wiki binary cannot be resolved', () => {
    const { cwd, filePath } = makeWikiFixture();
    const logFile = join(cwd, 'codex.log.jsonl');

    const patch = `*** Begin Patch\n*** Update File: ${filePath}\n*** End Patch`;
    const input = {
      cwd,
      hook_event_name: 'PostToolUse',
      model: 'test-model',
      session_id: 'diagnostics-e2e',
      transcript_path: null,
      permission_mode: 'default',
      tool_name: 'apply_patch',
      tool_response: null,
      tool_use_id: 'call-1',
      turn_id: 'turn-1',
      tool_input: { command: patch }
    };

    const result = spawnSync(process.execPath, [resolve(repoRoot, 'plugins-codex/wiki/hooks/post-tool-use.mjs')], {
      input: JSON.stringify(input),
      encoding: 'utf-8',
      env: isolatedEnv({ AGENT_HOOKS_LOG_FILE: logFile })
    });

    expect(result.error).toBeUndefined();
    const lines = readJsonlLines(logFile);
    expect(lines.length).toBeGreaterThan(0);
    expect(lines.some((line) => JSON.stringify(line).includes('wiki check execution error'))).toBe(true);
  });
});
