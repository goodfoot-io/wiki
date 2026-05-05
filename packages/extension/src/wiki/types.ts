/**
 * Core namespace types for multi-wiki support.
 *
 * @summary Wiki namespace types and contracts.
 */

/**
 * Describes a single wiki namespace discovered by `wiki namespaces --format json`.
 *
 * The CLI represents the default anonymous wiki with `null`; the cache keys
 * it as `"default"`.
 */
export interface WikiInfo {
  /** Canonical namespace label ("default" for the anonymous wiki). */
  namespace: string;
  /** Relative path from the workspace root. */
  path: string;
  /** Absolute filesystem path to the wiki root. */
  absPath: string;
}

/**
 * Result of parsing a wikilink that may carry an explicit namespace qualifier.
 *
 * Examples:
 *   [[ns:Title]]           → { namespace: "ns", title: "Title", fragment: null }
 *   [[ns:Title#fragment]]  → { namespace: "ns", title: "Title", fragment: "fragment" }
 *   [[Title]]              → { namespace: null, title: "Title", fragment: null }
 */
export interface ParsedWikilink {
  /** Namespace qualifier, or null when the wikilink has no `ns:` prefix. */
  namespace: string | null;
  /** The page title (everything after the optional `ns:` prefix and before any `#fragment`). */
  title: string;
  /** Optional fragment/anchor after `#`, or null when absent. */
  fragment: string | null;
}
