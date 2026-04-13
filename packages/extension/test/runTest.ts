/**
 * Wiki extension test runner.
 *
 * Mirrors the structure of packages/extension/test/runTest.ts with one difference:
 * if the wiki binary is not found on PATH, the test runner exits with code 1 instead
 * of 0. Per CLAUDE.md golden rule: "A test that does not run because of an
 * infrastructure error is a blocking condition."
 *
 * Supports headless Linux environments via Xvfb and uses a unique dist directory
 * per test run to enable safe parallel execution.
 *
 * @summary VS Code extension test runner for the wiki-extension package.
 */

import * as cp from 'node:child_process';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { fileURLToPath } from 'node:url';
import { runTests } from '@vscode/test-electron';

/**
 * Extension root directory (where package.json lives).
 * Resolved early so it is available for TEST_DIST_PATH calculation.
 */
const EXTENSION_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

/**
 * Unique instance ID for this test run.
 * Used to create unique paths for parallel test execution.
 */
const INSTANCE_ID = process.pid;

/**
 * Dynamic paths that are unique per test run instance.
 * These enable multiple test runs to execute in parallel.
 */
const TEST_WORKSPACE_PATH = `/tmp/wiki-ext-test-workspace-${INSTANCE_ID}`;
const USER_DATA_DIR_PATH = `/tmp/wiki-ext-test-${INSTANCE_ID}`;

/**
 * Unique dist directory for this test run.
 * Each test run builds to its own directory to prevent interference with other
 * concurrent test runs and the production build.
 */
const TEST_DIST_PATH = path.join(EXTENSION_ROOT, `dist-test-${INSTANCE_ID}`);

/**
 * Mutable state shared between setup and cleanup.
 */
const state = {
  assignedDisplay: null as number | null,
  xvfbProcess: null as cp.ChildProcess | null,
  cleanupDone: false
};

/**
 * Check whether the wiki binary is available on PATH.
 *
 * @returns True if wiki is found, false otherwise.
 */
function hasWikiBinary(): boolean {
  try {
    cp.execSync('which wiki', { encoding: 'utf-8' });
    return true;
  } catch {
    return false;
  }
}

/**
 * Reads the minimum VS Code version from package.json engines.vscode field.
 * Strips semver range prefixes (^, ~, >=) to get the base version number.
 *
 * @returns The minimum VS Code version (e.g., "1.101.0").
 */
function getMinVSCodeVersion(): string {
  const packageJsonPath = path.join(EXTENSION_ROOT, 'package.json');
  const packageJson = JSON.parse(fs.readFileSync(packageJsonPath, 'utf-8')) as {
    engines?: { vscode?: string };
  };
  const vscodeRange = packageJson.engines?.vscode ?? '^1.101.0';
  return vscodeRange.replace(/^[\^~>=]+/, '');
}

/**
 * Detect headless environment (CI, no DISPLAY).
 *
 * @returns True when running headless.
 */
function isHeadless(): boolean {
  return !!(
    process.env['CI'] ||
    process.env['GITHUB_ACTIONS'] ||
    process.env['HEADLESS'] ||
    (os.platform() === 'linux' && !process.env['DISPLAY'])
  );
}

/**
 * Find an available X display number for Xvfb.
 *
 * @returns Available display number in range 99-199.
 * @throws Error when no display in range 99-199 is available.
 */
function findAvailableDisplay(): number {
  for (let display = 99; display < 200; display++) {
    const lockFile = `/tmp/.X${display}-lock`;
    if (!fs.existsSync(lockFile)) {
      return display;
    }
  }
  throw new Error('No available X display found in range 99-199');
}

/**
 * Remove X11 lock files left by a previous Xvfb run.
 */
function cleanX11LockFiles(): void {
  const displayNum = state.assignedDisplay;
  if (displayNum === null) return;

  for (const filePath of [`/tmp/.X${displayNum}-lock`, `/tmp/.X11-unix/X${displayNum}`]) {
    try {
      fs.unlinkSync(filePath);
    } catch (err) {
      if (err && typeof err === 'object' && 'code' in err && (err as NodeJS.ErrnoException).code !== 'ENOENT') {
        console.warn(`[runTest] Failed to remove ${filePath}:`, err);
      }
    }
  }
}

