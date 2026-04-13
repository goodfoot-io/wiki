/**
 * Tests for the MarkdownRenderer rendering pipeline.
 *
 * Verifies front-matter stripping, GitHub-style heading IDs with duplicate
 * handling, block-level data-line source-map attributes, fenced code block
 * syntax highlighting, and basic paragraph rendering.
 *
 * @summary Unit tests for src/rendering/MarkdownRenderer.
 * @module test/suite/markdownRenderer.test
 */

import * as assert from 'node:assert';
import { render } from '../../src/rendering/MarkdownRenderer.js';

describe('MarkdownRenderer', () => {
  describe('render()', () => {
    it('renders a basic paragraph', () => {
      const html = render('Hello world');
      assert.ok(html.includes('<p'), 'Expected a <p> element');
      assert.ok(html.includes('Hello world'), 'Expected paragraph text');
    });

    it('strips YAML front matter from output', () => {
      const input = `---
title: My Page
tags: [foo, bar]
---

# Heading

Paragraph text.`;
      const html = render(input);
      assert.ok(!html.includes('title: My Page'), 'Expected front matter content to be absent');
      assert.ok(html.includes('Heading'), 'Expected heading content to be present');
      assert.ok(html.includes('Paragraph text.'), 'Expected paragraph content to be present');
    });

    it('adds GitHub-style id attributes to headings', () => {
      const html = render('# Hello World\n\n## Sub Heading\n');
      assert.ok(html.includes('id="hello-world"'), 'Expected id="hello-world" on h1');
      assert.ok(html.includes('id="sub-heading"'), 'Expected id="sub-heading" on h2');
    });

    it('adds -1, -2 suffixes for duplicate headings', () => {
      const html = render('# Foo\n\n# Foo\n\n# Foo\n');
      assert.ok(html.includes('id="foo"'), 'Expected first "foo" slug without suffix');
      assert.ok(html.includes('id="foo-1"'), 'Expected second "foo" slug with -1 suffix');
      assert.ok(html.includes('id="foo-2"'), 'Expected third "foo" slug with -2 suffix');
    });

    it('adds data-line attributes to block-level elements', () => {
      const html = render('# Heading\n\nParagraph.\n');
      assert.ok(html.includes('data-line="0"'), 'Expected data-line="0" on first block element');
      assert.ok(html.includes('data-line="2"'), 'Expected data-line="2" on second block element');
    });

    it('adds code-line class to block-level elements', () => {
      const html = render('Paragraph.\n');
      assert.ok(html.includes('code-line'), 'Expected code-line class on block element');
    });

    it('syntax-highlights fenced code blocks', () => {
      const html = render('```typescript\nconst x: number = 42;\n```\n');
      assert.ok(html.includes('<span'), 'Expected syntax-highlighted spans in code block');
    });

    it('produces independent slug state across separate render() calls', () => {
      const html1 = render('# Foo\n');
      const html2 = render('# Foo\n');
      assert.ok(html1.includes('id="foo"'), 'First call: expected id="foo"');
      assert.ok(html2.includes('id="foo"'), 'Second call: expected id="foo" (not id="foo-1")');
      assert.ok(!html2.includes('id="foo-1"'), 'Second call: slug state must not carry over from first call');
    });

    it('converts [[Page Name]] wikilinks to anchor elements', () => {
      const html = render('See [[Some Page]] for details.\n');
      assert.ok(html.includes('href="/Some%20Page"'), 'Expected href="/Some%20Page"');
      assert.ok(html.includes('>Some Page<'), 'Expected link text "Some Page"');
      assert.ok(!html.includes('[['), 'Expected wikilink brackets to be absent');
    });

    it('converts [[Target|Display Text]] wikilinks with custom display text', () => {
      const html = render('See [[Target Page|this page]] for details.\n');
      assert.ok(html.includes('href="/Target%20Page"'), 'Expected href="/Target%20Page"');
      assert.ok(html.includes('>this page<'), 'Expected custom display text');
    });

    it('leaves wikilinks inside fenced code blocks unmodified', () => {
      const html = render('```md\n[[Example]]\n```\n');
      assert.ok(html.includes('[[Example]]'), 'Expected raw wikilink syntax inside code block');
      assert.ok(!html.includes('href="/Example"'), 'Expected no anchor in code block');
    });

    it('leaves wikilinks inside inline code spans unmodified', () => {
      const html = render('Use the `[[Example]]` syntax.\n');
      assert.ok(html.includes('[[Example]]'), 'Expected raw wikilink syntax inside code span');
    });
  });
});
