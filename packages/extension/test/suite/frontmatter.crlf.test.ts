/**
 * Reproduction test: CRLF line endings cause parseFrontmatter to return empty.
 *
 * The opening-fence check at line 187 expects LF-only (`---\n`). CRLF files
 * (`---\r\n`) fail this check, causing parseFrontmatter to return `{}`
 * and hasWikiFrontmatter to return false.
 *
 * Similarly, extractFirstHeading at line 267 uses the same LF-only check and
 * fails to skip CRLF frontmatter when extracting the body.
 *
 * MUST FAIL against the unfixed code because the parser assumes LF-only line
 * endings and is blind to the `\r` in CRLF files.
 *
 * @summary Reproduction test for CRLF frontmatter parsing failure.
 * @module test/suite/frontmatter.crlf.test
 */

import * as assert from 'node:assert';
import { extractFirstHeading, hasWikiFrontmatter, parseFrontmatter } from '../../src/utils/frontmatter.js';

// ── Helper ────────────────────────────────────────────────────────────────────

/**
 * Build a CRLF-delimited frontmatter block. Converts LF in `body` to CRLF
 * and wraps with CRLF fence markers.
 *
 * @param body - The frontmatter body content with LF line endings.
 * @returns The complete frontmatter block with CRLF line endings.
 */
function fmCRLF(body: string): string {
  return `---\r\n${body.replace(/\n/g, '\r\n')}\r\n---\r\n`;
}

// ── parseFrontmatter — CRLF regression tests ───────────────────────────────────

describe('parseFrontmatter — CRLF line endings', () => {
  it('parses CRLF plain unquoted title and summary', () => {
    const result = parseFrontmatter(fmCRLF('title: Getting Started\nsummary: A guide.'));
    assert.strictEqual(result.title, 'Getting Started');
    assert.strictEqual(result.summary, 'A guide.');
  });

  it('parses CRLF double-quoted title', () => {
    const result = parseFrontmatter(fmCRLF('title: "My Page"\nsummary: "A description."'));
    assert.strictEqual(result.title, 'My Page');
    assert.strictEqual(result.summary, 'A description.');
  });

  it('parses CRLF block scalar title (|)', () => {
    const text = fmCRLF('title: |\n  Hello World\nsummary: s');
    const result = parseFrontmatter(text);
    assert.strictEqual(result.title, 'Hello World');
  });

  it('hasWikiFrontmatter returns true for valid CRLF frontmatter', () => {
    const info = parseFrontmatter(fmCRLF('title: My Title\nsummary: My Summary'));
    assert.strictEqual(hasWikiFrontmatter(info), true);
  });
});

// ── extractFirstHeading — CRLF regression tests ────────────────────────────────

describe('extractFirstHeading — CRLF frontmatter', () => {
  it('returns H1 text after CRLF frontmatter', () => {
    const text = '---\r\ntitle: T\r\nsummary: s\r\n---\r\n\r\n# My Heading\r\n\r\nBody.';
    assert.strictEqual(extractFirstHeading(text), 'My Heading');
  });

  it('returns first heading when no H1 exists after CRLF frontmatter', () => {
    const text = '---\r\ntitle: T\r\nsummary: s\r\n---\r\n\r\n## Section\r\n\r\nBody.';
    assert.strictEqual(extractFirstHeading(text), 'Section');
  });
});
