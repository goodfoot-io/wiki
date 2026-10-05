import { spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../../../..');
const installedPackageDir = resolve(repoRoot, 'node_modules/@goodfoot/agent-hooks');

const FIXTURE_HOOK = `import { postToolUseHook } from '@goodfoot/agent-hooks/claude-code';
export default postToolUseHook({ matcher: 'Edit|Write|NotebookEdit', timeout: 60000 }, (input, { logger }) => {
  logger.info('fixture', {});
  return null;
});
`;

/** Every file a claude-code build emits, read as raw text for byte comparison. */
interface BuildResult {
  bundle: string;
  manifest: string;
  /** The CLI's tracking sidecar: the generated filenames, beside hooks.json. */
  meta: string;
}

/**
 * The exact `@goodfoot/agent-hooks` version this workspace resolved. The
 * npm-installed side must install this same version, or the comparison
 * measures a library upgrade instead of the install layout.
 */
function installedVersion(): string {
  const manifest: unknown = JSON.parse(readFileSync(join(installedPackageDir, 'package.json'), 'utf-8'));
  if (typeof manifest !== 'object' || manifest === null || !('version' in manifest)) {
    throw new Error('installed @goodfoot/agent-hooks package.json declares no version');
  }
  const { version } = manifest;
  if (typeof version !== 'string') throw new Error('installed @goodfoot/agent-hooks version is not a string');
  return version;
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

  return {
    bundle: readFileSync(join(root, 'out', 'bin', 'hook.mjs'), 'utf-8'),
    manifest: readFileSync(join(root, 'out', 'hooks.json'), 'utf-8'),
    meta: readFileSync(join(root, 'out', 'hooks.meta.json'), 'utf-8')
  };
}

describe('build output is byte-stable across symlinked and non-symlinked node_modules layouts', () => {
  it('produces an identical bundle, manifest and sidecar through this worktree and a real npm install', () => {
    // The "symlinked" side: a scratch root whose own node_modules is a
    // symlink to this repo's real, installed node_modules -- placed directly
    // under the root (same depth as the npm-installed side below) so the
    // only variable under test is symlinked vs. real, not resolution depth.
    const symlinkedRoot = mkdtempSync(join(tmpdir(), 'symlinked-build-'));
    symlinkSync(resolve(repoRoot, 'node_modules'), join(symlinkedRoot, 'node_modules'), 'dir');
    const symlinkedCli = join(installedPackageDir, 'dist/cli.js');

    // A real `npm install` produces a fully real, non-symlinked node_modules
    // tree with its own resolved transitive dependencies -- copying just the
    // installed package's own directory would miss dependencies hoisted
    // elsewhere in this workspace, so a fresh install is the only faithful
    // non-symlinked comparison.
    const npmRoot = mkdtempSync(join(tmpdir(), 'npm-install-build-'));
    writeFileSync(join(npmRoot, 'package.json'), JSON.stringify({ name: 'scratch', private: true }), 'utf-8');
    const install = spawnSync(
      'npm',
      ['install', `@goodfoot/agent-hooks@${installedVersion()}`, '--no-save', '--no-audit', '--no-fund'],
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

      // Byte equality with no normalization: the CLI stamps no build time
      // anywhere, so every emitted file must depend only on its inputs.
      expect(npmInstalled.bundle).toBe(symlinked.bundle);
      expect(npmInstalled.manifest).toBe(symlinked.manifest);
      expect(npmInstalled.meta).toBe(symlinked.meta);

      // Pin what "identical" is measured over, so a format change cannot make
      // the comparison vacuous: the sidecar declares exactly the one bundle,
      // and the manifest carries the registration only.
      expect(JSON.parse(symlinked.meta)).toStrictEqual({ files: ['hook.mjs'] });
      expect(Object.keys(JSON.parse(symlinked.manifest))).toEqual(['hooks']);
    } finally {
      rmSync(symlinkedRoot, { recursive: true, force: true });
      rmSync(npmRoot, { recursive: true, force: true });
    }
  }, 30000);
});
