import { spawnSync } from 'node:child_process';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../../../..');
const packageDir = resolve(repoRoot, 'packages/agent-hooks');
const generatedPaths = ['plugins-claude/wiki/hooks', 'plugins-codex/wiki/hooks', 'plugins-opencode/wiki/dist'];

function porcelainOverGeneratedPaths(): string {
  const status = spawnSync('git', ['-C', repoRoot, 'status', '--porcelain', '--', ...generatedPaths], {
    encoding: 'utf8'
  });
  if (status.error) throw status.error;
  if (status.status !== 0) throw new Error(`git status failed: ${status.stderr}`);
  return status.stdout;
}

describe('generated plugin bundles are fresh', () => {
  it('a full rebuild leaves git status --porcelain empty over the generated paths', { timeout: 300_000 }, () => {
    const build = spawnSync('yarn', ['build'], { cwd: packageDir, encoding: 'utf8', timeout: 200_000 });
    if (build.error) throw build.error;
    expect(build.status, `yarn build failed:\n${build.stderr}`).toBe(0);

    const dirty = porcelainOverGeneratedPaths();
    expect(dirty, `rebuild produced uncommitted bundle changes — commit the rebuilt plugin bundles:\n${dirty}`).toBe(
      ''
    );
  });
});
