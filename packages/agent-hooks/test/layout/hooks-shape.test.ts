import { accessSync, existsSync, constants as fsConstants, readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../../../..');

interface HookCommand {
  type?: string;
  command?: string;
  timeout?: number;
}

interface HookGroup {
  matcher?: string;
  hooks?: HookCommand[];
}

/** Event name → matcher groups; PostToolUse is the only event the plugin registers. */
interface HookEvents {
  PostToolUse?: HookGroup[];
  [event: string]: HookGroup[] | undefined;
}

interface HooksManifest {
  hooks?: HookEvents;
}

function readManifest(relativePath: string): HooksManifest {
  return JSON.parse(readFileSync(resolve(repoRoot, relativePath), 'utf8')) as HooksManifest;
}

function expectSingleEvent(manifest: HooksManifest, label: string): HookGroup[] {
  const events = Object.keys(manifest.hooks ?? {});
  expect(events, `${label} must register exactly one event`).toEqual(['PostToolUse']);
  const groups = manifest.hooks?.PostToolUse ?? [];
  expect(groups, `${label} must have exactly one matcher group`).toHaveLength(1);
  return groups;
}

describe('emitted claude hooks.json shape', () => {
  const manifest = readManifest('plugins-claude/wiki/hooks/hooks.json');
  const groups = expectSingleEvent(manifest, 'claude manifest');

  it('uses the $CLAUDE_PLUGIN_ROOT command with a seconds timeout and Edit|Write|NotebookEdit matcher', () => {
    expect(groups[0]?.matcher).toBe('Edit|Write|NotebookEdit');
    const commands = groups[0]?.hooks ?? [];
    expect(commands).toHaveLength(1);
    expect(commands[0]?.type).toBe('command');
    expect(commands[0]?.command).toBe('node "$CLAUDE_PLUGIN_ROOT"/hooks/bin/post-tool-use.mjs');
    expect(commands[0]?.timeout).toBe(60);
  });

  it('declares exactly the post-tool-use bundle and stamps no timestamp', () => {
    // The CLI tracks generated bundles in a hooks.meta.json sidecar; the
    // manifest itself carries the registration only.
    expect(Object.keys(manifest)).toEqual(['hooks']);
    const meta: unknown = JSON.parse(
      readFileSync(resolve(repoRoot, 'plugins-claude/wiki/hooks/hooks.meta.json'), 'utf8')
    );
    expect(meta).toStrictEqual({ files: ['post-tool-use.mjs'] });
  });

  it('ships an executable bin/post-tool-use.mjs next to the manifest', () => {
    const bundle = resolve(repoRoot, 'plugins-claude/wiki/hooks/bin/post-tool-use.mjs');
    expect(() => accessSync(bundle, fsConstants.X_OK)).not.toThrow();
  });
});

describe('emitted codex hooks.json shape', () => {
  const manifest = readManifest('plugins-codex/wiki/hooks/hooks.json');
  const groups = expectSingleEvent(manifest, 'codex manifest');

  it(`uses the \${PLUGIN_ROOT} command with a seconds timeout and the patch/shell tool matcher`, () => {
    expect(groups[0]?.matcher).toBe('apply_patch|exec_command|exec|shell|local_shell');
    const commands = groups[0]?.hooks ?? [];
    expect(commands).toHaveLength(1);
    expect(commands[0]?.type).toBe('command');
    expect(commands[0]?.command).toBe(`node "\${PLUGIN_ROOT}/hooks/post-tool-use.mjs"`);
    expect(commands[0]?.timeout).toBe(60);
  });

  it('ships a sibling flat post-tool-use.mjs bundle (no bin/ subdirectory)', () => {
    expect(existsSync(resolve(repoRoot, 'plugins-codex/wiki/hooks/post-tool-use.mjs'))).toBe(true);
    expect(existsSync(resolve(repoRoot, 'plugins-codex/wiki/hooks/bin'))).toBe(false);
  });
});
