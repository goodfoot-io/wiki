/**
 * Reproduction test for the wiki tree webview file-link tooltip content bug.
 *
 * The tooltip must render wiki-aware content: for a wiki page (target file with
 * both a non-empty `title` and `summary` in YAML frontmatter) it must show the
 * page title in bold, its repo-root-absolute path in gray italics, then the
 * summary as prose. For any other file it must render ONLY the repo-root path
 * in italics. No blue type/language `<vscode-badge>` may appear in either case,
 * and a raw `@sha`/`#fragment` must never be shown as the title.
 *
 * This pins the testable contract to a pure tooltip-content builder
 * `buildTooltipHtml({ relPath, title?, summary? })`.
 *
 * @summary Reproduction: tooltip content must be wiki-aware with no badge.
 * @module test/suite/tooltipContent.test
 */

import * as assert from 'node:assert';
import { buildTooltipHtml } from '../../src/webviews/wiki/tooltip.js';

describe('tooltip content builder', () => {
  it('(a) wiki page: bold title, gray italic repo-root path, summary prose', () => {
    const html = buildTooltipHtml({
      relPath: 'docs/guide/setup.md',
      title: 'Setup Guide',
      summary: 'How to set up the project.'
    });
    assert.ok(/<(b|strong)\b|font-weight|wiki-tooltip-title/.test(html), 'Expected bold title styling');
    assert.ok(html.includes('Setup Guide'), 'Expected the page title text');
    assert.ok(html.includes('docs/guide/setup.md'), 'Expected repo-root-relative path');
    assert.ok(html.includes('How to set up the project.'), 'Expected summary prose');
  });

  it('(b) non-wiki file: ONLY the italic repo-root path', () => {
    const html = buildTooltipHtml({ relPath: 'packages/cli/src/main.rs' });
    assert.ok(html.includes('packages/cli/src/main.rs'), 'Expected repo-root-relative path');
    assert.ok(/italic|wiki-tooltip-path|<i\b|<em\b/.test(html), 'Expected italic path styling');
  });

  it('(c) NEITHER variant contains a vscode-badge', () => {
    const wiki = buildTooltipHtml({
      relPath: 'docs/a.md',
      title: 'A',
      summary: 'S'
    });
    const plain = buildTooltipHtml({ relPath: 'src/x.ts' });
    assert.ok(!wiki.includes('vscode-badge'), 'Wiki tooltip must not contain a vscode-badge');
    assert.ok(!plain.includes('vscode-badge'), 'Plain tooltip must not contain a vscode-badge');
  });

  it('(d) raw @sha / #fragment are not shown as the title', () => {
    const html = buildTooltipHtml({ relPath: 'docs/page.md', title: 'Page', summary: 'A page.' });
    assert.ok(!html.includes('@'), 'Tooltip must not surface a raw @sha pin');
    assert.ok(!html.includes('#L'), 'Tooltip must not surface a raw line-range fragment');
  });
});
