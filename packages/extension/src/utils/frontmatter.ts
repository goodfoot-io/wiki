/**
 * Shared YAML frontmatter parsing for wiki-aware file detection.
 *
 * @summary YAML-accurate frontmatter parser for `title` and `summary` fields.
 */

import { readFile } from 'node:fs/promises';

export interface FrontmatterInfo {
  title?: string;
  summary?: string;
}

// ── YAML scalar decoding ──────────────────────────────────────────────────────

/**
 * Decode a YAML double-quoted scalar value (the outer quotes have already been
 * stripped). Handles `\\`, `\"`, `\n`, `\t`, `\r`, `\uXXXX`, `\UXXXXXXXX`,
 * and `\xXX` escape sequences — matching serde_yaml / libyaml semantics.
 *
 * @param inner - The scalar content with outer double-quotes removed.
 * @returns The decoded string value.
 */
function decodeDoubleQuoted(inner: string): string {
  return inner.replace(
    /\\(["\\/bfnrt]|u[0-9a-fA-F]{4}|U[0-9a-fA-F]{8}|x[0-9a-fA-F]{2}|N|_|L|P|0|a|e|v)/g,
    (_, esc: string): string => {
      switch (esc[0]) {
        case '"':
          return '"';
        case '\\':
          return '\\';
        case '/':
          return '/';
        case 'b':
          return '\b';
        case 'f':
          return '\f';
        case 'n':
          return '\n';
        case 'r':
          return '\r';
        case 't':
          return '\t';
        case 'a':
          return '\x07';
        case 'e':
          return '\x1b';
        case 'v':
          return '\x0b';
        case '0':
          return '\0';
        case 'N':
          return '';
        case '_':
          return ' ';
        case 'L':
          return ' ';
        case 'P':
          return ' ';
        case 'u':
        case 'U':
        case 'x': {
          const cp = Number.parseInt(esc.slice(1), 16);
          // Guard against out-of-range code points (> 0x10FFFF) and lone surrogates
          // (0xD800–0xDFFF), which would cause String.fromCodePoint to throw RangeError.
          // Degrade to the Unicode replacement character to keep parseFrontmatter total.
          if (cp > 0x10ffff || (cp >= 0xd800 && cp <= 0xdfff)) {
            return '�';
          }
          return String.fromCodePoint(cp);
        }
        default:
          return esc;
      }
    }
  );
}

/**
 * Decode a plain (unquoted) YAML scalar: strip trailing inline comments and
 * trim surrounding whitespace.
 *
 * A `#` introduces a comment only when preceded by at least one space — this
 * matches the YAML 1.2 rule and serde_yaml behaviour.
 *
 * @param raw - The raw scalar value from the YAML line (after the key colon).
 * @returns The decoded string value with inline comments removed.
 */
function decodePlain(raw: string): string {
  const commentIdx = raw.search(/ #/);
  const value = commentIdx >= 0 ? raw.slice(0, commentIdx) : raw;
  return value.trim();
}

/**
 * Decode a YAML single-quoted scalar (outer quotes already stripped):
 * `''` is the only escape sequence, representing a literal single quote.
 *
 * @param inner - The scalar content with outer single-quotes removed.
 * @returns The decoded string value.
 */
function decodeSingleQuoted(inner: string): string {
  return inner.replace(/''/g, "'");
}

/**
 * Collect indented continuation lines for a block scalar (`|` or `>`),
 * starting immediately after the indicator line. Returns the folded/literal
 * text matching serde_yaml output (trailing newline stripped, no leading
 * newline).
 *
 * Only the subset of block-scalar features needed for `title`/`summary`
 * single-value strings is implemented (default chomping, auto-detect indent).
 *
 * @param lines - All lines of the YAML frontmatter block.
 * @param startIdx - Index of the first continuation line after the indicator.
 * @param style - `|` for literal, `>` for folded.
 * @returns The decoded block scalar text.
 */
function collectBlockScalar(lines: string[], startIdx: number, style: '|' | '>'): string {
  // Determine indent from the first non-empty continuation line.
  let indent = 0;
  for (let i = startIdx; i < lines.length; i++) {
    const ln = lines[i]!;
    if (ln.trim() === '') continue;
    const m = ln.match(/^(\s+)/);
    indent = m ? m[1]!.length : 0;
    break;
  }
  if (indent === 0) return '';

  const collected: string[] = [];
  for (let i = startIdx; i < lines.length; i++) {
    const ln = lines[i]!;
    if (ln.trim() === '') {
      collected.push('');
    } else if (ln.match(/^\s/)) {
      collected.push(ln.slice(indent));
    } else {
      break; // de-dented: end of block scalar
    }
  }

  // Strip trailing empty lines (clip chomping default).
  while (collected.length > 0 && collected[collected.length - 1] === '') {
    collected.pop();
  }

  if (style === '>') {
    // Folded: replace single newlines with spaces, keep paragraph breaks.
    return collected
      .reduce<string[]>((acc, ln) => {
        if (ln === '') {
          acc.push('\n');
        } else {
          const last = acc[acc.length - 1];
          if (last === undefined || last === '\n') {
            acc.push(ln);
          } else {
            acc[acc.length - 1] = `${last} ${ln}`;
          }
        }
        return acc;
      }, [])
      .join('');
  }
  // Literal: join with newlines.
  return collected.join('\n');
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/**
 * Find the byte offset of the start of a `---` close fence within `text`,
 * starting the search at `startPos`.  Handles both LF (`\n`) and CRLF (`\r\n`)
 * line endings by computing byte offsets directly from the original string.
 *
 * Mirrors the Rust CLI's `find_close_fence` (frontmatter.rs:280-303): only an
 * *exact* `---` line (with optional trailing `\r`) is accepted — `----`,
 * `---extra`, and `--- ` are correctly rejected.
 *
 * @param text - The raw file content to search within.
 * @param startPos - The byte offset at which to start the search.
 * @returns Offset of the `---` close fence, or -1 when no exact match is found.
 */
function findCloseFence(text: string, startPos: number): number {
  let offset = startPos;
  while (offset < text.length) {
    const newlineIdx = text.indexOf('\n', offset);
    if (newlineIdx < 0) return -1;
    // The line content excludes the \n and any \r right before it.
    const lineEnd = newlineIdx > offset && text[newlineIdx - 1] === '\r' ? newlineIdx - 1 : newlineIdx;
    const lineContent = text.slice(offset, lineEnd);
    if (lineContent === '---') return offset;
    offset = newlineIdx + 1;
  }
  return -1;
}

// ── Public API ────────────────────────────────────────────────────────────────

/**
 * Parse `---\nkey: value\n---` YAML frontmatter to extract title/summary.
 *
 * Decodes scalar values to match serde_yaml semantics:
 * - Double-quoted strings: standard YAML escape sequences (`\"`, `\n`, `\uXXXX`, …)
 * - Single-quoted strings: `''` → `'`
 * - Plain scalars: inline comments (` #…`) are stripped
 * - Block/folded scalars (`|`/`>`): continuation lines are collected and joined
 *
 * Accepts both LF (`---\n`) and CRLF (`---\r\n`) opening fences, and locates
 * the closing fence via exact `---` line matching (rejecting `----`, `---text`,
 * `--- `).
 *
 * @param text - Raw file contents.
 * @returns Parsed frontmatter info (empty when no frontmatter is present).
 */
export function parseFrontmatter(text: string): FrontmatterInfo {
  // Opening fence: must start with --- followed by \n or \r\n.
  if (!text.startsWith('---')) return {};
  const afterDash = text[3];
  if (afterDash === '\r') {
    // CRLF: ---\r\n
    if (text[4] !== '\n') return {};
  } else if (afterDash !== '\n') {
    return {};
  }

  const startOffset = afterDash === '\r' ? 5 : 4;
  const end = findCloseFence(text, startOffset);
  if (end < 0) return {};

  // Normalise CRLF to LF so the rest of the parser is line-ending agnostic.
  const block = text.slice(startOffset, end).replace(/\r\n/g, '\n');
  const lines = block.split('\n');
  const info: FrontmatterInfo = {};

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i]!;
    const m = line.match(/^([A-Za-z_][A-Za-z0-9_-]*)\s*:\s*(.*)$/);
    if (m == null) continue;
    const key = m[1]!;
    if (key !== 'title' && key !== 'summary') continue;

    const raw = m[2]!.trim();
    let value: string;

    if (raw === '|' || raw === '>' || raw === '|-' || raw === '>-' || raw === '|+' || raw === '>+') {
      const style = raw[0] as '|' | '>';
      value = collectBlockScalar(lines, i + 1, style);
    } else if (raw.startsWith('"') && raw.endsWith('"') && raw.length >= 2) {
      value = decodeDoubleQuoted(raw.slice(1, -1));
    } else if (raw.startsWith("'") && raw.endsWith("'") && raw.length >= 2) {
      value = decodeSingleQuoted(raw.slice(1, -1));
    } else {
      value = decodePlain(raw);
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

/**
 * Extract the first heading from markdown `text` for use as a tab-title
 * fallback when frontmatter `title` is absent or empty.
 *
 * Mirrors the CLI fallback order: first ATX H1 (`# …`), then first ATX
 * heading of any level (`## …`, `### …`, …).
 *
 * @param text - Raw markdown file contents.
 * @returns Heading text, or undefined when no headings are present.
 */
export function extractFirstHeading(text: string): string | undefined {
  // Search only the non-frontmatter portion to avoid treating `title:` as a heading.
  let body = text;
  if (text.startsWith('---')) {
    const afterDash = text[3];
    let startOffset: number;
    if (afterDash === '\r' && text[4] === '\n') {
      startOffset = 5;
    } else if (afterDash === '\n') {
      startOffset = 4;
    } else {
      startOffset = -1;
    }
    if (startOffset > 0) {
      const end = findCloseFence(text, startOffset);
      if (end >= 0) {
        const afterClose = end + 3;
        if (text[afterClose] === '\r') {
          body = text.slice(afterClose + 2);
        } else if (text[afterClose] === '\n') {
          body = text.slice(afterClose + 1);
        } else {
          body = text.slice(afterClose);
        }
      }
    }
  }

  let firstHeading: string | undefined;
  for (const line of body.split('\n')) {
    const m = line.match(/^(#{1,6})\s+(.+)/);
    if (m == null) continue;
    if (m[1] === '#') return m[2]!.trim(); // first H1 wins immediately
    if (firstHeading === undefined) firstHeading = m[2]!.trim();
  }
  return firstHeading;
}
