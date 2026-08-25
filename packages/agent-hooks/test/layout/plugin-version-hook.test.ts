import { spawnSync } from 'node:child_process';
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { afterAll, describe, expect, it } from 'vitest';

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../../../..');
const hookScript = join(repoRoot, '.githooks', 'pre-commit.plugin-version.sh');

const FIXTURES = [
  'plugins-claude/wiki/.claude-plugin/plugin.json',
  'plugins-codex/wiki/.codex-plugin/plugin.json',
  'plugins-opencode/wiki/package.json',
  '.claude-plugin/marketplace.json'
];

function git(cwd: string, args: string[]): string {
  const result = spawnSync('git', args, { cwd, encoding: 'utf8' });
  expect(result.status, `git ${args.join(' ')} failed: ${result.stderr}`).toBe(0);
  return result.stdout;
}

function firstVersion(content: string): string {
  return /"version"\s*:\s*"([^"]+)"/.exec(content)?.[1] ?? '';
}

describe('pre-commit.plugin-version.sh', () => {
  const scratch = mkdtempSync(join(tmpdir(), 'plugin-version-hook-'));

  afterAll(() => {
    rmSync(scratch, { recursive: true, force: true });
  });

  it('bumps all four surfaces in one increment at the index level, sweeps nothing, exits 0, and keeps the script free of GNU-only sed addressing', {
    timeout: 10_000
  }, () => {
    for (const relative of FIXTURES) {
      const destination = join(scratch, relative);
      mkdirSync(dirname(destination), { recursive: true });
      cpSync(join(repoRoot, relative), destination);
    }
    mkdirSync(join(scratch, 'plugins-codex/wiki/hooks'), { recursive: true });

    git(scratch, ['init', '-q']);
    git(scratch, ['config', 'user.email', 'layout-test@example.invalid']);
    git(scratch, ['config', 'user.name', 'layout-test']);
    git(scratch, ['add', '-A']);
    git(scratch, ['commit', '-qm', 'seed fixtures']);

    // Trigger: a staged non-manifest change inside a plugin tree starts the bump.
    // An unrelated unstaged manifest edit must survive both sides of the staging.
    writeFileSync(join(scratch, 'plugins-codex/wiki/hooks/hooks.json'), 'x\n');
    git(scratch, ['add', 'plugins-codex/wiki/hooks/hooks.json']);
    const opencodeManifest = join(scratch, 'plugins-opencode/wiki/package.json');
    const seeded = JSON.parse(readFileSync(opencodeManifest, 'utf8')) as { description?: string };
    seeded.description = `${seeded.description ?? ''} [local-wip]`;
    writeFileSync(opencodeManifest, `${JSON.stringify(seeded, null, 2)}\n`);

    // Isolate from this repo's git context; PATH must stay so node/git are found.
    const env = { ...process.env } as Record<string, string | undefined>;
    delete env.GIT_DIR;
    delete env.GIT_WORK_TREE;
    delete env.GIT_INDEX_FILE;
    delete env.GIT_OBJECT_DIRECTORY;

    const hook = spawnSync('bash', [hookScript], { cwd: scratch, env, encoding: 'utf8' });
    expect(hook.status, `hook failed: ${hook.stderr}`).toBe(0);

    const indexed = (relative: string): string =>
      spawnSync('git', ['show', `:${relative}`], { cwd: scratch, encoding: 'utf8' }).stdout;

    // (a) one shared increment across all three manifests + marketplace surfaces, in the INDEX
    expect(firstVersion(indexed(FIXTURES[0]))).toBe('0.5.125');
    expect(firstVersion(indexed(FIXTURES[1]))).toBe('0.5.125');
    expect(firstVersion(indexed(FIXTURES[2]))).toBe('0.5.125');
    const indexedMarketplace = indexed(FIXTURES[3]);
    expect(firstVersion(indexedMarketplace)).toBe('1.0.73'); // metadata.version rides one increment too
    const marketplaceDoc = JSON.parse(indexedMarketplace) as { plugins?: { name: string; version?: string }[] };
    expect(marketplaceDoc.plugins?.[0]).toMatchObject({ name: 'wiki', version: '0.5.125' });

    // (b) the unrelated unstaged edit survives: present in the worktree, absent from the index blob
    const worktreeManifest = readFileSync(opencodeManifest, 'utf8');
    expect(worktreeManifest).toContain('[local-wip]');
    expect(worktreeManifest).toContain('"version": "0.5.125"');
    const indexBlob = indexed(FIXTURES[2]);
    expect(indexBlob).toContain('"version": "0.5.125"');
    expect(indexBlob).not.toContain('[local-wip]');

    // (d) static portability guard: the first-match sed address form that BSD
    // cannot compile must stay out of the script entirely, and the portable
    // node-based mechanism must still be present.
    const source = readFileSync(hookScript, 'utf8');
    expect(source.match(/0,\//g)?.length ?? 0).toBe(0);
    expect(source).toContain('bump_first_marketplace_version');
  });
});
