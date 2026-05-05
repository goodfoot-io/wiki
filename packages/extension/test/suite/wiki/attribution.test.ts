/**
 * Tests for attributeFileToNamespace.
 *
 * Verifies parent-directory walking, wiki.toml parsing, and cache
 * cross-referencing logic.
 *
 * @summary File-to-namespace attribution tests.
 * @module test/suite/wiki/attribution.test
 */

import * as assert from 'node:assert';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import * as vscode from 'vscode';
import type { WikiBinaryManager } from '../../../src/utils/wikiInstaller.js';
import { attributeFileToNamespace } from '../../../src/wiki/attribution.js';
import { NamespaceCache } from '../../../src/wiki/namespaceCache.js';

/**
 * Create a minimal WikiBinaryManager that returns a fixed binary path.
 *
 * Uses a double type assertion because WikiBinaryManager has private members
 * which prevent plain object literals from being structurally assignable.
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
 */
async function createCache(
  tempDir: string,
  entries: ReadonlyArray<{ namespace: string | null; path: string; abs_path: string }>
): Promise<NamespaceCache> {
  const binaryPath = path.join(tempDir, 'wiki-ns');
  writeFixture(binaryPath, JSON.stringify(entries));
  const diagnostics = vscode.languages.createDiagnosticCollection('test');
  const cache = new NamespaceCache(createTestManager(binaryPath), diagnostics);
  await cache.refresh();
  diagnostics.dispose();
  return cache;
}

describe('attributeFileToNamespace', () => {
  it('returns namespace from nearest wiki.toml', async () => {
    const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'attr-test-'));
    try {
      const workspaceRoot = path.join(tempDir, 'workspace');
      const subDir = path.join(workspaceRoot, 'sub');
      fs.mkdirSync(subDir, { recursive: true });
      fs.writeFileSync(path.join(workspaceRoot, 'wiki.toml'), 'namespace = "myns"\n');
      fs.writeFileSync(path.join(subDir, 'file.md'), '# Test\n');

      // Empty cache so the fast path yields null and directory walking is used.
      const cache = await createCache(tempDir, []);
      const result = attributeFileToNamespace(path.join(subDir, 'file.md'), workspaceRoot, cache);
      assert.strictEqual(result, 'myns');
    } finally {
      fs.rmSync(tempDir, { recursive: true, force: true });
    }
  });

  it('returns "default" when wiki.toml has no namespace field', async () => {
    const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'attr-test-'));
    try {
      const workspaceRoot = path.join(tempDir, 'workspace');
      fs.mkdirSync(workspaceRoot, { recursive: true });
      fs.writeFileSync(path.join(workspaceRoot, 'wiki.toml'), 'key = "value"\n');
      fs.writeFileSync(path.join(workspaceRoot, 'file.md'), '# Test\n');

      const cache = await createCache(tempDir, []);
      const result = attributeFileToNamespace(path.join(workspaceRoot, 'file.md'), workspaceRoot, cache);
      assert.strictEqual(result, 'default');
    } finally {
      fs.rmSync(tempDir, { recursive: true, force: true });
    }
  });

  it('returns "default" when no wiki.toml is found', async () => {
    const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'attr-test-'));
    try {
      const workspaceRoot = path.join(tempDir, 'workspace');
      fs.mkdirSync(workspaceRoot, { recursive: true });
      fs.writeFileSync(path.join(workspaceRoot, 'file.md'), '# Test\n');

      const cache = await createCache(tempDir, []);
      const result = attributeFileToNamespace(path.join(workspaceRoot, 'file.md'), workspaceRoot, cache);
      assert.strictEqual(result, 'default');
    } finally {
      fs.rmSync(tempDir, { recursive: true, force: true });
    }
  });

  it('cross-references resolved directory against the cache', async () => {
    const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'attr-test-'));
    try {
      // The file and wiki.toml live at tempDir. The cache has "docs" at a
      // *different* absPath so the fast path does NOT match, forcing the
      // function through the directory walk -> cache cross-reference path.
      const workspaceRoot = tempDir;
      fs.writeFileSync(path.join(tempDir, 'wiki.toml'), 'namespace = "docs"\n');
      fs.writeFileSync(path.join(tempDir, 'file.md'), '# Test\n');

      // Cache has "docs" but pointing to a different directory.
      const otherDir = path.join(tempDir, 'other-ns');
      const cache = await createCache(tempDir, [{ namespace: 'docs', path: 'other-ns', abs_path: otherDir }]);

      const result = attributeFileToNamespace(path.join(tempDir, 'file.md'), workspaceRoot, cache);
      assert.strictEqual(result, 'docs');
    } finally {
      fs.rmSync(tempDir, { recursive: true, force: true });
    }
  });

  it('walks parent directories from file toward workspace root', async () => {
    const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'attr-test-'));
    try {
      const workspaceRoot = path.join(tempDir, 'workspace');
      const nestedDir = path.join(workspaceRoot, 'a', 'b', 'c');
      fs.mkdirSync(nestedDir, { recursive: true });
      fs.writeFileSync(path.join(workspaceRoot, 'wiki.toml'), 'namespace = "rootns"\n');
      fs.writeFileSync(path.join(nestedDir, 'file.md'), '# Test\n');

      const cache = await createCache(tempDir, []);
      const result = attributeFileToNamespace(path.join(nestedDir, 'file.md'), workspaceRoot, cache);
      assert.strictEqual(result, 'rootns');
    } finally {
      fs.rmSync(tempDir, { recursive: true, force: true });
    }
  });
});
