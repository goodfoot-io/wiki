/**
 * Reproduction test: searchPages passes queries starting with `-` directly
 * as argv[0], allowing argument injection.
 *
 * ## Bug
 * `searchPages` (wikiQuickPick.ts:120) constructs the argv as:
 * ```
 * [query, '--format', 'json']
 * ```
 * and passes it to `runWikiCommand`. When the user types `--help` in the
 * QuickPick, the argv becomes `['--help', '--format', 'json']`. The wiki CLI
 * interprets `--help` as a flag, producing help output instead of search
 * results. This is argument injection (though not shell injection, since
 * spawn is array-form).
 *
 * ## Code path
 * `onDidChangeValue` -> `searchPages(binaryPath, query.trim(), signal)`
 * (L217) -> `runWikiCommand(binaryPath, [query, '--format', 'json'], signal, workspaceRoot())`
 * (L120) -> `child_process.spawn(binaryPath, ['--help', '--format', 'json'])`.
 *
 * ## Test approach
 * 1. Mock `child_process.spawn` via `createRequire` (following the
 *    pattern established by wikiEditorProvider.noSpawn.test.ts) to capture the
 *    argument vector passed to the wiki binary.
 * 2. Call `runWikiCommand` with the same args `searchPages` would construct
 *    for a `--help` query.
 * 3. Assert that the argument vector starts with `'--'` (the POSIX
 *    end-of-options separator), which would prevent the wiki CLI from
 *    interpreting the query as a flag.
 * 4. The assertion FAILS against the current unfixed code because
 *    `searchPages` passes `query` directly as argv[0] without any `--`
 *    separator.
 *
 * @summary Reproduction test for argument injection in wiki QuickPick search.
 * @module test/suite/wikiQuickPick.argumentInjection.test
 */

import * as assert from 'node:assert';
import { EventEmitter } from 'node:events';
import { createRequire } from 'node:module';
import { runWikiCommand } from '../../src/utils/wikiBinary.js';

// esbuild compiles `import { spawn } from 'node:child_process'` in wikiBinary.ts
// to a runtime property access on the live CJS module object (see the
// wikiEditorProvider.noSpawn.test.ts comment for details). Mutating the CJS
// exports is observed by production code called after the mutation.
const require = createRequire(__filename);
const childProcess = require('node:child_process') as typeof import('node:child_process');

function makeFakeSpawnChild(): ReturnType<typeof childProcess.spawn> {
  const child = new EventEmitter() as ReturnType<typeof childProcess.spawn>;
  // ChildProcess has readonly stdout/stderr/kill, but we need to set them
  // for our fake. Cast through unknown to bypass the readonly constraint.
  const writable = child as unknown as { stdout: EventEmitter; stderr: EventEmitter; kill: () => boolean };
  writable.stdout = new EventEmitter();
  writable.stderr = new EventEmitter();
  writable.kill = () => true;
  return child;
}

describe('wikiQuickPick — argument injection (bug reproduction)', () => {
  it('searchPages passes query `--help` as argv[0] without `--` separator', async () => {
    const spawnedArgVectors: string[][] = [];

    const originalSpawn = childProcess.spawn;
    (childProcess as { spawn: typeof childProcess.spawn }).spawn = function patchedSpawn(
      this: unknown,
      ...args: Parameters<typeof childProcess.spawn>
    ): ReturnType<typeof childProcess.spawn> {
      const argv = Array.isArray(args[1]) ? (args[1] as string[]) : [];
      spawnedArgVectors.push(argv);
      const fake = makeFakeSpawnChild();
      process.nextTick(() => (fake as EventEmitter).emit('close', 0));
      return fake;
    } as typeof childProcess.spawn;

    try {
      // searchPages constructs: runWikiCommand(binaryPath, [query, '--format', 'json'], ...)
      // When the user types '--help' in the QuickPick, query.trim() === '--help',
      // and the argv becomes ['--help', '--format', 'json'].
      await runWikiCommand('/fake/wiki', ['--help', '--format', 'json']);

      assert.strictEqual(spawnedArgVectors.length, 1, 'Expected exactly one spawn call');

      const argv = spawnedArgVectors[0];
      assert.ok(argv, 'Expected captured argv');
      assert.ok(argv.length >= 1, 'Expected argv to have at least one element');

      // The current (broken) behavior: argv = ['--help', '--format', 'json']
      // The wiki CLI interprets '--help' as a flag, producing help output.
      //
      // The desired behavior: argv[0] should be '--' (the POSIX end-of-options
      // separator), telling the CLI that everything after is a positional
      // argument, not a flag.
      //
      // THIS ASSERTION FAILS until the fix is applied. The test reproduces the
      // bug by demonstrating that the query is placed directly as argv[0]
      // without any protection against flag interpretation.
      assert.strictEqual(
        argv[0],
        '--',
        `Expected argv[0] to be '--' separator to prevent argument injection, ` +
          `but got '${argv[0]}'. Full argv: ${JSON.stringify(argv)}. ` +
          `The query '--help' is interpreted as a CLI flag instead of a search term.`
      );
    } finally {
      (childProcess as { spawn: typeof childProcess.spawn }).spawn = originalSpawn;
    }
  });
});
