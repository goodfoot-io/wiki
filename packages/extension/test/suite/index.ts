/**
 * Discovers compiled test files and runs them with Mocha.
 *
 * Simplified runner with no caching infrastructure — wiki-extension has a small
 * test suite that does not benefit from fingerprint-based caching.
 *
 * Supports the TEST_PATTERN environment variable for focused runs. Patterns are
 * normalized against the compiled layout ({TEST_DIST_PATH}/test/suite/*.test.cjs),
 * so bare file names match without the `suite/` prefix, and a pattern matching
 * zero compiled suites is an error instead of a silent "0 passing" success.
 *
 * @summary Mocha test runner entry point for the wiki-extension test suite.
 * @module test/suite/index
 */

import * as path from 'node:path';
import { glob } from 'glob';
import Mocha from 'mocha';

// Mocha registers process-level error listeners for each test file; raise the
// limit to avoid spurious MaxListenersExceededWarning output.
process.setMaxListeners(0);

/**
 * Normalizes a TEST_PATTERN value into a glob anchored at the compiled test root.
 *
 * Suites compile to `{TEST_DIST_PATH}/test/suite/*.test.cjs`, so a pattern
 * without any directory segment (e.g. `workspaceLinkScan`) could otherwise
 * never match a file one level down in `suite/`. A bare stem is therefore
 * anchored and given a trailing wildcard, which also lets it select a suite
 * family by prefix (`wikiQuickPick.debounce` → `wikiQuickPick.debounce.test.cjs`).
 * Patterns that already carry a directory segment or an explicit `.cjs` suffix
 * are used verbatim.
 *
 * @param testPattern - Raw TEST_PATTERN value, or undefined for the full run.
 * @returns Glob pattern relative to the compiled test root (every suite by default).
 */
function resolveTestGlob(testPattern: string | undefined): string {
  if (!testPattern) {
    return '**/*.test.cjs';
  }

  const normalized = testPattern
    .replace(/\\/g, '/')
    .replace(/^test\//, '')
    .replace(/\.tsx?$/, '.cjs');
  const explicitCjs = normalized.endsWith('.cjs');
  const suffixed = explicitCjs ? normalized : `${normalized}.cjs`;

  // Absolute paths address compiled files directly.
  if (path.isAbsolute(suffixed)) {
    return suffixed;
  }
  // Patterns naming a directory are honored verbatim.
  if (suffixed.includes('/')) {
    return suffixed;
  }
  // A bare stem filters compiled base names: tolerate the conventional `.test`
  // segment and match suite families by prefix (foo -> suite/foo.test.cjs).
  return explicitCjs ? `**/${suffixed}` : `**/${suffixed.replace(/\.cjs$/, '*.cjs')}`;
}

/**
 * Runs all test suites discovered under the compiled test directory.
 *
 * Called by @vscode/test-electron after the Extension Host has loaded the extension.
 * Must export a `run` function — that is the contract required by the test runner.
 *
 * @returns Promise that resolves when all tests pass, rejects on any failure.
 * @throws Error with failure count when one or more tests fail.
 */
export async function run(): Promise<void> {
  // __dirname resolves to the directory containing this compiled .cjs file, which
  // is {TEST_DIST_PATH}/test/suite/. One level up is {TEST_DIST_PATH}/test/.
  const testsRoot = path.resolve(__dirname, '..');

  const testPattern = process.env['TEST_PATTERN'];
  const globPattern = resolveTestGlob(testPattern);
  if (testPattern) {
    console.log('[suite] Running tests matching:', globPattern);
  }

  const mocha = new Mocha({ ui: 'bdd', color: true, timeout: 10000 });

  const files = await glob(globPattern, { cwd: testsRoot });
  if (files.length === 0) {
    throw new Error(
      `[suite] No compiled test files match ${testPattern ? `TEST_PATTERN '${testPattern}'` : 'the default pattern'} (glob '${globPattern}' under ${testsRoot}). Suites compile to test/suite/*.test.cjs — check the pattern for typos.`
    );
  }
  console.log(`[suite] Matched ${files.length} compiled test file(s)`);
  for (const f of files) {
    mocha.addFile(path.resolve(testsRoot, f));
  }

  return new Promise<void>((resolve, reject) => {
    mocha.run((failures: number) => {
      if (failures > 0) {
        reject(new Error(`${failures} tests failed.`));
      } else {
        resolve();
      }
    });
  });
}
