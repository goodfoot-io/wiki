import { spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../../../..');

const FIXTURE_HOOK = `import { postToolUseHook } from '@goodfoot/agent-hooks/claude-code';
export default postToolUseHook({ matcher: 'Edit|Write|NotebookEdit', timeout: 60000 }, (input, { logger }) => {
  logger.info('fixture', {});
  return null;
});
`;

interface BuildResult {
  bundle: string;
  manifest: Record<string, unknown>;
}

function buildFixture(cliEntryPath: string, root: string): BuildResult {
  mkdirSync(join(root, 'src'), { recursive: true });
  mkdirSync(join(root, 'out'), { recursive: true });
  writeFileSync(join(root, 'src', 'hook.ts'), FIXTURE_HOOK, 'utf-8');

  const result = spawnSync(
    process.execPath,
    [cliEntryPath, '--agent', 'claude-code', '-i', 'src/hook.ts', '-o', 'out/hooks.json', '--no-sourcemap'],
    { cwd: root, encoding: 'utf-8' }
  );
  expect(result.status, `CLI build failed: ${result.stderr}`).toBe(0);

  const bundle = readFileSync(join(root, 'out', 'bin', 'hook.mjs'), 'utf-8');
  const manifest = JSON.parse(readFileSync(join(root, 'out', 'hooks.json'), 'utf-8'));
  return { bundle, manifest };
}

// Strips only the fields the wrapper's own canonicalizeHookManifest already
// normalizes deterministically (timestamp) -- nothing else should differ.
function stripNondeterministicFields(manifest: Record<string, unknown>): unknown {
  const generated = manifest.__generated as Record<string, unknown> | undefined;
  if (generated && 'timestamp' in generated) {
    const { timestamp, ...rest } = generated;
    return { ...manifest, __generated: rest };
  }
  return manifest;
}

describe('build output is byte-stable across symlinked and non-symlinked node_modules layouts', () => {
  it('produces an identical bundle and manifest through this worktree and a real npm install', () => {
    // The "symlinked" side: a scratch root whose own node_modules is a
    // symlink to this repo's real, installed node_modules -- placed directly
    // under the root (same depth as the npm-installed side below) so the
    // only variable under test is symlinked vs. real, not resolution depth.
    const symlinkedRoot = mkdtempSync(join(tmpdir(), 'symlinked-build-'));
    symlinkSync(resolve(repoRoot, 'node_modules'), join(symlinkedRoot, 'node_modules'), 'dir');
    const symlinkedCli = resolve(repoRoot, 'node_modules/@goodfoot/agent-hooks/dist/cli.js');

    // A real `npm install` produces a fully real, non-symlinked node_modules
    // tree with its own resolved transitive dependencies -- copying just the
    // installed package's own directory would miss dependencies hoisted
    // elsewhere in this workspace, so a fresh install is the only faithful
    // non-symlinked comparison.
    const npmRoot = mkdtempSync(join(tmpdir(), 'npm-install-build-'));
    writeFileSync(join(npmRoot, 'package.json'), JSON.stringify({ name: 'scratch', private: true }), 'utf-8');
    const install = spawnSync(
      'npm',
      ['install', '@goodfoot/agent-hooks@1.0.0', '--no-save', '--no-audit', '--no-fund'],
      {
        cwd: npmRoot,
        encoding: 'utf-8'
      }
    );
    expect(install.status, `npm install failed: ${install.stderr}`).toBe(0);
    const npmCli = resolve(npmRoot, 'node_modules/@goodfoot/agent-hooks/dist/cli.js');

    try {
      const symlinked = buildFixture(symlinkedCli, symlinkedRoot);
      const npmInstalled = buildFixture(npmCli, npmRoot);

      expect(npmInstalled.bundle).toBe(symlinked.bundle);
      expect(stripNondeterministicFields(npmInstalled.manifest)).toEqual(
        stripNondeterministicFields(symlinked.manifest)
      );
    } finally {
      rmSync(symlinkedRoot, { recursive: true, force: true });
      rmSync(npmRoot, { recursive: true, force: true });
    }
  }, 30000);
});
