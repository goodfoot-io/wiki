/**
 * Shared YAML frontmatter parsing for wiki-aware file detection.
 *
 * @summary Minimal frontmatter parser for `title` and `summary` fields.
 */

import { readFile } from 'node:fs/promises';

export interface FrontmatterInfo {
  title?: string;
  summary?: string;
}

/**
 * Parse `---\nkey: value\n---` YAML frontmatter to extract title/summary.
 * Only string scalars are supported; quoted forms are unwrapped.
 *
 * @param text - Raw file contents.
 * @returns Parsed frontmatter info (empty when no frontmatter is present).
 */
export function parseFrontmatter(text: string): FrontmatterInfo {
  if (!text.startsWith('---\n')) return {};
  const end = text.indexOf('\n---', 4);
  if (end < 0) return {};
  const block = text.slice(4, end);
  const info: FrontmatterInfo = {};
  for (const line of block.split('\n')) {
    const m = line.match(/^([A-Za-z_][A-Za-z0-9_-]*)\s*:\s*(.*)$/);
    if (m == null) continue;
    const key = m[1]!;
    let value = m[2]!.trim();
    if ((value.startsWith('"') && value.endsWith('"')) || (value.startsWith("'") && value.endsWith("'"))) {
      value = value.slice(1, -1);
    }
    if (key === 'title') info.title = value;
    if (key === 'summary') info.summary = value;
  }
  return info;
}

/**
 * Read `absPath` from disk and parse its frontmatter.
 *
 * @param absPath - Absolute filesystem path to a markdown file.
 * @returns Parsed frontmatter, or null if the file cannot be read.
 */
export async function readFrontmatter(absPath: string): Promise<FrontmatterInfo | null> {
  try {
    const text = await readFile(absPath, 'utf8');
    return parseFrontmatter(text);
  } catch {
    return null;
  }
}

/**
 * A markdown file is wiki-aware only when it carries both a non-empty
 * `title` and a non-empty `summary` in its YAML frontmatter.
 *
 * @param info - Parsed frontmatter, or null when the file was unreadable.
 * @returns True when both `title` and `summary` are present and non-empty.
 */
export function hasWikiFrontmatter(info: FrontmatterInfo | null): boolean {
  if (info == null) return false;
  return (
    typeof info.title === 'string' &&
    info.title.length > 0 &&
    typeof info.summary === 'string' &&
    info.summary.length > 0
  );
}
