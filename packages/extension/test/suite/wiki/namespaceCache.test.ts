/**
 * Tests for NamespaceCache.
 *
 * Uses the real NamespaceCache implementation with a test fixture binary for
 * the `wiki namespaces` command.
 *
 * @summary NamespaceCache unit tests.
 * @module test/suite/wiki/namespaceCache.test
 */

import * as assert from 'node:assert';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import * as vscode from 'vscode';
import type { WikiBinaryManager } from '../../../src/utils/wikiInstaller.js';
import { NamespaceCache } from '../../../src/wiki/namespaceCache.js';

/**
 * Create a minimal WikiBinaryManager that returns a fixed binary path.
 *
 * Uses a double type assertion because WikiBinaryManager has private members
 * which prevent plain object literals from being structurally assignable.
 *
 * @param binaryPath - Absolute path to the wiki CLI binary the manager should resolve.
 * @returns A minimal WikiBinaryManager that always returns a ready handle for the given path.
 */
function createTestManager(binaryPath: string): WikiBinaryManager {
  const handle = { path: binaryPath, source: 'path' as const };
  return {
    ready: async () => handle,
    start: async () => ({ handle, installed: false }),
    retry: async () => ({ handle, installed: false }),
    formatFailure: (_error: unknown) => ''
  } as unknown as WikiBinaryManager;
}

/**
 * Write a fixture binary at `binaryPath` that outputs `outputJson` on stdout
 * and exits with `exitCode`.
 *
 * @param binaryPath - Absolute path where the fixture script should be written.
 * @param outputJson  - JSON string the fixture writes to stdout when executed.
 * @param exitCode     - Exit code the fixture process returns (default 0).
 */
function writeFixture(binaryPath: string, outputJson: string, exitCode = 0): void {
  fs.writeFileSync(
    binaryPath,
    Buffer.from(
      `#!/usr/bin/env node
process.stdout.write(${JSON.stringify(outputJson)});
process.exit(${exitCode});
`,
      'utf-8'
    ),
    { mode: 0o755 }
  );
}

/**
 * Create a NamespaceCache, provision a fixture binary that outputs `entries`,
 * and call refresh().
 *
 * @param tempDir     - Temporary directory for the fixture binary.
 * @param entries     - Namespace entries the fixture binary writes to stdout.
 * @param diagnostics - Diagnostic collection for cache error reporting.
 * @returns A refreshed NamespaceCache populated with the given entries.
 */
async function createCache(
  tempDir: string,
  entries: ReadonlyArray<{ namespace: string | null; path: string; abs_path: string }>,
  diagnostics: vscode.DiagnosticCollection
): Promise<NamespaceCache> {
  const binaryPath = path.join(tempDir, 'wiki-ns');
  writeFixture(binaryPath, JSON.stringify(entries));
  const cache = new NamespaceCache(createTestManager(binaryPath), diagnostics);
  await cache.refresh();
  return cache;
}

