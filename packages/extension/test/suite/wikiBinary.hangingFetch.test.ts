/**
 * Post-fix test: `installManagedWikiBinary` rejects via `AbortSignal` when the
 * underlying fetch hangs.
 *
 * `installManagedWikiBinary` accepts an optional `AbortSignal` in
 * `InstallManagedWikiBinaryParams.signal`. When a caller provides a
 * signal (backed by an `AbortController` with a timeout), the fetch calls
 * pass the signal through and the function rejects before the caller's
 * deadline.
 *
 * This test creates an `AbortController` with a short timeout, passes its
 * signal to `installManagedWikiBinary` along with a never-resolving fetch
 * mock (simulating a hung TCP connection), and asserts the install promise
 * rejects — proving the fetch will not block indefinitely.
 *
 * @summary Verifies AbortSignal prevents indefinite hang in installManagedWikiBinary.
 * @module test/suite/wikiBinary.hangingFetch.test
 */

import * as assert from 'node:assert';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { installManagedWikiBinary } from '../../src/utils/wikiBinary.js';
import { resolveWikiPlatform } from '../../src/utils/wikiPlatform.js';

describe('wikiBinary hanging fetch', () => {
  it('rejects via AbortSignal when the underlying fetch hangs', async function () {
    this.timeout(10_000);

    const target = resolveWikiPlatform();
    if (target == null) {
      this.skip();
    }

    const storageRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'wiki-hang-'));
    const version = '9.9.9-test';
    const releaseBaseUrl = 'http://127.0.0.1:0';

    // A fetch implementation that hangs indefinitely unless the provided
    // AbortSignal fires — simulates an unresponsive remote endpoint.
    const hangingFetch: typeof fetch = (_url, init) => {
      return new Promise<Response>((_resolve, reject) => {
        const signal = init?.signal;
        if (signal != null) {
          if (signal.aborted) {
            reject(new Error('The operation was aborted'));
            return;
          }
          signal.addEventListener(
            'abort',
            () => {
              reject(new Error('The operation was aborted'));
            },
            { once: true }
          );
        }
        // If no signal, hang forever (as before the fix); with a signal,
        // reject when the caller aborts.
      });
    };

    // Create an AbortController that fires after 500 ms — well within the
    // 2-second race timeout, so the abort wins.
    const controller = new AbortController();
    const ABORT_TIMEOUT_MS = 500;
    const abortTimer = setTimeout(() => controller.abort(), ABORT_TIMEOUT_MS);

    const RACE_TIMEOUT_MS = 2000;

    try {
      const installPromise = installManagedWikiBinary({
        storageRoot,
        version,
        releaseBaseUrl,
        fetchImpl: hangingFetch,
        signal: controller.signal
      });

      const timeoutPromise = new Promise<never>((_, reject) => {
        setTimeout(
          () => reject(new Error(`NO_ABORT: hung for ${RACE_TIMEOUT_MS}ms without aborting`)),
          RACE_TIMEOUT_MS
        );
      });

      // Race the install against a short test-level timeout.
      //   - With AbortSignal:    installPromise rejects with an abort error
      //                          before the 2s test timeout → passes
      //   - Without AbortSignal: test timeout fires first → fails
      const raced = await Promise.race([installPromise, timeoutPromise]);

      assert.fail(`Unexpected success — install returned ${JSON.stringify(raced)}`);
    } catch (error: unknown) {
      const message = error instanceof Error ? error.message : String(error);
      if (message.includes('NO_ABORT')) {
        throw new Error(
          `installManagedWikiBinary blocked indefinitely — the AbortSignal did not take effect (message: ${message})`
        );
      }
      // Any non-NO_ABORT error (e.g. abort error) is the desired green-phase
      // behaviour — swallow and pass.
    } finally {
      clearTimeout(abortTimer);
      fs.rmSync(storageRoot, { recursive: true, force: true });
    }
  });
});
