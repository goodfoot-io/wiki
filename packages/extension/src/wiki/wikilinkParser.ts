/**
 * Qualified wikilink parser.
 *
 * Parses `[[ns:Title]]`, `[[ns:Title#fragment]]`, and bare `[[Title]]`
 * into a structured result. Intended for use in Step 5 (webview messaging
 * and markdown rendering).
 *
 * @summary Qualified wikilink parser (stub for Step 5).
 */

import type { ParsedWikilink } from './types.js';

/**
 * Parse a wikilink string into its namespace, title, and optional fragment.
 *
 * Handles the following forms:
 *   `ns:Title`             → { namespace: "ns", title: "Title", fragment: null }
 *   `ns:Title#fragment`    → { namespace: "ns", title: "Title", fragment: "fragment" }
 *   `Title`                → { namespace: null, title: "Title", fragment: null }
 *   `Title|Display`        → { namespace: null, title: "Title", fragment: null }
 *   `Title#fragment`       → { namespace: null, title: "Title", fragment: "fragment" }
 *
 * Pipe syntax (`|Display`) is silently stripped from all forms.
 *
 * @param input - The raw wikilink content (everything between `[[` and `]]`).
 * @returns Parsed wikilink components.
 */
export function parseQualifiedWikilink(input: string): ParsedWikilink {
  const match = input.match(/^(?:([A-Za-z0-9_-]+):)?([^#[\]|]+?)(?:#([^\]|]+))?(?:\|[^[\]]*)?$/);

  if (match == null) {
    // Fallback: treat the entire input as a bare title.
    return { namespace: null, title: input, fragment: null };
  }

  return {
    namespace: match[1] ?? null,
    title: match[2]!,
    fragment: match[3] ?? null
  };
}
