/**
 * Reproduction test: `resolveManagedWikiBinary` re-hashes the full binary
 * on every call instead of trusting mtime/size after initial verification.
 *
 * The test installs a managed binary, calls resolveManagedWikiBinary to warm
 * the resolved state, then corrupts the binary content while preserving its
 * mtime, atime, permissions, and file size.  A second call to
 * resolveManagedWikiBinary with the current code re-reads the full binary
 * via sha256File, detects the corruption against the manifest checksum,
 * and returns null.  After a fix that trusts mtime/size, the second call
 * would skip re-hashing and return a handle, accepting the (unchanged
 * metadata) binary as valid.
 *
 * The assertion is that the second resolve returns a handle — which fails
 * under the current code because the checksum mismatch forces a null
 * return.
 *
 * @summary Verifies resolveManagedWikiBinary re-reads binary on every call (no mtime/size trust).
 * @module test/suite/wikiBinary.rehashPerf.test
 */

import * as assert from 'node:assert';
import { createHash } from 'node:crypto';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { installManagedWikiBinary, resolveManagedWikiBinary } from '../../src/utils/wikiBinary.js';
import { getManagedBinaryPaths, getWikiChecksumsAssetName, resolveWikiPlatform } from '../../src/utils/wikiPlatform.js';

describe('wikiBinary rehash performance', () => {
  it('re-reads full binary on every resolve call instead of trusting mtime/size', async function () {
    if (process.platform === 'win32') {
      this.skip();
    }

    const target = resolveWikiPlatform();
    assert.ok(target, 'Expected the current platform to be supported');

    const storageRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'wiki-rehash-'));
    const version = '9.9.9-rehash-perf';

    // Create fixture binary content (51 bytes).
    const assetBytes = Buffer.from('#!/usr/bin/env node\nprocess.stdout.write("[]");\n');
    const sha256 = createHash('sha256').update(assetBytes).digest('hex');

    const fetchImpl: typeof fetch = async (url, _init) => {
      const urlStr = typeof url === 'string' ? url : url instanceof URL ? url.href : (url as Request).url;
      if (urlStr.includes(getWikiChecksumsAssetName())) {
        return new Response(
          JSON.stringify({
            version,
            assets: {
              [target!.assetKey]: { name: target!.assetName, sha256 }
            }
          }),
          { status: 200 }
        );
      }
      if (urlStr.includes(target!.assetName)) {
        return new Response(assetBytes, { status: 200 });
      }
      return new Response(null, { status: 404 });
    };

    const params = { storageRoot, version, releaseBaseUrl: 'http://127.0.0.1:0', fetchImpl };
    const managedPaths = getManagedBinaryPaths(storageRoot, version, target);

    try {
      // Step 1: Install the managed binary.
      // installManagedWikiBinary calls resolveManagedWikiBinary internally,
      // which will miss (binary doesn't exist yet), proceed to download,
      // verify checksum, and write both binary and manifest.
      const installed = await installManagedWikiBinary(params);
      assert.strictEqual(installed.installed, true);

      // Step 2: First explicit resolve — expected to re-read and verify
      // the binary via sha256File (current code always does this).
      const first = await resolveManagedWikiBinary(params);
      assert.ok(first, 'First resolve should succeed — checksum matches manifest');

      // Step 3: Corrupt the binary content while preserving metadata that
      // the putative mtime/size fix would trust.
      //
      //   - content: replaced with same-length garbage so sha256 changes
      //   - mtime:   restored to original so it still ≤ installedAt
      //   - mode:    restored so assertExecutable passes
      //   - size:    identical (garbage buffer is same length)
      const { atime, mtime, mode } = fs.statSync(managedPaths.binaryPath);
      const corruptContent = Buffer.alloc(assetBytes.length, 0xa5); // same-length garbage
      fs.writeFileSync(managedPaths.binaryPath, corruptContent);
      fs.chmodSync(managedPaths.binaryPath, mode);
      fs.utimesSync(managedPaths.binaryPath, atime, mtime);

      // Step 4: Second resolve — should NOT re-read the binary.
      //
      // Current code path (bug):
      //   resolveManagedWikiBinary
      //     → sha256File(binaryPath)       ← reads entire binary
      //     → sha256(binary) !== checksum  ← detects corruption
      //     → return null                  ← assertion fails → TEST FAILS
      //
      // After fix (mtime/size trust):
      //   resolveManagedWikiBinary
      //     → stat(binaryPath)             ← checks mtime, size
      //     → mtime <= installedAt, same size → trust (skip sha256)
      //     → return handle                ← assertion passes
      const second = await resolveManagedWikiBinary(params);
      assert.ok(
        second,
        'Expected second resolve to succeed without re-reading the binary ' +
          '(mtime unchanged, size unchanged — should be trusted without full re-hash). ' +
          'This assertion fails under the current code because sha256File always re-reads ' +
          'the entire binary and the corruption is detected against the manifest checksum.'
      );
    } finally {
      fs.rmSync(storageRoot, { recursive: true, force: true });
    }
  });
});
