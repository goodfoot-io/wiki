import { chmodSync, mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import {
  compareSemver,
  extractPatchedFilePaths,
  findManagedWikiBinary,
  runWikiCheck,
  WIKI_EXECUTABLE,
  type WikiCheckLogger
} from '../../src/common/wiki-check.js';

let fixtureDir: string | undefined;
let counter = 0;

function makeBinary(script: string): string {
  if (!fixtureDir) fixtureDir = mkdtempSync(join(tmpdir(), `wiki-check-binaries-`));
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
    const workdir = mkdtempSync(join(tmpdir(), `wiki-check-cwd-`));
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

describe('findManagedWikiBinary', () => {
  const originalHome = process.env.HOME;
  let home: string | undefined;

  afterEach(() => {
    if (originalHome === undefined) delete process.env.HOME;
    else process.env.HOME = originalHome;
    if (home) {
      rmSync(home, { recursive: true, force: true });
      home = undefined;
    }
  });

  function recordingLogger(): { probe: WikiCheckLogger; warnings: Array<{ message: string; context?: unknown }> } {
    const warnings: Array<{ message: string; context?: unknown }> = [];
    return {
      probe: { info: () => undefined, warn: (message, context) => warnings.push({ message, context }) },
      warnings
    };
  }

  /** A fake home whose VS Code globalStorage holds the extension's managed `bin` root. */
  function managedBinRoot(): string {
    home = mkdtempSync(join(tmpdir(), 'managed-wiki-home-'));
    process.env.HOME = home;
    const binRoot = join(home, '.config', 'Code', 'User', 'globalStorage', 'goodfoot.wiki-extension', 'bin');
    mkdirSync(binRoot, { recursive: true });
    return binRoot;
  }

  function installBinary(binRoot: string, version: string): string {
    const targetDir = join(binRoot, version, 'linux-x64');
    mkdirSync(targetDir, { recursive: true });
    const binary = join(targetDir, WIKI_EXECUTABLE);
    writeFileSync(binary, '#!/bin/sh\nexit 0\n', 'utf-8');
    chmodSync(binary, 0o755);
    return binary;
  }

  it('skips stray files and vanished symlinks silently, picking the newest real install', () => {
    const binRoot = managedBinRoot();
    installBinary(binRoot, '1.0.0');
    const newest = installBinary(binRoot, '1.2.0');
    writeFileSync(join(binRoot, '.DS_Store'), 'junk', 'utf-8');
    symlinkSync(join(binRoot, 'does-not-exist'), join(binRoot, '9.9.9'));
    const { probe, warnings } = recordingLogger();

    expect(findManagedWikiBinary(probe)).toBe(newest);
    expect(warnings).toHaveLength(0);
  });

  it('reports an unreadable version directory, then falls through to older installs', () => {
    const binRoot = managedBinRoot();
    const older = installBinary(binRoot, '1.0.0');
    // A self-referential symlink lists as a directory candidate but fails to
    // read with ELOOP — an unexpected error that must not vanish silently.
    const looping = join(binRoot, '9.9.9');
    symlinkSync(looping, looping);
    const { probe, warnings } = recordingLogger();

    expect(findManagedWikiBinary(probe)).toBe(older);
    expect(warnings).toHaveLength(1);
    expect(warnings[0]?.message).toContain('managed wiki binary directory unreadable');
    expect(JSON.stringify(warnings[0]?.context)).toContain(looping);
  });
});
