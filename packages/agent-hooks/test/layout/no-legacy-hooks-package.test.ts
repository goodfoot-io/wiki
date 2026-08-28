import { spawnSync } from 'node:child_process';
import { lstatSync, mkdtempSync, readFileSync, readlinkSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { afterEach, describe, expect, it } from 'vitest';

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

/** One tracked path's occurrence count for a single legacy package name. */
interface Hit {
  path: string;
  legacyPackage: string;
  occurrences: number;
}

function listTrackedPaths(cwd: string = repoRoot): string[] {
  // -z terminates entries with NUL and disables git's default core.quotepath
  // octal-escaping of non-ASCII bytes, so paths come back raw regardless of
  // repo config.
  const result = spawnSync('git', ['-C', cwd, 'ls-files', '-z'], { encoding: 'utf8' });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`git ls-files failed: ${result.stderr}`);
  return result.stdout.split('\0').filter(Boolean);
}

function readTrackedContent(relativePath: string, cwd: string = repoRoot): string {
  const absPath = resolve(cwd, relativePath);
  const stats = lstatSync(absPath);
  if (stats.isSymbolicLink()) return readlinkSync(absPath);
  if (stats.isFile()) return readFileSync(absPath, 'utf8');
  throw new Error(`Tracked path is neither a symlink nor a regular file: ${relativePath}`);
}

function countOccurrences(content: string, needle: string): number {
  return content.split(needle).length - 1;
}

/**
 * Counts legacy-package occurrences across every tracked file in a single
 * `git grep`.
 *
 * Reading each tracked path from Node instead costs one lstat plus one read per
 * file, which on a container filesystem measured ~7.8 s for this repo's ~400
 * tracked files -- past this test's timeout, and dominated by per-file syscall
 * overhead rather than bytes, so no amount of skipping large files fixes it.
 * `git grep` does the same traversal in a single process.
 *
 * `-I` skips binary files. Nothing tracked here is binary today, and a legacy
 * reference compiled into an artifact is not a site anyone could edit anyway --
 * the sources it was built from are covered in full.
 *
 * `-o` prints one line per match rather than per matching line, so two
 * references on one line are counted as two.
 */
function scanTrackedForLegacyPackages(cwd: string = repoRoot): Hit[] {
  // Both names go in one invocation: the traversal, not the matching, is what
  // costs, so a second pass would double the wall time for nothing. `-o` makes
  // each line `<path>\0<matched text>`, which attributes the hit without
  // needing a pass per name.
  const result = spawnSync(
    'git',
    ['-C', cwd, 'grep', '-I', '-F', '-o', '-z', ...LEGACY_PACKAGES.flatMap((name) => ['-e', name])],
    { encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 }
  );
  if (result.error) throw result.error;
  // git grep exits 1 for "no matches found", which is the healthy case here;
  // anything above that is a real failure and must not read as clean.
  if (result.status !== 0 && result.status !== 1) {
    throw new Error(`git grep failed (exit ${result.status}): ${result.stderr}`);
  }

  const counts = new Map<string, number>();
  for (const line of result.stdout.split('\n')) {
    if (!line) continue;
    // With -z the path is NUL-terminated, separating it unambiguously from a
    // path containing a colon.
    const nul = line.indexOf('\0');
    if (nul === -1) throw new Error(`unexpected git grep -z output line: ${line}`);
    const key = `${line.slice(0, nul)}\0${line.slice(nul + 1)}`;
    counts.set(key, (counts.get(key) ?? 0) + 1);
  }

  return [...counts].map(([key, occurrences]) => {
    const [path, legacyPackage] = key.split('\0');
    return { path: path as string, legacyPackage: legacyPackage as string, occurrences };
  });
}

// Applies the exception policy to counted hits, returning one description
// string per offense. Shared by the real git-tracked scan and the fixture-backed
// fail-case below, so both exercise the identical policy.
function findOffenders(hits: Hit[], exceptions: Exception[]): string[] {
  const offenders: string[] = [];
  for (const { path, legacyPackage, occurrences } of hits) {
    if (occurrences === 0) continue;

    const exception = exceptions.find((e) => e.path === path);
    if (exception && exception.occurrences === occurrences) continue;

    offenders.push(`${path}: ${occurrences}x "${legacyPackage}"`);
  }
  return offenders;
}

/** Counts a set of in-memory files the way the fixture cases describe them. */
function hitsFromContents(entries: Array<{ path: string; content: string }>): Hit[] {
  return entries.flatMap(({ path, content }) =>
    LEGACY_PACKAGES.map((legacyPackage) => ({
      path,
      legacyPackage,
      occurrences: countOccurrences(content, legacyPackage)
    })).filter((hit) => hit.occurrences > 0)
  );
}

