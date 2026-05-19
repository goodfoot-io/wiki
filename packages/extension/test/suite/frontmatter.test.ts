/**
 * Unit tests for `parseFrontmatter` YAML scalar decoding parity and
 * `extractFirstHeading` fallback behaviour (F1 + F2).
 *
 * These tests run inside the VS Code Extension Host but require no VS Code APIs —
 * they exercise pure utility functions directly.
 *
 * @summary Frontmatter YAML decode parity and heading-fallback unit tests.
 * @module test/suite/frontmatter.test
 */

import * as assert from 'node:assert';
import { extractFirstHeading, hasWikiFrontmatter, parseFrontmatter } from '../../src/utils/frontmatter.js';

// ── Helper ────────────────────────────────────────────────────────────────────

function fm(body: string): string {
  return `---\n${body}\n---\n`;
}

// ── parseFrontmatter — YAML scalar decode parity (F1) ────────────────────────

describe('parseFrontmatter — YAML scalar decode parity', () => {
  it('plain unquoted title (baseline — must remain unchanged)', () => {
    const { title } = parseFrontmatter(fm('title: Getting Started\nsummary: s'));
    assert.strictEqual(title, 'Getting Started');
  });

  it('double-quoted title with escaped double-quote', () => {
    const { title } = parseFrontmatter(fm('title: "Foo \\"Bar\\""\nsummary: s'));
    assert.strictEqual(title, 'Foo "Bar"');
  });

  it('plain title with inline comment is stripped', () => {
    const { title } = parseFrontmatter(fm('title: Getting Started # rename me\nsummary: s'));
    assert.strictEqual(title, 'Getting Started');
  });

  it('single-quoted title with doubled single-quote', () => {
    const { title } = parseFrontmatter(fm("title: 'It''s here'\nsummary: s"));
    assert.strictEqual(title, "It's here");
  });

  it('double-quoted title with unicode escape', () => {
    // é is é
    const { title } = parseFrontmatter(fm('title: "Caf\\u00E9"\nsummary: s'));
    assert.strictEqual(title, 'Café');
  });

  it('literal block scalar title (|)', () => {
    const text = fm('title: |\n  Hello World\nsummary: s');
    const { title } = parseFrontmatter(text);
    assert.strictEqual(title, 'Hello World');
  });

  it('folded block scalar title (>)', () => {
    const text = fm('title: >\n  Hello World\nsummary: s');
    const { title } = parseFrontmatter(text);
    assert.strictEqual(title, 'Hello World');
  });

  it('hasWikiFrontmatter gate: decoded title must be non-empty for gate to pass', () => {
    // A quoted title that decodes to a real string must still gate-pass.
    const { title, summary } = parseFrontmatter(fm('title: "My Page"\nsummary: "A description."'));
    assert.strictEqual(title, 'My Page');
    assert.strictEqual(summary, 'A description.');
    // Gate: both non-empty.
    assert.ok(title && title.length > 0);
    assert.ok(summary && summary.length > 0);
  });

  it('double-quoted title with \\n escape decodes to newline character', () => {
    const { title } = parseFrontmatter(fm('title: "Line1\\nLine2"\nsummary: s'));
    assert.strictEqual(title, 'Line1\nLine2');
  });

  it('out-of-range \\U escape does not throw and degrades to replacement character', () => {
    // \U00110000 exceeds the Unicode max (0x10FFFF) — must not throw RangeError
    assert.doesNotThrow(() => {
      parseFrontmatter(fm('title: "\\U00110000"\nsummary: s'));
    });
    const { title } = parseFrontmatter(fm('title: "\\U00110000"\nsummary: s'));
    assert.strictEqual(title, '�');
  });

  it('lone surrogate \\u escape does not throw and degrades to replacement character', () => {
    // \uD800 is a lone high surrogate — must not throw RangeError
    assert.doesNotThrow(() => {
      parseFrontmatter(fm('title: "\\uD800"\nsummary: s'));
    });
    const { title } = parseFrontmatter(fm('title: "\\uD800"\nsummary: s'));
    assert.strictEqual(title, '�');
  });

  it('hasWikiFrontmatter gate does not throw on out-of-range \\U escape', () => {
    // Verifies the gate path does not throw when the title contains a degraded escape.
    assert.doesNotThrow(() => {
      const info = parseFrontmatter(fm('title: "\\U00110000"\nsummary: "valid"'));
      hasWikiFrontmatter(info);
    });
  });
});

// ── extractFirstHeading — heading fallback (F2) ───────────────────────────────

describe('extractFirstHeading', () => {
  it('returns H1 text when present', () => {
    const text = '---\ntitle: T\nsummary: s\n---\n\n# My Heading\n\nBody.';
    assert.strictEqual(extractFirstHeading(text), 'My Heading');
  });

  it('returns first heading of any level when no H1 exists', () => {
    const text = '---\ntitle: T\nsummary: s\n---\n\n## Section\n\nBody.';
    assert.strictEqual(extractFirstHeading(text), 'Section');
  });

  it('prefers H1 over earlier lower-level heading', () => {
    const text = '## First\n\n# Actual Title\n';
    assert.strictEqual(extractFirstHeading(text), 'Actual Title');
  });

  it('returns undefined when no headings exist', () => {
    const text = 'Just a paragraph.\n';
    assert.strictEqual(extractFirstHeading(text), undefined);
  });

  it('does not treat frontmatter content as headings', () => {
    // The `title:` key in frontmatter must not be mistaken for a heading.
    const text = '---\ntitle: Not A Heading\nsummary: s\n---\n\n## Real Heading\n';
    assert.strictEqual(extractFirstHeading(text), 'Real Heading');
  });
});
