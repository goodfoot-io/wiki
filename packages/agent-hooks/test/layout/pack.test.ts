import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { beforeAll, describe, expect, it } from 'vitest';

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../../../..');
const packageDir = resolve(repoRoot, 'packages/agent-hooks');

interface PackedTarball {
  path: string;
  entries: string[];
  listing: string[];
}

let packed: PackedTarball;

beforeAll(() => {
  const stdout = execFileSync('node', ['scripts/pack-opencode.mjs'], { cwd: packageDir, encoding: 'utf8' });
  const tarballPath = stdout
    .split('\n')
    .map((line) => line.trim())
    .filter(Boolean)
    .pop();
  expect(tarballPath, 'pack-opencode.mjs printed no tarball path').toEqual(expect.any(String));

  const entries = execFileSync('tar', ['-tf', tarballPath as string], { encoding: 'utf8' })
    .split('\n')
    .filter(Boolean);
  const listing = execFileSync('tar', ['-tvf', tarballPath as string], { encoding: 'utf8' })
    .split('\n')
    .filter(Boolean);

  packed = { path: tarballPath as string, entries, listing };
}, 120_000);

function requireEntry(name: string): void {
  expect(packed.entries, `tarball is missing ${name}; has:\n${packed.entries.join('\n')}`).toContain(name);
}

describe('opencode plugin tarball layout', () => {
  it('contains the built bundle, the installer bin, and the skill entrypoint', () => {
    requireEntry('package/dist/index.mjs');
    requireEntry('package/bin/opencode-wiki.mjs');
    requireEntry('package/skills/wiki/SKILL.md');
  });

  it('carries the real SKILL.md bytes (dereferenced symlink)', () => {
    const packedBytes = execFileSync('tar', ['-xOf', packed.path, 'package/skills/wiki/SKILL.md']);
    const sourceBytes = readFileSync(resolve(repoRoot, 'skills/wiki/SKILL.md'));
    expect(packedBytes.equals(sourceBytes)).toBe(true);
  });

  it('contains no symlink entries', () => {
    const symlinks = packed.listing.filter((line) => line.startsWith('l'));
    expect(symlinks, `symlink entries found in tarball:\n${symlinks.join('\n')}`).toEqual([]);
  });
});