describe('no tracked file references either superseded hooks package', () => {
  let scratchRepo: string | undefined;

  afterEach(() => {
    if (scratchRepo) rmSync(scratchRepo, { recursive: true, force: true });
    scratchRepo = undefined;
  });

  it('listTrackedPaths/readTrackedContent handle a non-ASCII tracked filename', () => {
    // Regression for git's default core.quotepath=true octal-escaping
    // non-ASCII tracked paths (e.g. "café.md" -> "caf\303\251.md"), which
    // used to make readTrackedContent resolve a bogus path and throw ENOENT.
    scratchRepo = mkdtempSync(join(tmpdir(), 'agent-hooks-quotepath-'));
    spawnSync('git', ['init', '-q', scratchRepo]);
    spawnSync('git', ['-C', scratchRepo, 'config', 'user.email', 'test@example.com']);
    spawnSync('git', ['-C', scratchRepo, 'config', 'user.name', 'test']);
    const nonAsciiName = 'café.md';
    writeFileSync(join(scratchRepo, nonAsciiName), 'hello');
    spawnSync('git', ['-C', scratchRepo, 'add', '.']);
    spawnSync('git', ['-C', scratchRepo, 'commit', '-q', '-m', 'init']);

    const tracked = listTrackedPaths(scratchRepo);
    expect(tracked).toEqual([nonAsciiName]);
    expect(readTrackedContent(nonAsciiName, scratchRepo)).toBe('hello');
  });

  it('the git grep scan counts every occurrence, including a non-ASCII path and two on one line', () => {
    // Pins the scanner against a real git repository rather than trusting that
    // its output shape matches what findOffenders expects. Two references on a
    // single line prove `-o` counts matches, not matching lines.
    scratchRepo = mkdtempSync(join(tmpdir(), 'agent-hooks-grep-'));
    spawnSync('git', ['init', '-q', scratchRepo]);
    spawnSync('git', ['-C', scratchRepo, 'config', 'user.email', 'test@example.com']);
    spawnSync('git', ['-C', scratchRepo, 'config', 'user.name', 'test']);
    writeFileSync(join(scratchRepo, 'café.ts'), `import '${LEGACY_PACKAGES[0]}'; // also ${LEGACY_PACKAGES[0]}\n`);
    writeFileSync(join(scratchRepo, 'clean.ts'), 'export const ok = true;\n');
    spawnSync('git', ['-C', scratchRepo, 'add', '.']);
    spawnSync('git', ['-C', scratchRepo, 'commit', '-q', '-m', 'init']);

    expect(scanTrackedForLegacyPackages(scratchRepo)).toEqual([
      { path: 'café.ts', legacyPackage: LEGACY_PACKAGES[0], occurrences: 2 }
    ]);
  });

  // `git grep` over this repo is I/O-bound on a container filesystem (~1.6 s,
  // 7% CPU) and vitest's 5 s default leaves no headroom on a cold cache.
  it('finds zero un-excepted references to @goodfoot/claude-code-hooks or @goodfoot/codex-hooks', {
    timeout: 60_000
  }, () => {
    // Nothing legitimate references either legacy package today -- start empty.
    const exceptions: Exception[] = [];
    const hits = scanTrackedForLegacyPackages().filter((hit) => hit.path !== selfPath);
    const offenders = findOffenders(hits, exceptions);
    expect(offenders, `un-excepted legacy package references found:\n${offenders.join('\n')}`).toEqual([]);
  });

  it('fires on a legacy reference (in-memory fixture, not a real tracked file)', () => {
    const fixtures = [
      { path: 'fixtures/still-on-legacy-package.ts', content: "import x from '@goodfoot/claude-code-hooks';\n" }
    ];
    const offenders = findOffenders(hitsFromContents(fixtures), []);
    expect(offenders).toEqual(['fixtures/still-on-legacy-package.ts: 1x "@goodfoot/claude-code-hooks"']);
  });

  it('reads git-tracked symlinks-to-directories via readlinkSync, not a dereferencing read', () => {
    // The per-plugin skills trees became real generated directories when the
    // wiki skill moved onto @goodfoot/agent-skills; .agents/skills is where
    // tracked symlinks-to-directories still live.
    const trackedSymlinkDirs = ['.agents/skills/wiki', '.agents/skills/wiki-cli-analysis'];
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
