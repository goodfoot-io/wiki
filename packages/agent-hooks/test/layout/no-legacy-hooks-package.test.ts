import { spawnSync } from 'node:child_process';
import { lstatSync, readFileSync, readlinkSync } from 'node:fs';
import { dirname, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../../../..');
// This scanner's own source necessarily spells out both legacy package names
// literally (to define what it looks for), so it is excluded from the scan
// by construction rather than via the exception list -- that list is for
// legitimate references in *other* tracked files, not the detector itself.
const selfPath = relative(repoRoot, fileURLToPath(import.meta.url));

const LEGACY_PACKAGES = ['@goodfoot/claude-code-hooks', '@goodfoot/codex-hooks'];

interface Exception {
  path: string;
  occurrences: number;
}

function listTrackedPaths(): string[] {
  const result = spawnSync('git', ['-C', repoRoot, 'ls-files'], { encoding: 'utf8' });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`git ls-files failed: ${result.stderr}`);
  return result.stdout.split('\n').filter(Boolean);
}

function readTrackedContent(relativePath: string): string {
  const absPath = resolve(repoRoot, relativePath);
  const stats = lstatSync(absPath);
  if (stats.isSymbolicLink()) return readlinkSync(absPath);
  if (stats.isFile()) return readFileSync(absPath, 'utf8');
  throw new Error(`Tracked path is neither a symlink nor a regular file: ${relativePath}`);
}

function countOccurrences(content: string, needle: string): number {
  return content.split(needle).length - 1;
}

// Scans { path -> content } for un-excepted legacy-package references,
// returning one description string per offense. Shared by the real
// git-tracked scan and the fixture-backed fail-case below, so both exercise
// the identical detection logic.
function findOffenders(entries: Array<{ path: string; content: string }>, exceptions: Exception[]): string[] {
  const offenders: string[] = [];
  for (const { path, content } of entries) {
    for (const legacyPackage of LEGACY_PACKAGES) {
      const occurrences = countOccurrences(content, legacyPackage);
      if (occurrences === 0) continue;

      const exception = exceptions.find((e) => e.path === path);
      if (exception && exception.occurrences === occurrences) continue;

      offenders.push(`${path}: ${occurrences}x "${legacyPackage}"`);
    }
  }
  return offenders;
}

describe('no tracked file references either superseded hooks package', () => {
  it('finds zero un-excepted references to @goodfoot/claude-code-hooks or @goodfoot/codex-hooks', () => {
    // Nothing legitimate references either legacy package today -- start empty.
    const exceptions: Exception[] = [];
    const entries = listTrackedPaths()
      .filter((path) => path !== selfPath)
      .map((path) => ({ path, content: readTrackedContent(path) }));
    const offenders = findOffenders(entries, exceptions);
    expect(offenders, `un-excepted legacy package references found:\n${offenders.join('\n')}`).toEqual([]);
  });

  it('fires on a legacy reference (in-memory fixture, not a real tracked file)', () => {
    const fixtures = [
      { path: 'fixtures/still-on-legacy-package.ts', content: "import x from '@goodfoot/claude-code-hooks';\n" }
    ];
    const offenders = findOffenders(fixtures, []);
    expect(offenders).toEqual(['fixtures/still-on-legacy-package.ts: 1x "@goodfoot/claude-code-hooks"']);
  });

  it('reads git-tracked symlinks-to-directories via readlinkSync, not a dereferencing read', () => {
    const trackedSymlinkDirs = [
      '.agents/skills/wiki',
      'plugins-claude/wiki/skills/wiki',
      'plugins-codex/wiki/skills/wiki',
      'plugins-opencode/wiki/skills/wiki'
    ];
    const tracked = new Set(listTrackedPaths());
    const present = trackedSymlinkDirs.filter((path) => tracked.has(path));
    expect(present.length).toBeGreaterThan(0);

    for (const path of present) {
      expect(lstatSync(resolve(repoRoot, path)).isSymbolicLink()).toBe(true);
      // Must not throw EISDIR -- a naive readFileSync would dereference into
      // the target directory and fail exactly this way.
      expect(() => readTrackedContent(path)).not.toThrow();
    }
  });
});
