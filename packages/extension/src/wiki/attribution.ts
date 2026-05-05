/**
 * File-to-namespace attribution logic.
 *
 * Determines which wiki namespace owns a given file by walking parent
 * directories looking for `wiki.toml` and cross-referencing the result
 * against the namespace cache.
 *
 * @summary File-to-namespace attribution.
 */

import * as fs from 'node:fs';
import * as path from 'node:path';
import type { NamespaceCache } from './namespaceCache.js';

/**
 * Determine the namespace that owns `filePath`.
 *
 * Resolution order:
 *   1. Fast path — ask the cache directly (longest-prefix match).
 *   2. Walk parent directories from the file toward the workspace root,
 *      looking for the nearest `wiki.toml`. Read its `namespace` field.
 *   3. Cross-reference the resolved directory against the cache to get the
 *      canonical namespace label.
 *   4. If nothing is found, return `"default"`.
 *
 * @param filePath    - Absolute path to the file to attribute.
 * @param workspaceRoot - Absolute path to the workspace root.
 * @param cache       - Initialised NamespaceCache instance.
 * @returns The owning namespace label.
 */
export function attributeFileToNamespace(filePath: string, workspaceRoot: string, cache: NamespaceCache): string {
  // Fast path: cache already knows which namespace root contains this file.
  const cached = cache.resolveNamespaceForFile(filePath);
  if (cached !== null) {
    return cached;
  }

  // Walk parent directories toward the workspace root looking for wiki.toml.
  const resolvedRoot = path.resolve(workspaceRoot);
  let dir = path.dirname(path.resolve(filePath));

  while (dir.startsWith(resolvedRoot) || dir.length > resolvedRoot.length) {
    const wikiTomlPath = path.join(dir, 'wiki.toml');
    try {
      if (fs.existsSync(wikiTomlPath)) {
        const ns = readNamespaceFromWikiToml(wikiTomlPath);
        // If the resolved directory is a known namespace root, return the
        // canonical label from the cache.
        if (ns !== null) {
          const cachedInfo = cache.get(ns);
          if (cachedInfo !== undefined) {
            return cachedInfo.namespace;
          }
          return ns;
        }
        // wiki.toml exists but has no namespace field → default.
        return 'default';
      }
    } catch {
      // Ignore individual file errors and continue walking up.
      void 0;
    }

    const parent = path.dirname(dir);
    if (parent === dir) break; // Reached filesystem root.
    dir = parent;
  }

  // Fallback: re-check cache (the file might be inside a namespace root
  // that was added after the initial fast-path check).
  const fallback = cache.resolveNamespaceForFile(filePath);
  if (fallback !== null) {
    return fallback;
  }

  return 'default';
}

/**
 * Read the `namespace` field from a `wiki.toml` file.
 *
 * @param wikiTomlPath - Absolute path to the wiki.toml file.
 * @returns The namespace value, or null if the field is absent or empty.
 */
function readNamespaceFromWikiToml(wikiTomlPath: string): string | null {
  const content = fs.readFileSync(wikiTomlPath, 'utf-8');
  const match = content.match(/^\s*namespace\s*=\s*"([^"]*)"\s*$/m);
  const value = match?.[1];
  if (value != null && value.length > 0) {
    return value;
  }
  return null;
}
