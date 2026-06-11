/**
 * Reproduction test: `parseFrontmatter` closing fence detection is too permissive.
 *
 * `indexOf('\n---', 4)` at line 188 matches ANY occurrence of the 4-character
 * sequence `\n---`, even when more characters follow the three dashes.  This
 * causes premature truncation of the frontmatter block.
 *
 * The Rust CLI's `find_close_fence` (frontmatter.rs:280-303) iterates line-by-line
 * and requires an exact `---` line match (`line_content == "---"`).  The TypeScript
 * parser has no concept of line-exact matching.
 *
 * Failure modes:
 * 1. A line content `----` (four dashes) matches because `\n---` is a prefix of
 *    `\n----`.  The Rust parser correctly rejects it (`"----" != "---"`).
 * 2. A line content `---text` (dashes followed by text) matches for the same
 *    reason.  The Rust parser rejects it (`"---text" != "---"`).
 *
 * When either pattern appears before a value like `summary`, that value is lost
 * from the parsed result because the block is truncated at the false fence.
 *
 * MUST FAIL against the current unfixed code because `String.prototype.indexOf`
 * has no mechanism to enforce an exact line match.
 *
 * @summary Reproduction test for permissive closing fence detection in
 *   parseFrontmatter (no line-exact matching).
 * @module test/suite/frontmatter.closingFencePermissive.test
 */

import * as assert from 'node:assert';
import { parseFrontmatter } from '../../src/utils/frontmatter.js';

// ── Helper ────────────────────────────────────────────────────────────────────

function fm(body: string): string {
  return `---\n${body}\n---\n`;
}

// ── Tests ─────────────────────────────────────────────────────────────────────

describe('parseFrontmatter — permissive closing fence detection', () => {
  it('extracts summary when line ---- (four dashes) appears before it', () => {
    // `indexOf('\n---', 4)` matches inside `\n----` at line 3, truncating
    // the block to just `title: My Page` before `summary: My Summary` is seen.
    const result = parseFrontmatter(fm('title: My Page\n----\nsummary: My Summary'));
    assert.strictEqual(result.title, 'My Page');
    assert.strictEqual(result.summary, 'My Summary');
  });

  it('extracts summary when line ---extra (dashes then text) appears before it', () => {
    // `indexOf('\n---', 4)` matches inside `\n---extra`, same premature
    // truncation as the `----` case.
    const result = parseFrontmatter(fm('title: My Page\n---extra\nsummary: My Summary'));
    assert.strictEqual(result.title, 'My Page');
    assert.strictEqual(result.summary, 'My Summary');
  });

  it('extracts summary when line --- (single trailing-space) appears before it', () => {
    // Not a test of whitespace handling inside the value: a line consisting of
    // `--- ` (three dashes and a trailing space) is not an exact `---` match
    // but `\n--- ` still matches `indexOf('\n---')`.
    const result = parseFrontmatter(fm('title: My Page\n--- \nsummary: My Summary'));
    assert.strictEqual(result.title, 'My Page');
    assert.strictEqual(result.summary, 'My Summary');
  });

  it('still parses normally when ---- line is at the end (after all values)', () => {
    // Regression guard: the bug only manifests when the false fence appears
    // BEFORE a required value.  When it appears after, values are still found.
    const result = parseFrontmatter(fm('title: My Page\nsummary: My Summary\n----\nkey: val'));
    assert.strictEqual(result.title, 'My Page');
    assert.strictEqual(result.summary, 'My Summary');
  });
});
