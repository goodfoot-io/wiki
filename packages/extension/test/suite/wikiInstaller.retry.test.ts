/**
 * Reproduction test: `WikiBinaryManager.retry()` can never recover from a
 * failed activation-time install.
 *
 * `retry()` awaits the cached `readyPromise` before clearing it. When that
 * promise rejected during activation (offline, bad release URL), the await
 * rethrows the original failure, so the promise is never discarded and every
 * subsequent attempt keeps serving the stale rejection until the window
 * reloads.
 *
 * This test drives a real install against a local release server that first
 * fails every request, asserts `start()` rejects, then flips the server to
 * serve a valid fixture release and asserts `retry()` recovers. It MUST FAIL
 * against the current unfixed code, where `retry()` rethrows the stale
 * rejection instead of discarding it.
 *
 * @summary Reproduction test for the dead retry path in wikiInstaller.
 * @module test/suite/wikiInstaller.retry.test
 */

import * as assert from 'node:assert';
import { createHash } from 'node:crypto';
import * as fs from 'node:fs';
import { createServer, type Server } from 'node:http';
import * as os from 'node:os';
import * as path from 'node:path';
import type * as vscode from 'vscode';
import { WikiBinaryManager } from '../../src/utils/wikiInstaller.js';
import { getWikiChecksumsAssetName, getWikiReleaseTag, resolveWikiPlatform } from '../../src/utils/wikiPlatform.js';

describe('wikiInstaller — retry recovers from a failed start (bug reproduction)', () => {
  it('retry() discards a rejected start and installs successfully', async function () {
    if (process.platform === 'win32') {
      this.skip();
    }
    this.timeout(10000);

    const target = resolveWikiPlatform();
    assert.ok(target, 'Expected the current platform to be supported');

    const version = '9.9.9-test';
    const tag = getWikiReleaseTag(version);
    const storageRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'wiki-retry-storage-'));
    const releaseRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'wiki-retry-release-'));
    const fixturePath = path.join(releaseRoot, target.assetName);
    fs.writeFileSync(fixturePath, '#!/bin/sh\necho ok\n', { mode: 0o755 });
    const sha256 = createHash('sha256').update(fs.readFileSync(fixturePath)).digest('hex');

    // One server, two modes: 'fail' rejects every request (activation-time
    // offline/bad-URL stand-in); 'serve' answers with a valid release so a
    // fresh ensureReady can succeed.
    let mode: 'fail' | 'serve' = 'fail';
    const server: Server = createServer((request, response) => {
      if (mode === 'fail') {
        response.writeHead(503);
        response.end();
        return;
      }
      if (request.url === `/${tag}/${getWikiChecksumsAssetName()}`) {
        response.writeHead(200, { 'content-type': 'application/json' });
        response.end(
          JSON.stringify({
            version,
            assets: {
              [target.assetKey]: {
                name: target.assetName,
                sha256
              }
            }
          })
        );
        return;
      }
      if (request.url === `/${tag}/${target.assetName}`) {
        response.writeHead(200, { 'content-type': 'application/octet-stream' });
        response.end(fs.readFileSync(fixturePath));
        return;
      }
      response.writeHead(404);
      response.end();
    });
    await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', () => resolve()));
    const address = server.address();
    assert.ok(address && typeof address === 'object', 'Expected HTTP server address');
    const releaseBaseUrl = `http://127.0.0.1:${address.port}`;

    const savedPathFallback = process.env['WIKI_EXTENSION_USE_PATH_FALLBACK'];
    const savedReleaseUrl = process.env['WIKI_EXTENSION_RELEASE_BASE_URL'];
    process.env['WIKI_EXTENSION_USE_PATH_FALLBACK'] = '0';
    process.env['WIKI_EXTENSION_RELEASE_BASE_URL'] = releaseBaseUrl;

    try {
      const context = fakeExtensionContext(storageRoot, version);
      const manager = new WikiBinaryManager(context);

      // Activation-equivalent: the first start fails (server refuses).
      await assert.rejects(manager.start(), /HTTP 503/, 'Expected the initial start to fail');

      // Conditions heal: the release server now works.
      mode = 'serve';

      // Expected: retry discards the failed promise and installs.
      // Actual (bug): retry() rethrows the stale activation failure forever.
      const result = await manager.retry();
      assert.strictEqual(result.handle.source, 'managed', 'Expected a managed binary after retry');
      assert.strictEqual(result.installed, true, 'Expected retry to perform a fresh install');
    } finally {
      server.close();
      fs.rmSync(storageRoot, { recursive: true, force: true });
      fs.rmSync(releaseRoot, { recursive: true, force: true });
      restoreEnv('WIKI_EXTENSION_USE_PATH_FALLBACK', savedPathFallback);
      restoreEnv('WIKI_EXTENSION_RELEASE_BASE_URL', savedReleaseUrl);
    }
  });
});

function restoreEnv(key: string, value: string | undefined): void {
  if (value === undefined) {
    delete process.env[key];
  } else {
    process.env[key] = value;
  }
}

function fakeExtensionContext(storageRoot: string, version: string): vscode.ExtensionContext {
  return {
    globalStorageUri: { fsPath: storageRoot },
    extension: { packageJSON: { version } },
    extensionMode: 1, // vscode.ExtensionMode.Production
    environmentVariableCollection: {
      clear: () => {},
      description: undefined,
      persistent: false,
      prepend: () => {},
      append: () => {},
      replace: () => {},
      get: () => undefined,
      forEach: () => {},
      delete: () => {},
      [Symbol.iterator]: function* () {}
    }
  } as unknown as vscode.ExtensionContext;
}
