import { chmodSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import { compareSemver, extractPatchedFilePaths, runWikiCheck } from '../../src/common/wiki-check.js';

let fixtureDir: string | undefined;
let counter = 0;

function makeBinary(script: string): string {
  if (!fixtureDir) fixtureDir = mkdirSync(join(tmpdir(), `wiki-check-binaries-`), { recursive: true });
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

describe('runWikiCheck', () => {
  it('reports clean on exit 0', () => {
    const binary = makeBinary('exit 0');
    expect(runWikiCheck('/some/file.md', { binary })).toEqual({ status: 'clean' });
  });

  it('surfaces non-zero exits as residual with combined output', () => {
    const binary = makeBinary('echo "line-range drift" ; exit 1');
    const result = runWikiCheck('/some/file.md', { binary });
    expect(result.status).toBe('residual');
    expect(result.output).toContain('line-range drift');
  });

  it('reports residual even when a failing run printed nothing', () => {
    const binary = makeBinary('exit 1');
    expect(runWikiCheck('/some/file.md', { binary })).toEqual({ status: 'residual' });
  });

  it('spawns the wiki check contract argv (`check --fix <file>`)', () => {
    const binary = makeBinary('echo "argv: $@" ; exit 1');
    const result = runWikiCheck('/some/file.md', { binary });
    expect(result.output).toBe('argv: check --fix /some/file.md');
  });

  it('runs the child in the requested cwd', () => {
    const workdir = mkdirSync(join(tmpdir(), `wiki-check-cwd-`), { recursive: true });
    try {
      const binary = makeBinary('pwd ; exit 1');
      const result = runWikiCheck('/some/file.md', { binary, cwd: workdir });
      expect(result.status).toBe('residual');
      expect(result.output).toBe(workdir);
    } finally {
      rmSync(workdir, { recursive: true, force: true });
    }
  });

  it('classifies an unlaunchable binary as unavailable instead of throwing', () => {
    const result = runWikiCheck('/some/file.md', { binary: '/definitely/not/a/real/wiki-binary' });
    expect(result.status).toBe('unavailable');
    expect(result.output).toBeTruthy();
  });

  it('never rejects: every failure mode lands in the result', () => {
    const binary = makeBinary('kill -TERM $$');
    expect(() => runWikiCheck('/some/file.md', { binary })).not.toThrow();
  });
});

describe('compareSemver', () => {
  it('orders dotted numeric versions', () => {
    expect(compareSemver('0.5.9', '0.5.10')).toBeLessThan(0);
    expect(compareSemver('0.5.10', '0.5.9')).toBeGreaterThan(0);
    expect(compareSemver('1.0.0', '1.0.0')).toBe(0);
    expect(compareSemver('0.6', '0.5.74')).toBeGreaterThan(0);
  });
});

describe('extractPatchedFilePaths', () => {
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

  it('returns nothing for a patch that declares no files', () => {
    expect(extractPatchedFilePaths('*** Begin Patch\n*** End Patch')).toEqual([]);
  });
});
