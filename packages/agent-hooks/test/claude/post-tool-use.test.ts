import { chmodSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { Logger, type PostToolUseInput } from '@goodfoot/agent-hooks/claude-code';
import { afterEach, describe, expect, it } from 'vitest';
import hook from '../../src/claude/post-tool-use.js';
import { resolveWikiBinary, type WikiCheckLogger } from '../../src/common/wiki-check.js';

const logger = new Logger();

let fixtureDir: string | undefined;
let counter = 0;

function makeFixture(name = 'page.md', content = '---\ntitle: T\nsummary: S\n---\nbody'): string {
  if (!fixtureDir) fixtureDir = mkdirSync(join(tmpdir(), 'claude-wiki-fixtures-'), { recursive: true });
  const path = join(fixtureDir, name);
  writeFileSync(path, content, 'utf-8');
  return path;
}

function makeBinary(script: string): string {
  if (!fixtureDir) fixtureDir = mkdirSync(join(tmpdir(), 'claude-wiki-fixtures-'), { recursive: true });
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
    session_id: 'claude-e2e',
    transcript_path: '/test/transcript.jsonl',
    hook_event_name: 'PostToolUse',
    cwd: fixtureDir ?? process.cwd(),
    tool_name: toolName,
    tool_input: toolInput
  };
}

describe('post-tool-use', () => {
  describe('hook metadata', () => {
    it('is a function', () => {
      expect(typeof hook).toBe('function');
    });

    it('has PostToolUse hook event name', () => {
      expect(hook.eventName).toBe('PostToolUse');
    });

    it('matches file-modifying tools', () => {
      const matcher = hook.matcher;
      expect(typeof matcher).toBe('string');
      expect(matcher).toContain('Edit');
      expect(matcher).toContain('Write');
      expect(matcher).toContain('NotebookEdit');
      expect(matcher).not.toContain('MultiEdit');
    });

    it('has a 60-second timeout', () => {
      expect(hook.timeout).toBe(60000);
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

    it('returns null for regular markdown files without wiki frontmatter', async () => {
      const plain = makeFixture('plain.md', '# just markdown');
      process.env.WIKI_BIN = makeBinary('echo "should not run" ; exit 1');

      try {
        const result = await hook(inputFor('Edit', { file_path: plain, old_string: 'a', new_string: 'b' }), {
          logger
        });
        expect(result).toBeNull();
      } finally {
        delete process.env.WIKI_BIN;
      }
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

  describe('binary resolution', () => {
    const originalWikiBin = process.env.WIKI_BIN;
    afterEach(() => {
      if (originalWikiBin === undefined) delete process.env.WIKI_BIN;
      else process.env.WIKI_BIN = originalWikiBin;
    });

    function recordingLogger(): { probe: WikiCheckLogger; warnings: Array<{ message: string; context?: unknown }> } {
      const warnings: Array<{ message: string; context?: unknown }> = [];
      return {
        probe: {
          info: () => undefined,
          warn: (message, context) => warnings.push({ message, context })
        },
        warnings
      };
    }

    it('honors WIKI_BIN when it points at an existing file and logs nothing', () => {
      // process.execPath (the node binary) is guaranteed to exist.
      process.env.WIKI_BIN = process.execPath;
      const { probe, warnings } = recordingLogger();
      expect(resolveWikiBinary(probe)).toBe(process.execPath);
      // A healthy override must stay silent.
      expect(warnings).toHaveLength(0);
    });

    it('warns when WIKI_BIN is set but its path does not exist, then falls through', () => {
      const rejected = '/definitely/not/a/real/wiki/binary';
      process.env.WIKI_BIN = rejected;
      const { probe, warnings } = recordingLogger();

      // Falls through to PATH/managed/bare-name resolution — never the bogus path.
      expect(resolveWikiBinary(probe)).not.toBe(rejected);
      expect(warnings).toHaveLength(1);
      expect(warnings[0].message).toContain('WIKI_BIN override rejected');
      expect(JSON.stringify(warnings[0].context)).toContain(rejected);
    });
  });

  describe('end-to-end surfacing for detected wiki pages', () => {
    const originalWikiBin = process.env.WIKI_BIN;
    afterEach(() => {
      if (originalWikiBin === undefined) delete process.env.WIKI_BIN;
      else process.env.WIKI_BIN = originalWikiBin;
    });

    function additionalContext(result: unknown): string {
      const output = result as { stdout?: { hookSpecificOutput?: { additionalContext?: string } } } | null;
      return output?.stdout?.hookSpecificOutput?.additionalContext ?? '';
    }

    it('surfaces residual diagnostics inside a wiki block for a real frontmatter page', async () => {
      const page = makeFixture();
      process.env.WIKI_BIN = makeBinary('echo "line-range drift in $3" ; exit 1');

      try {
        const result = await hook(inputFor('Write', { file_path: page, content: 'rewritten' }), { logger });
        const context = additionalContext(result);
        expect(context.startsWith('<wiki>\n')).toBe(true);
        expect(context.endsWith('\n</wiki>')).toBe(true);
        // The stub echoes its third argv word (`check --fix <path>`), proving
        // the detected fixture is the file the binary was spawned against.
        expect(context).toContain(`line-range drift in ${page}`);
      } finally {
        delete process.env.WIKI_BIN;
      }
    });

    it('returns null when the check passes clean', async () => {
      const page = makeFixture();
      process.env.WIKI_BIN = makeBinary('exit 0');

      try {
        const result = await hook(inputFor('Edit', { file_path: page, old_string: 'body', new_string: 'changed' }), {
          logger
        });
        expect(result).toBeNull();
      } finally {
        delete process.env.WIKI_BIN;
      }
    });

    it('appends the loud SKIPPED block when the binary cannot be launched', async () => {
      const page = makeFixture();
      // An existing but non-executable override stops resolution deterministically
      // (existsSync passes, spawn fails with EACCES) regardless of ambient PATH.
      const deadBinary = join(fixtureDir ?? tmpdir(), `dead-${counter + 1}`);
      counter += 1;
      writeFileSync(deadBinary, '#!/bin/sh\nexit 0\n', 'utf-8');
      process.env.WIKI_BIN = deadBinary;

      try {
        const result = await hook(inputFor('Write', { file_path: page, content: 'rewritten' }), { logger });
        const context = additionalContext(result);
        expect(context.startsWith('<wiki>\n')).toBe(true);
        expect(context).toContain('wiki validation was SKIPPED');
        expect(context).toContain(page);
      } finally {
        delete process.env.WIKI_BIN;
      }
    });
  });
});
