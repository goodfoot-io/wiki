import { chmodSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import type { WikiCheckResult } from '../../src/common/wiki-check.js';
import { assemblePlugin, default as wikiOpencode } from '../../src/opencode/index.js';
import type { OpencodeAfterOutput, OpencodeToolInput } from '../../src/opencode/types.js';

let fixtureDir: string | undefined;

function makeFile(name: string, content: string): string {
  if (!fixtureDir) fixtureDir = mkdirSync(join(tmpdir(), `opencode-wiki-`), { recursive: true });
  const path = join(fixtureDir, name);
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, content, 'utf-8');
  return path;
}

function makeBinary(script: string): string {
  const path = join(fixtureDir ?? '', `stub-wiki-${Math.random().toString(36).slice(2)}.sh`);
  writeFileSync(path, `#!/bin/sh\n${script}\n`, 'utf-8');
  chmodSync(path, 0o755);
  return path;
}

afterEach(() => {
  if (fixtureDir) {
    rmSync(fixtureDir, { recursive: true, force: true });
    fixtureDir = undefined;
  }
});

function afterInput(tool: string, args: unknown): OpencodeToolInput {
  return { tool, sessionID: 'oc-session', callID: 'oc-call', args };
}

function afterOutput(text = 'tool ran'): OpencodeAfterOutput {
  return { title: 't', output: text, metadata: {} };
}

describe('opencode adapter', () => {
  describe('plugin shape', () => {
    it('default export resolves to tool.execute.after plus a no-op-safe dispose', async () => {
      const hooks = await wikiOpencode();
      expect(typeof hooks['tool.execute.after']).toBe('function');
      expect(() => hooks.dispose()).not.toThrow();
      await hooks.dispose();
    });
  });

  describe('tool filtering', () => {
    it('runs the wiki check only for edit and write tools', async () => {
      const wikiPath = makeFile('page.md', '---\ntitle: T\nsummary: S\n---\nbody');
      let calls = 0;
      const hooks = assemblePlugin({
        directory: fixtureDir,
        resolveBinary: () => '/definitely/not/a/real/wiki-binary',
        executeCheck: (filePath, options) => {
          calls += 1;
          return { status: 'clean' as const, filePath, binary: options.binary };
        }
      });

      await hooks['tool.execute.after'](afterInput('bash', { command: 'ls' }), afterOutput());
      await hooks['tool.execute.after'](afterInput('read', { filePath: wikiPath }), afterOutput());
      expect(calls).toBe(0);

      await hooks['tool.execute.after'](afterInput('edit', { filePath: wikiPath }), afterOutput());
      await hooks['tool.execute.after'](afterInput('write', { filePath: wikiPath }), afterOutput());
      expect(calls).toBe(2);
    });
  });

  describe('wiki check wiring', () => {
    it('passes the resolved absolute file path through to the real check runner', async () => {
      const relative = 'notes/rel-page.md';
      makeFile(relative, '---\ntitle: R\nsummary: Resolved\n---\nbody');
      process.env.WIKI_BIN = makeBinary('echo "argv: $@" ; exit 1');
      const hooks = assemblePlugin({ directory: fixtureDir });

      try {
        const output = afterOutput();
        await hooks['tool.execute.after'](afterInput('edit', { filePath: relative }), output);
        // The residual block carries the diagnostics the stub printed, which
        // echo the spawn argv -- proving the resolved path reached the binary.
        expect(output.output).toContain(`check --fix ${join(fixtureDir, relative)}`);
      } finally {
        delete process.env.WIKI_BIN;
      }
    });
  });

  describe('injection', () => {
    it('appends a wiki context block on residual diagnostics, preserving prior output', async () => {
      const wikiPath = makeFile('page.md', '---\ntitle: T\nsummary: S\n---\nbody');
      process.env.WIKI_BIN = makeBinary('echo "line-range drift" >&2 ; exit 1');
      const hooks = assemblePlugin({ directory: fixtureDir });

      try {
        const output = afterOutput('original result');
        await hooks['tool.execute.after'](afterInput('write', { filePath: wikiPath }), output);
        expect(output.output?.startsWith('original result')).toBe(true);
        expect(output.output).toContain('\n<wiki>\n');
        expect(output.output?.endsWith('</wiki>')).toBe(true);
        expect(output.output).toContain('line-range drift');
      } finally {
        delete process.env.WIKI_BIN;
      }
    });

    it('stays silent when the check exits clean', async () => {
      const wikiPath = makeFile('page.md', '---\ntitle: T\nsummary: S\n---\nbody');
      process.env.WIKI_BIN = makeBinary('exit 0');
      const hooks = assemblePlugin({ directory: fixtureDir });

      try {
        const output = afterOutput();
        await hooks['tool.execute.after'](afterInput('edit', { filePath: wikiPath }), output);
        expect(output.output).toBe('tool ran');
      } finally {
        delete process.env.WIKI_BIN;
      }
    });
  });

  describe('fail-open contract', () => {
    it('is a silent no-op for non-wiki files', async () => {
      const plain = makeFile('plain.md', '# just markdown');
      process.env.WIKI_BIN = makeBinary('echo "should not run" ; exit 1');
      const hooks = assemblePlugin({ directory: fixtureDir });

      try {
        for (const missing of ['does-not-exist.md', plain]) {
          const output = afterOutput();
          await hooks['tool.execute.after'](afterInput('edit', { filePath: missing }), output);
          expect(output.output).toBe('tool ran');
        }
      } finally {
        delete process.env.WIKI_BIN;
      }
    });

    it('never throws when the wiki binary is missing', async () => {
      const wikiPath = makeFile('page.md', '---\ntitle: T\nsummary: S\n---\nbody');
      const hooks = assemblePlugin({
        directory: fixtureDir,
        resolveBinary: () => '/definitely/not/a/real/wiki-binary'
      });

      const output = afterOutput();
      await expect(
        hooks['tool.execute.after'](afterInput('edit', { filePath: wikiPath }), output)
      ).resolves.toBeUndefined();
      expect(output.output).toBe('tool ran');
    });

    it('never throws when the injected check itself explodes', async () => {
      const wikiPath = makeFile('page.md', '---\ntitle: T\nsummary: S\n---\nbody');
      const explode: () => WikiCheckResult = () => {
        throw new Error('executor blew up');
      };
      const hooks = assemblePlugin({ directory: fixtureDir, executeCheck: explode });

      const output = afterOutput();
      await expect(
        hooks['tool.execute.after'](afterInput('edit', { filePath: wikiPath }), output)
      ).resolves.toBeUndefined();
      expect(output.output).toBe('tool ran');
    });

    it('survives malformed host payloads without throwing', async () => {
      const hooks = assemblePlugin({ directory: fixtureDir });
      await expect(
        hooks['tool.execute.after']({} as OpencodeToolInput, {} as OpencodeAfterOutput)
      ).resolves.toBeUndefined();
      await expect(
        hooks['tool.execute.after'](undefined as unknown as OpencodeToolInput, null as unknown as OpencodeAfterOutput)
      ).resolves.toBeUndefined();
    });
  });
});
