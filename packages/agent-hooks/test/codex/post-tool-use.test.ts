import { chmodSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { type HookContext, Logger, type PostToolUseInput } from '@goodfoot/codex-hooks';
import { afterEach, describe, expect, it } from 'vitest';
import {
  createHandler,
  extractPatchedFilePaths,
  default as hook,
  narrowPatchText,
  WIKI_POST_MATCHER
} from '../../src/codex/post-tool-use.js';

const logger = new Logger();
const ctx: HookContext = { logger };

let fixtureDir: string | undefined;
let counter = 0;

function makeWikiFixture(name = 'page.md', content = '---\ntitle: T\nsummary: S\n---\nbody'): string {
  if (!fixtureDir) fixtureDir = mkdirSync(join(tmpdir(), `codex-wiki-fixtures-`), { recursive: true });
  const path = join(fixtureDir, name);
  writeFileSync(path, content, 'utf-8');
  return path;
}

function makeBinary(script: string): string {
  if (!fixtureDir) fixtureDir = mkdirSync(join(tmpdir(), `codex-wiki-fixtures-`), { recursive: true });
  counter += 1;
  const path = join(fixtureDir, `stub-${counter}.sh`);
  writeFileSync(path, `#!/bin/sh\n${script}\n`, 'utf-8');
  chmodSync(path, 0o755);
  return path;
}

afterEach(() => {
  if (fixtureDir) {
    rmSync(fixtureDir, { recursive: true, force: true });
    fixtureDir = undefined;
    counter = 0;
  }
});

function inputFor(toolName: string, toolInput: unknown): PostToolUseInput {
  return {
    cwd: fixtureDir ?? process.cwd(),
    hook_event_name: 'PostToolUse',
    model: 'test-model',
    session_id: 'codex-test-session',
    transcript_path: null,
    permission_mode: 'default',
    tool_name: toolName,
    tool_response: null,
    tool_use_id: 'call-1',
    turn_id: 'turn-1',
    tool_input: toolInput
  };
}

describe('codex post-tool-use', () => {
  describe('hook metadata', () => {
    it('has PostToolUse hook event name', () => {
      expect(hook.hookEventName).toBe('PostToolUse');
    });

    it('matches the codex edit and shell tools', () => {
      expect(hook.matcher).toBe(WIKI_POST_MATCHER);
      for (const tool of ['apply_patch', 'exec_command', 'exec', 'shell', 'local_shell']) {
        expect(hook.matcher).toContain(tool);
      }
      expect(hook.matcher).not.toContain('Bash');
    });

    it('authors the registration timeout in milliseconds', () => {
      expect(hook.timeout).toBe(60000);
    });
  });

  describe('patch-text narrowing', () => {
    it('reads the command envelope', () => {
      expect(narrowPatchText({ command: '*** Begin Patch' })).toBe('*** Begin Patch');
      expect(narrowPatchText({ command: 42 })).toBeNull();
      expect(narrowPatchText(null)).toBeNull();
      expect(narrowPatchText('string-input')).toBeNull();
    });

    it('extracts add, update, and delete paths deduplicated', () => {
      const patch = [
        '*** Begin Patch',
        '*** Update File: a.md',
        '*** Add File: b.md',
        '*** Delete File: a.md',
        '*** End Patch'
      ].join('\n');
      expect(extractPatchedFilePaths(patch)).toEqual(['a.md', 'b.md']);
    });
  });

  describe('handler behavior', () => {
    it('returns undefined for shell tools carrying no patch envelope', async () => {
      const handler = createHandler();
      await expect(handler(inputFor('exec_command', { cmd: 'ls' }), ctx)).resolves.toBeUndefined();
    });

    it('returns undefined when the patch touches no wiki member', async () => {
      const plain = makeWikiFixture('plain.md', '# just markdown');
      const patch = `*** Begin Patch\n*** Update File: ${plain}\n*** End Patch`;
      const handler = createHandler();
      await expect(handler(inputFor('apply_patch', { command: patch }), ctx)).resolves.toBeUndefined();
    });

    it('appends residual diagnostics inside a wiki block on non-zero exit', async () => {
      const wikiPath = makeWikiFixture();
      process.env.WIKI_BIN = makeBinary('echo "line-range drift" ; exit 1');
      const patch = `*** Begin Patch\n*** Update File: ${wikiPath}\n*** End Patch`;
      const handler = createHandler();

      try {
        const result = await handler(inputFor('apply_patch', { command: patch }), ctx);
        expect(result).toBeDefined();
        const context = result?.stdout.hookSpecificOutput?.additionalContext ?? '';
        expect(context.startsWith('<wiki>\n')).toBe(true);
        expect(context.endsWith('\n</wiki>')).toBe(true);
        expect(context).toContain('line-range drift');
      } finally {
        delete process.env.WIKI_BIN;
      }
    });

    it('stays silent on a clean check', async () => {
      const wikiPath = makeWikiFixture();
      process.env.WIKI_BIN = makeBinary('exit 0');
      const patch = `*** Begin Patch\n*** Update File: ${wikiPath}\n*** End Patch`;

      try {
        const result = await createHandler()(inputFor('apply_patch', { command: patch }), ctx);
        expect(result).toBeUndefined();
      } finally {
        delete process.env.WIKI_BIN;
      }
    });

    it('surfaces a loud skip block when the binary cannot be launched', async () => {
      const wikiPath = makeWikiFixture();
      // An existing but non-executable override stops resolution deterministically
      // (existsSync passes, spawn fails with EACCES) regardless of ambient PATH.
      const deadBinary = join(fixtureDir ?? tmpdir(), `stub-dead-${(counter += 1)}`);
      writeFileSync(deadBinary, '#!/bin/sh\nexit 0\n', 'utf-8');
      process.env.WIKI_BIN = deadBinary;
      const patch = `*** Begin Patch\n*** Update File: ${wikiPath}\n*** End Patch`;

      try {
        const result = await createHandler()(inputFor('apply_patch', { command: patch }), ctx);
        const context = result?.stdout.hookSpecificOutput?.additionalContext ?? '';
        expect(context).toContain('wiki validation was SKIPPED');
      } finally {
        delete process.env.WIKI_BIN;
      }
    });
  });
});
