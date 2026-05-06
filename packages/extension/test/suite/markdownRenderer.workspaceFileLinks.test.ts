/**
 * Reproduction test for wiki webview links to workspace files by bare filename.
 *
 * ## Bug
 * Links like `[CLAUDE.md](CLAUDE.md)` or `[CLAUDE.md](CLAUDE.md#L63-L74)` render
 * as `<a href="CLAUDE.md#L63-L74">` — a bare path with no `file:///` scheme.
 * The webview click handler at `src/webviews/wiki/index.ts:116-130` classifies
 * this as a wikilink (it does not start with `file:///`, `http://`, `https://`,
 * or `mailto:`) and posts a `navigate` message instead of `openFile`, causing
 * "Could not find wiki page: CLAUDE.md".
 *
 * ## Hypothesis
 * The markdown renderer (MarkdownRenderer.ts) uses markdown-it with default link
 * rendering. For `[text](CLAUDE.md#L63-L74)`, markdown-it produces a bare href
 * with no scheme. The renderer should detect when a link target names a workspace
 * file and produce a `file:///` href — but it currently has no workspace context
 * to do so.
 *
 * ## Test
 * This test MUST FAIL against the current unfixed code, proving the hypothesis
 * that the renderer produces wikilink-style hrefs (bare paths without scheme)
 * for workspace-file links.
 *
 * @summary Reproduction test for bare-filename workspace links in wiki webview.
 * @module test/suite/markdownRenderer.workspaceFileLinks.test
 */

import * as assert from 'node:assert';
import { render } from '../../src/rendering/MarkdownRenderer.js';

describe('MarkdownRenderer — workspace file links', () => {
  describe('render()', () => {
    it('produces file:/// hrefs for bare filename links to workspace files', () => {
      const html = render('See [CLAUDE.md](CLAUDE.md#L63-L74) for details.\n');

      // Current (broken) output:
      //   <a href="CLAUDE.md#L63-L74">CLAUDE.md</a>
      //
      // Expected output:
      //   <a href="file:///CLAUDE.md#L63-L74">CLAUDE.md</a>
      //
      // Without the file:/// prefix, the webview click handler at
      // src/webviews/wiki/index.ts:116-130 falls through to the wikilink branch:
      //
      //   1. href.startsWith('file:///') → false
      //   2. starts with http/https/mailto → false
      //   3. else → post({ type: 'navigate', ... })
      //
      // This causes "Could not find wiki page: CLAUDE.md".
      assert.ok(
        html.includes('file:///'),
        'Expected file:/// href for workspace file link. ' +
          'Current output has bare href without scheme, proving the renderer ' +
          'produces wikilink-style hrefs for workspace-file links.'
      );
    });
  });
});
