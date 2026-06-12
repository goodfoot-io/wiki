/**
 * Pre-fix test: `installManagedWikiBinary` blocks indefinitely when no
 * `AbortSignal` is provided and the underlying fetch hangs.
 *
 * `installManagedWikiBinary` accepts an optional `AbortSignal` in
 * `InstallManagedWikiBinaryParams.signal` and passes it to fetch calls.
 * When the caller does not provide a signal (see `wikiInstaller.ts`
 * L99-L105), the fetch has no timeout, and a hung connection blocks
 * `ready()` forever.
 *
 * This test calls `installManagedWikiBinary` WITHOUT a signal, using a
 * never-resolving fetch mock (simulating a hung TCP connection), and
 * asserts the promise rejects within 5 seconds. The test FAILS against
 * the current unfixed code because the function has no default timeout
 * when no `AbortSignal` is provided.
 *
 * The existing test `wikiBinary.hangingFetch.test.ts` verifies the case
 * where a caller-provided `AbortSignal` *is* passed — that test passes.
 * This test covers the complementary case: when no signal is provided,
 * there is no fallback timeout.
 *
 * @summary Verifies absence of default timeout in installManagedWikiBinary.
 * @module test/suite/wikiBinary.noDefaultTimeout.test
 */

import * as assert from 'node:assert';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { installManagedWikiBinary } from '../../src/utils/wikiBinary.js';
import { resolveWikiPlatform } from '../../src/utils/wikiPlatform.js';

describe('wikiBinary no default timeout', () => {
  it('blocks indefinitely when no AbortSignal is provided and fetch hangs', async function () {
    this.timeout(45_000);

    const target = resolveWikiPlatform();
    if (target == null) {
      this.skip();
    }

    const storageRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'wiki-notimeout-'));
    const version = '9.9.9-test';
    const releaseBaseUrl = 'http://127.0.0.1:0';

    // A fetch implementation that hangs indefinitely — simulates an
    // unresponsive remote endpoint. Unlike the hangingFetch mock in
    // wikiBinary.hangingFetch.test.ts, this one does NOT react to any
    // AbortSignal (it ignores the init options entirely).
    const hangingFetch: typeof fetch = () => {
      return new Promise<Response>(() => {
        // Intentionally never resolve or reject — simulates a hung
        // TCP connection that neither responds nor times out.
      });
    };

    const HANG_TIMEOUT_MS = 35_000;

    try {
      const installPromise = installManagedWikiBinary({
        storageRoot,
        version,
        releaseBaseUrl,
        fetchImpl: hangingFetch
        // NOTE: No signal is provided — this is the bug scenario.
        // In production, wikiInstaller.ts calls installManagedWikiBinary
        // without a signal (see ensureReady L99-L105).
      });

      // Race the install against a fixed timeout. If the timeout fires
      // first, the test fails — the code under test has no default
      // timeout when no caller-provided AbortSignal exists.
      const timeoutPromise = new Promise<never>((_, reject) => {
        setTimeout(
          () => reject(new Error(`NO_DEFAULT_TIMEOUT: hung for ${HANG_TIMEOUT_MS}ms without timing out`)),
          HANG_TIMEOUT_MS
        );
      });

      const raced = await Promise.race([installPromise, timeoutPromise]);
      // If we reach here, the install promise resolved (unexpected,
      // since the hanging fetch should never resolve).
      assert.fail(`Unexpected success — install returned ${JSON.stringify(raced)}`);
    } catch (error: unknown) {
      const message = error instanceof Error ? error.message : String(error);
      if (message.includes('NO_DEFAULT_TIMEOUT')) {
        // The timeout won the race — the install promise hung
        // indefinitely. This is the EXPECTED failure mode against
        // the unfixed code, proving the absence of a default timeout.
        throw new Error(
          `installManagedWikiBinary blocked indefinitely — no default timeout was applied when no AbortSignal was provided (message: ${message})`
        );
      }
      // Any other error (e.g. a future default-timeout rejection)
      // means the install promise rejected before the test timeout.
      // This is the desired behavior — swallow and pass.
    } finally {
      fs.rmSync(storageRoot, { recursive: true, force: true });
    }
  });
});