describe('NamespaceCache', () => {
  it('refreshes from wiki namespaces --format json', async () => {
    const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'ns-test-'));
    const diagnostics = vscode.languages.createDiagnosticCollection('test');
    try {
      const entries = [
        { namespace: 'default', path: 'wiki', abs_path: path.join(tempDir, 'wiki') },
        { namespace: 'mesh', path: 'wiki/mesh', abs_path: path.join(tempDir, 'wiki/mesh') }
      ];
      const cache = await createCache(tempDir, entries, diagnostics);

      assert.strictEqual(cache.getAll().length, 2);

      const defaultNs = cache.get('default');
      assert.ok(defaultNs);
      assert.strictEqual(defaultNs.namespace, 'default');
      assert.strictEqual(defaultNs.absPath, path.join(tempDir, 'wiki'));

      const meshNs = cache.get('mesh');
      assert.ok(meshNs);
      assert.strictEqual(meshNs.namespace, 'mesh');
      assert.strictEqual(meshNs.absPath, path.join(tempDir, 'wiki/mesh'));

      const unknown = cache.get('nonexistent');
      assert.strictEqual(unknown, undefined);
    } finally {
      diagnostics.dispose();
      fs.rmSync(tempDir, { recursive: true, force: true });
    }
  });

  it('maps null namespace to "default"', async () => {
    const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'ns-test-'));
    const diagnostics = vscode.languages.createDiagnosticCollection('test');
    try {
      const entries = [{ namespace: null, path: 'wiki', abs_path: path.join(tempDir, 'wiki') }];
      const cache = await createCache(tempDir, entries, diagnostics);

      const defaultNs = cache.get('default');
      assert.ok(defaultNs, 'Expected null namespace to map to "default"');
      assert.strictEqual(defaultNs.namespace, 'default');
      assert.strictEqual(defaultNs.absPath, path.join(tempDir, 'wiki'));

      const nullNs = cache.get('null');
      assert.strictEqual(nullNs, undefined, 'Expected no entry for string "null"');
    } finally {
      diagnostics.dispose();
      fs.rmSync(tempDir, { recursive: true, force: true });
    }
  });

  it('returns WikiInfo by namespace name', async () => {
    const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'ns-test-'));
    const diagnostics = vscode.languages.createDiagnosticCollection('test');
    try {
      const entries = [
        { namespace: 'default', path: 'wiki', abs_path: path.join(tempDir, 'wiki') },
        { namespace: 'mesh', path: 'wiki/mesh', abs_path: path.join(tempDir, 'wiki/mesh') }
      ];
      const cache = await createCache(tempDir, entries, diagnostics);

      const defaultNs = cache.get('default');
      assert.ok(defaultNs);
      assert.strictEqual(defaultNs.namespace, 'default');
      assert.strictEqual(defaultNs.path, 'wiki');

      const meshNs = cache.get('mesh');
      assert.ok(meshNs);
      assert.strictEqual(meshNs.namespace, 'mesh');

      const unknown = cache.get('nonexistent');
      assert.strictEqual(unknown, undefined);
    } finally {
      diagnostics.dispose();
      fs.rmSync(tempDir, { recursive: true, force: true });
    }
  });

  it('returns all discovered namespaces', async () => {
    const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'ns-test-'));
    const diagnostics = vscode.languages.createDiagnosticCollection('test');
    try {
      const entries = [
        { namespace: 'default', path: 'wiki', abs_path: path.join(tempDir, 'wiki') },
        { namespace: 'mesh', path: 'wiki/mesh', abs_path: path.join(tempDir, 'wiki/mesh') },
        { namespace: 'docs', path: 'wiki/docs', abs_path: path.join(tempDir, 'wiki/docs') }
      ];
      const cache = await createCache(tempDir, entries, diagnostics);

      const all = cache.getAll();
      assert.strictEqual(all.length, 3);

      const names = all.map((w) => w.namespace).sort();
      assert.deepStrictEqual(names, ['default', 'docs', 'mesh']);
    } finally {
      diagnostics.dispose();
      fs.rmSync(tempDir, { recursive: true, force: true });
    }
  });

  it('resolves namespace for a file path via longest-prefix match', async () => {
    const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'ns-test-'));
    const diagnostics = vscode.languages.createDiagnosticCollection('test');
    try {
      const wikiRoot = path.join(tempDir, 'wiki');
      const meshRoot = path.join(tempDir, 'wiki', 'mesh');
      const entries = [
        { namespace: 'default', path: 'wiki', abs_path: wikiRoot },
        { namespace: 'mesh', path: 'wiki/mesh', abs_path: meshRoot }
      ];
      const cache = await createCache(tempDir, entries, diagnostics);

      // File inside default namespace root.
      assert.strictEqual(cache.resolveNamespaceForFile(path.join(wikiRoot, 'some-page.md')), 'default');

      // File inside mesh namespace root (longer prefix should win).
      assert.strictEqual(cache.resolveNamespaceForFile(path.join(meshRoot, 'mesh-page.md')), 'mesh');

      // File outside any namespace root.
      assert.strictEqual(cache.resolveNamespaceForFile(path.join(tempDir, 'outside.md')), null);
    } finally {
      diagnostics.dispose();
      fs.rmSync(tempDir, { recursive: true, force: true });
    }
  });

  it('invokes the wiki binary with a workspace cwd so namespace discovery works on Remote-SSH hosts', async () => {
    const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'ns-test-'));
    const diagnostics = vscode.languages.createDiagnosticCollection('test');
    try {
      const binaryPath = path.join(tempDir, 'wiki-ns');
      const cwdLogPath = path.join(tempDir, 'cwd.log');
      // Fixture records its process.cwd() so we can verify the cache passes a
      // workspace cwd rather than letting the binary inherit the extension-host
      // cwd (which on Remote-SSH lives outside any wiki repo).
      fs.writeFileSync(
        binaryPath,
        Buffer.from(
          `#!/usr/bin/env node
require('node:fs').writeFileSync(${JSON.stringify(cwdLogPath)}, process.cwd());
process.stdout.write('[]');
process.exit(0);
`,
          'utf-8'
        ),
        { mode: 0o755 }
      );

      const cache = new NamespaceCache(createTestManager(binaryPath), diagnostics);
      await cache.refresh();

      assert.ok(fs.existsSync(cwdLogPath), 'Expected fixture binary to have been invoked');
      const recordedCwd = fs.readFileSync(cwdLogPath, 'utf-8');

      const workspaceFolders = vscode.workspace.workspaceFolders;
      assert.ok(workspaceFolders != null && workspaceFolders.length > 0, 'Test requires an open workspace folder');
      const expectedCwd = workspaceFolders[0]!.uri.fsPath;

      assert.strictEqual(
        recordedCwd,
        expectedCwd,
        `Expected NamespaceCache.refresh to invoke the wiki binary with cwd=${expectedCwd}, got cwd=${recordedCwd}`
      );
    } finally {
      diagnostics.dispose();
      fs.rmSync(tempDir, { recursive: true, force: true });
    }
  });

  it('surfaces non-zero exit as a workspace diagnostic', async () => {
    const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'ns-test-'));
    const diagnostics = vscode.languages.createDiagnosticCollection('test');
    try {
      const binaryPath = path.join(tempDir, 'wiki-ns');
      const diagUri = vscode.Uri.parse('wiki://namespace-cache');

      // First refresh with a binary that fails.
      writeFixture(binaryPath, '', 2);
      const cache = new NamespaceCache(createTestManager(binaryPath), diagnostics);
      await cache.refresh();

      const entries = diagnostics.get(diagUri);
      assert.ok(entries, 'Expected diagnostics to be set');
      assert.ok(entries.length > 0, 'Expected at least one diagnostic');
      assert.ok(entries[0]!.message.includes('exited with code 2'), 'Expected diagnostic message to mention exit code');

      // A successful subsequent refresh should clear the diagnostic.
      writeFixture(
        binaryPath,
        JSON.stringify([{ namespace: 'default', path: 'wiki', abs_path: path.join(tempDir, 'wiki') }])
      );
      await cache.refresh();

      const afterSuccess = diagnostics.get(diagUri);
      // After delete() the VS Code DiagnosticCollection may return either
      // undefined or an empty array depending on the runtime environment.
      assert.ok(afterSuccess === undefined || afterSuccess.length === 0, 'Expected diagnostics cleared on success');
    } finally {
      diagnostics.dispose();
      fs.rmSync(tempDir, { recursive: true, force: true });
    }
  });
});
