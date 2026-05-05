/**
 * Caches the namespace-to-wiki-root mapping discovered by `wiki namespaces`.
 *
 * On refresh, shells out to `wiki namespaces --format json`, parses the
 * result, and populates an in-memory map. Exposes lookup by namespace label
 * and by file path (longest-prefix match).
 *
 * Non-zero exits from the CLI are surfaced as a workspace diagnostic so the
 * user sees namespace discovery failures in the Problems panel.
 *
 * @summary Namespace discovery and caching.
 */

import * as vscode from 'vscode';
import { runWikiCommand } from '../utils/wikiBinary.js';
import type { WikiBinaryManager } from '../utils/wikiInstaller.js';
import type { WikiInfo } from './types.js';

/** Virtual document URI used to scope namespace-cache diagnostics. */
const DIAGNOSTIC_URI = vscode.Uri.parse('wiki://namespace-cache');

/**
 * In-memory cache of the `{namespace → WikiInfo}` mapping.
 *
 * Thread-safe for concurrent reads; refresh() uses an internal lock to
 * prevent overlapping updates.
 */
export class NamespaceCache {
  private readonly _namespaces = new Map<string, WikiInfo>();
  private readonly _binaryManager: WikiBinaryManager;
  private readonly _diagnostics: vscode.DiagnosticCollection;
  private _refreshing = false;

  constructor(binaryManager: WikiBinaryManager, diagnostics: vscode.DiagnosticCollection) {
    this._binaryManager = binaryManager;
    this._diagnostics = diagnostics;
  }

  /**
   * Re-fetch namespace data from `wiki namespaces --format json`.
   *
   * On success the cache is replaced atomically. On failure (non-zero exit,
   * parse error, or binary-not-ready) a diagnostic is surfaced and the
   * previous cache state is preserved.
   */
  async refresh(): Promise<void> {
    if (this._refreshing) return;
    this._refreshing = true;

    try {
      const handle = await this._binaryManager.ready();
      const { stdout, stderr, exitCode } = await runWikiCommand(handle.path, ['namespaces', '--format', 'json']);

      if (exitCode !== 0) {
        this._diagnostics.set(DIAGNOSTIC_URI, [
          new vscode.Diagnostic(
            new vscode.Range(0, 0, 0, 0),
            `wiki namespaces exited with code ${exitCode}: ${stderr}`,
            vscode.DiagnosticSeverity.Error
          )
        ]);
        return;
      }

      // Clear previous diagnostic on success.
      this._diagnostics.delete(DIAGNOSTIC_URI);

      const entries: Array<{ namespace: string | null; path: string; abs_path: string }> = JSON.parse(stdout);
      const next = new Map<string, WikiInfo>();

      for (const entry of entries) {
        const ns = entry.namespace ?? 'default';
        next.set(ns, {
          namespace: ns,
          path: entry.path,
          absPath: entry.abs_path
        });
      }

      // Atomic swap.
      this._namespaces.clear();
      for (const [key, value] of next) {
        this._namespaces.set(key, value);
      }
    } catch (error: unknown) {
      this._diagnostics.set(DIAGNOSTIC_URI, [
        new vscode.Diagnostic(
          new vscode.Range(0, 0, 0, 0),
          `Failed to refresh namespace cache: ${error instanceof Error ? error.message : String(error)}`,
          vscode.DiagnosticSeverity.Error
        )
      ]);
    } finally {
      this._refreshing = false;
    }
  }

  /**
   * Look up a single namespace by label.
   *
   * @param ns - Namespace label (e.g. `"default"`, `"mesh"`).
   * @returns The WikiInfo, or undefined when the namespace is not in the cache.
   */
  get(ns: string): WikiInfo | undefined {
    return this._namespaces.get(ns);
  }

  /**
   * Return all discovered namespaces.
   *
   * @returns An array of all cached WikiInfo entries.
   */
  getAll(): WikiInfo[] {
    return Array.from(this._namespaces.values());
  }

  /**
   * Find which namespace owns the given file path.
   *
   * Uses longest-prefix matching: the namespace whose `absPath` is the
   * longest prefix of `filePath` wins. Returns `null` when no namespace
   * root contains the file.
   *
   * @param filePath - Absolute filesystem path to check.
   * @returns The owning namespace label, or null.
   */
  resolveNamespaceForFile(filePath: string): string | null {
    let best: string | null = null;
    let bestLength = 0;

    for (const [ns, info] of this._namespaces) {
      const prefix = info.absPath.endsWith('/') ? info.absPath : `${info.absPath}/`;
      if (filePath.startsWith(prefix) && prefix.length > bestLength) {
        best = ns;
        bestLength = prefix.length;
      }
    }

    return best;
  }
}