/**
 * Start Xvfb virtual display for Linux headless testing.
 *
 * @returns The Xvfb child process, or null if not needed.
 * @throws Error when no display is available or Xvfb fails to start.
 */
function startXvfb(): cp.ChildProcess | null {
  if (os.platform() !== 'linux' || process.env['DISPLAY']) {
    return null;
  }

  const display = findAvailableDisplay();
  state.assignedDisplay = display;
  const displayStr = `:${display}`;

  let xvfbError = '';
  const xvfb = cp.spawn(
    'Xvfb',
    [
      displayStr,
      '-screen',
      '0',
      '1024x768x24',
      '-ac',
      '-nolisten',
      'tcp',
      '+extension',
      'GLX',
      '+extension',
      'RANDR',
      '+extension',
      'RENDER'
    ],
    { detached: false, stdio: ['ignore', 'ignore', 'pipe'] }
  );

  if (xvfb.stderr) {
    xvfb.stderr.on('data', (data: Buffer) => {
      xvfbError += data.toString();
    });
  }
  xvfb.on('exit', () => {});
  xvfb.on('error', (err) => {
    console.error('[runTest] Xvfb error:', err.message);
  });

  process.env['DISPLAY'] = displayStr;
  process.env['ELECTRON_DISABLE_SANDBOX'] = '1';
  process.env['ELECTRON_DISABLE_GPU_SANDBOX'] = '1';
  process.env['ELECTRON_DISABLE_SECURITY_WARNINGS'] = '1';
  process.env['HEADLESS'] = '1';

  // Wait up to 10 s for Xvfb to be ready.
  let displayReady = false;
  for (let i = 0; i < 10; i++) {
    cp.execSync('sleep 1');
    if (xvfb.exitCode !== null) {
      throw new Error(`Xvfb exited with code ${xvfb.exitCode}. Error: ${xvfbError}`);
    }
    try {
      cp.execSync(`xdpyinfo -display ${displayStr} > /dev/null 2>&1`);
      displayReady = true;
      break;
    } catch (_ignored) {
      void _ignored;
    }
  }

  if (!displayReady) {
    xvfb.kill('SIGTERM');
    throw new Error(`Xvfb display ${displayStr} not accessible after 10 s`);
  }

  return xvfb;
}

/**
 * Clean up Xvfb and temp directories. Safe to call multiple times.
 */
function performCleanup(): void {
  if (state.cleanupDone) return;
  state.cleanupDone = true;

  if (state.xvfbProcess != null) {
    try {
      state.xvfbProcess.kill('SIGTERM');
    } catch (err) {
      if (err && typeof err === 'object' && 'code' in err && (err as NodeJS.ErrnoException).code !== 'ESRCH') {
        console.warn('[runTest] Failed to kill Xvfb:', err);
      }
    }
  }
  cleanX11LockFiles();

  for (const dir of [TEST_WORKSPACE_PATH, USER_DATA_DIR_PATH, TEST_DIST_PATH]) {
    if (fs.existsSync(dir)) {
      try {
        fs.rmSync(dir, { recursive: true, force: true });
      } catch (err) {
        console.warn(`[runTest] Failed to clean ${dir}:`, err);
      }
    }
  }
}

process.on('exit', performCleanup);
process.on('SIGINT', () => {
  performCleanup();
  process.exit(130);
});
process.on('SIGTERM', () => {
  performCleanup();
  process.exit(143);
});

/**
 * Prepare a minimal test workspace directory.
 */
function prepareTestWorkspace(): void {
  if (fs.existsSync(TEST_WORKSPACE_PATH)) {
    fs.rmSync(TEST_WORKSPACE_PATH, { recursive: true, force: true });
  }
  fs.mkdirSync(TEST_WORKSPACE_PATH, { recursive: true });
  fs.writeFileSync(path.join(TEST_WORKSPACE_PATH, 'README.md'), '# Test Workspace\n');
}

