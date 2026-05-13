/**
 * Core wiki types.
 *
 * @summary Wiki types shared between providers.
 */

/**
 * Frontmatter-derived metadata for a wiki-aware markdown file.
 *
 * A markdown file participates in wiki-aware language features (completion
 * ranking, hover summaries, link diagnostics) only when both `title` and
 * `summary` are present in its YAML frontmatter.
 */
export interface WikiInfo {
  /** Frontmatter title. */
  title: string;
  /** Frontmatter summary. */
  summary: string;
  /** Absolute filesystem path to the markdown file. */
  absPath: string;
}
