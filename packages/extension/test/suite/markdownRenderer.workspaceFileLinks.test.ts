/**
 * Regression test for wiki webview links to workspace files by bare filename.
 *
 * ## Behavior
 * Links like `[CLAUDE.md](CLAUDE.md)` or `[CLAUDE.md](CLAUDE.md#L63-L74)` in
 * markdown are rendered with bare hrefs (e.g. `<a href="CLAUDE.md#L63-L74">`).
 * The renderer does NOT produce `file:///` hrefs — it has no filesystem or
 * workspace context, and markdown-it preserves the bare path. It is the host's
 * navigate handler that resolves bare paths against the workspace root.
 *
 * ## Data flow
 * 1. Renderer: `[text](CLAUDE.md)` → `<a href="CLAUDE.md">` (bare path)
 * 2. Webview click handler: href lacks `file:///` → posts `navigate` message
 * 3. Host: resolves pageName as wiki page → not found → workspace fallback
 *    (WikiEditorProvider.ts:225-246) constructs `file:///` URI from workspace
 *    root and opens the file.
 *
 * ## Test
 * This test verifies that the renderer preserves bare hrefs (its correct
 * behavior). The host-side resolution is tested by
 * wikiEditorProvider.workspaceFallback.test.ts.
 *
 * @summary Regression test for bare-filename workspace links in wiki webview.
 * @module test/suite/markdownRenderer.workspaceFileLinks.test
 */

import * as assert from 'node:assert';
import { render } from '../../src/rendering/MarkdownRenderer.js';

describe('MarkdownRenderer — workspace file links', () => {
  describe('render()', () => {
    it('preserves bare hrefs for filename links without a scheme', () => {
      const html = render('See [CLAUDE.md](CLAUDE.md#L63-L74) for details.\n');

      // The renderer preserves the bare path — it does not have workspace
      // context. The host-side navigate handler resolves bare paths against
      // the workspace root (see wikiEditorProvider.workspaceFallback.test.ts).
      assert.ok(
        html.includes('href="CLAUDE.md#L63-L74"'),
        'Expected bare href without scheme, matching markdown-it default behavior. ' +
          'The host resolves workspace paths, not the renderer.'
      );
    });
  });
});
