/**
 * Reproduction test: `installManagedWikiBinary` hangs indefinitely when
 * `fetchImpl` receives no `AbortSignal` and the connection never resolves.
 *
 * The default `fetchImpl` is the global `fetch` (L154 of wikiBinary.ts). Neither
 * call site (`checksumsUrl` at L170, `assetUrl` at L188) passes an `AbortSignal`
 * or applies a timeout. When the GitHub release endpoint is unreachable or hangs
 * (offline user, DNS failure, TCP SYN accepted but no response), the download
 * hangs forever.  Because `ensureReady()` → `installManagedWikiBinary()` is
 * awaited by `WikiEditorProvider`'s `ready` handler, a hung download blocks
 * article rendering — no content appears, even though the shell HTML with the
 * loading spinner was already delivered.
 *
 * This test is a **red-phase reproduction**: it MUST FAIL against the current
 * unfixed code by demonstrating that a hanging fetch blocks indefinitely with
 * no abort mechanism.  After the fix (adding a configurable timeout or
 * `AbortSignal` to `installManagedWikiBinary`), the promise will reject before
 * the 2-second race timeout, and the test will pass.
 *
 * @summary Hanging fetch reproduction test — must fail against unfixed code.
 * @module test/suite/wikiBinary.hangingFetch.test
 */

import * as assert from 'node:assert';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { installManagedWikiBinary } from '../../src/utils/wikiBinary.js';
import { resolveWikiPlatform } from '../../src/utils/wikiPlatform.js';

describe('wikiBinary hanging fetch', () => {
  it('has no fetch timeout or AbortSignal — a hung connection blocks indefinitely', async function () {
    this.timeout(10_000);

    const target = resolveWikiPlatform();
    if (target == null) {
      this.skip();
    }

    const storageRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'wiki-hang-'));
    const version = '9.9.9-test';
    const releaseBaseUrl = 'http://127.0.0.1:0';

    // A fetch implementation that never resolves — simulates a hung TCP
    // connection (offline, DNS failure, or a remote that accepts the SYN
    // but never sends a response).
    const hangingFetch: typeof fetch = () => {
      return new Promise<Response>(() => {
        /* never resolves or rejects */
      });
    };

    const RACE_TIMEOUT_MS = 2000;

    try {
      const installPromise = installManagedWikiBinary({
        storageRoot,
        version,
        releaseBaseUrl,
        fetchImpl: hangingFetch
      });

      const timeoutPromise = new Promise<never>((_, reject) => {
        setTimeout(
          () => reject(new Error(`NO_ABORT: hung for ${RACE_TIMEOUT_MS}ms without aborting`)),
          RACE_TIMEOUT_MS
        );
      });

      // Race the install against a short timeout.
      //   - Before fix (no abort):     timeout always wins → test fails (red)
      //   - After fix  (with abort):   installPromise rejects first with a
      //     fetch-abort/timeout error → test passes (green)
      const raced = await Promise.race([installPromise, timeoutPromise]);

      // If the race resolved with a value, the hanging fetch somehow completed.
      // This should not happen with a never-resolving mock.
      assert.fail(`Unexpected success — install returned ${JSON.stringify(raced)}`);
    } catch (error: unknown) {
      const message = error instanceof Error ? error.message : String(error);
      if (message.includes('NO_ABORT')) {
        // The test timeout fired first — the code has no abort mechanism.
        // Re-throw to fail this test as a red-phase reproduction.
        throw new Error(
          `installManagedWikiBinary blocked indefinitely — no timeout or AbortSignal on fetch (message: ${message})`
        );
      }
      // An error that is NOT the test timeout means the promise rejected for
      // another reason (e.g. an abort error from the fixed code).  That is the
      // desired green-phase behavior — swallow the error and pass.
    } finally {
      fs.rmSync(storageRoot, { recursive: true, force: true });
    }
  });
});