/**
 * Main entry point.
 */
async function main(): Promise<void> {
  // Fail closed if wiki binary is not available — a test that does not run because of
  // an infrastructure error is a blocking condition (CLAUDE.md golden rule).
  if (!hasWikiBinary()) {
    console.error(
      '[runTest] wiki binary not found on PATH. A test that does not run because of an infrastructure error is a blocking condition.'
    );
    process.exit(1);
  }

  let exitCode = 1;

  // extensionDevelopmentPath points to the unique dist directory which contains
  // a generated package.json with main pointing to bundle.cjs.
  const extensionDevelopmentPath = TEST_DIST_PATH;
  const extensionTestsPath = path.join(TEST_DIST_PATH, 'test/suite/index.cjs');

  try {
    if (isHeadless()) {
      state.xvfbProcess = startXvfb();
    }

    // Build the extension and test files into the unique dist directory.
    cp.execSync('node scripts/build/build-testing.js', {
      cwd: EXTENSION_ROOT,
      stdio: 'inherit',
      env: {
        ...process.env,
        TEST_DIST_DIR: TEST_DIST_PATH
      }
    });

    // Verify extensionTestsPath exists after build.
    if (!fs.existsSync(extensionTestsPath)) {
      throw new Error(`extensionTestsPath does not exist after build: ${extensionTestsPath}`);
    }

    prepareTestWorkspace();

    // Remove VSCODE_ and ELECTRON_RUN_AS_NODE env vars that cause MODULE_NOT_FOUND errors.
    const problematicVars = Object.keys(process.env).filter(
      (key) => key.startsWith('VSCODE_') || key === 'ELECTRON_RUN_AS_NODE'
    );
    for (const key of problematicVars) {
      delete process.env[key];
    }

    exitCode = await runTests({
      version: getMinVSCodeVersion(),
      extensionDevelopmentPath,
      extensionTestsPath,
      extensionTestsEnv: { ...process.env },
      launchArgs: [
        TEST_WORKSPACE_PATH,
        '--disable-extensions',
        '--disable-gpu',
        '--no-sandbox',
        `--user-data-dir=${USER_DATA_DIR_PATH}`
      ]
    });
  } catch (err) {
    const errorMessage = err instanceof Error ? err.message : String(err);

    // On SIGSEGV in headless mode, retry once after restarting Xvfb.
    const isSIGSEGV =
      errorMessage.includes('SIGSEGV') ||
      (err && typeof err === 'object' && 'signal' in err && (err as { signal?: string }).signal === 'SIGSEGV');

    if (isSIGSEGV && isHeadless()) {
      cleanX11LockFiles();

      if (state.xvfbProcess) {
        try {
          state.xvfbProcess.kill('SIGTERM');
        } catch (killErr) {
          if (
            killErr &&
            typeof killErr === 'object' &&
            'code' in killErr &&
            (killErr as NodeJS.ErrnoException).code !== 'ESRCH'
          ) {
            console.warn('[runTest] Failed to kill Xvfb before retry:', killErr);
          }
        }
      }

      state.xvfbProcess = startXvfb();

      console.warn('[runTest] Retrying after SIGSEGV...');

      try {
        exitCode = await runTests({
          version: getMinVSCodeVersion(),
          extensionDevelopmentPath,
          extensionTestsPath,
          extensionTestsEnv: { ...process.env },
          launchArgs: [
            TEST_WORKSPACE_PATH,
            '--disable-extensions',
            '--disable-gpu',
            '--no-sandbox',
            `--user-data-dir=${USER_DATA_DIR_PATH}`
          ]
        });
      } catch (retryErr) {
        console.error('[runTest] Retry after SIGSEGV also failed:', retryErr);
        exitCode = 1;
      }
    } else {
      console.error('[runTest] Test run failed:', errorMessage);
      exitCode = 1;
    }
  } finally {
    performCleanup();
  }

  process.exit(exitCode);
}

main().catch((err: unknown) => {
  console.error('[wiki-extension] Unhandled error in main:', err);
  performCleanup();
  process.exit(1);
});
