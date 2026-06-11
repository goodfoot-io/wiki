/**
 * Reproduction test: concurrent `installManagedWikiBinary` calls race on shared
 * filesystem paths.
 *
 * Simulates the `retry()` bug in `WikiBinaryManager` where calling `retry()`
 * during an in-flight `ensureReady()` starts a second `installManagedWikiBinary`
 * without awaiting or cancelling the first. Both concurrent calls download to
 * the same `.download` paths, and the loser's catch handler can delete the
 * winner's binary via `cleanupInstallArtifacts` + `rm(binaryPath, { force: true })`.
 *
 * The test calls `installManagedWikiBinary` twice concurrently (as `retry()`
 * + `start()` would), using a custom `fetchImpl` that counts asset downloads.
 * With the bug, both calls proceed past `resolveManagedWikiBinary` (both miss
 * because neither has installed yet), so both download → count=2. With
 * serialization (the fix), the second call finds the first's install and
 * returns early without downloading → count=1.
 *
 * @summary Verifies concurrent installs serialize (second call does not re-download).
 * @module test/suite/wikiBinary.concurrentInstall.test
 */

import * as assert from 'node:assert';
import { createHash } from 'node:crypto';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { installManagedWikiBinary } from '../../src/utils/wikiBinary.js';
import { getManagedBinaryPaths, getWikiChecksumsAssetName, resolveWikiPlatform } from '../../src/utils/wikiPlatform.js';

describe('wikiBinary concurrent install', () => {
  it('serializes concurrent installs — second call should await first rather than re-downloading', async function () {
    if (process.platform === 'win32') {
      this.skip();
    }

    const target = resolveWikiPlatform();
    assert.ok(target, 'Expected the current platform to be supported');

    const storageRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'wiki-concurrent-'));
    const version = '9.9.9-race';

    // A minimal executable script used as the "binary" asset.
    const assetBytes = Buffer.from('#!/usr/bin/env node\nprocess.stdout.write("[]");\n');
    const sha256 = createHash('sha256').update(assetBytes).digest('hex');

    // Track how many times the asset endpoint is hit.
    let assetDownloadCount = 0;

    const fetchImpl: typeof fetch = async (url, _init) => {
      const urlStr = typeof url === 'string' ? url : url instanceof URL ? url.href : (url as Request).url;

      // Serve checksums manifest instantly.
      if (urlStr.includes(getWikiChecksumsAssetName())) {
        return new Response(
          JSON.stringify({
            version,
            assets: {
              [target.assetKey]: {
                name: target.assetName,
                sha256
              }
            }
          }),
          { status: 200 }
        );
      }

      // Serve binary asset with a significant delay so both calls overlap.
      if (urlStr.includes(target.assetName)) {
        assetDownloadCount++;
        // 500 ms ensures the other concurrent call has time to also reach
        // this point before either completes.
        await new Promise((r) => setTimeout(r, 500));
        return new Response(assetBytes, { status: 200 });
      }

      return new Response(null, { status: 404 });
    };

    const params = {
      storageRoot,
      version,
      releaseBaseUrl: 'http://127.0.0.1:0',
      fetchImpl
    };

    // Start two concurrent installs — simulating what happens when
    // `retry()` is called during an in-flight `ensureReady()`.
    await Promise.allSettled([installManagedWikiBinary(params), installManagedWikiBinary(params)]);

    try {
      // Both calls should NOT have downloaded independently.
      //
      // With the current buggy code:
      //   - Call A: resolveManagedWikiBinary → miss → download  (count=1)
      //   - Call B: resolveManagedWikiBinary → miss (A hasn't finished) → download (count=2)
      //   Result: assetDownloadCount === 2  ← BUG
      //
      // With serialization (the fix):
      //   - Call A: resolveManagedWikiBinary → miss → download  (count=1)
      //   - Call B: resolveManagedWikiBinary → HIT (sees A's install) → skip
      //   Result: assetDownloadCount === 1  ← CORRECT
      assert.strictEqual(
        assetDownloadCount,
        1,
        `Expected 1 asset download (serialized installs), got ${assetDownloadCount} — ` +
          'concurrent installs raced: retry() did not await the prior ensureReady()'
      );

      // Additionally, the final binary should exist and its checksum should
      // match the manifest.  If the race caused the catch handler to delete
      // the winner's binary, this assertion catches it too.
      const managedPaths = getManagedBinaryPaths(storageRoot, version, target);
      assert.ok(fs.existsSync(managedPaths.binaryPath), 'Managed binary should exist after concurrent installs');

      const manifestBody = JSON.parse(fs.readFileSync(managedPaths.manifestPath, 'utf8'));
      const binaryChecksum = createHash('sha256').update(fs.readFileSync(managedPaths.binaryPath)).digest('hex');
      assert.strictEqual(
        binaryChecksum,
        manifestBody.checksum,
        'Binary checksum should match manifest after concurrent installs'
      );
    } finally {
      fs.rmSync(storageRoot, { recursive: true, force: true });
    }
  });
});
