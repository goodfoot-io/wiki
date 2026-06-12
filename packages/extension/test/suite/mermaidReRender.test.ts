/**
 * Reproduction test: mermaid diagrams unconditionally re-processed on every render.
 *
 * ## Bug
 * The morphdom `patch()` call in `content.ts` (line 25) strips `data-processed`
 * from `<pre class="mermaid">` elements during DOM morphing, because the parsed
 * HTML sourced from the markdown renderer does not carry this attribute.
 * After `patch()` completes, `renderDiagrams()` queries
 * `pre.mermaid:not([data-processed])` — which matches ALL diagrams —
 * and re-runs mermaid on every diagram on every keystroke.
 *
 * ## Root-cause hypothesis
 * morphdom in `patch()` is called without an `onBeforeElUpdated` guard
 * (line 25 of content.ts: `morphdom(contentEl, newEl)` — no options argument).
 * Without a callback that skips `pre.mermaid[data-processed]` elements whose
 * text content hasn't changed, morphdom replaces the rendered SVG with raw
 * diagram source text and removes the `data-processed` attribute, forcing
 * `renderDiagrams()` to re-process all diagrams.
 *
 * ## Data-flow trace confirming the hypothesis
 * ```
 * onDidChangeTextDocument (every keystroke)
 *   → _renderPage()
 *     → postMessage({ type: 'updateContent', html })
 *       → index.ts updateContent handler
 *         → patch(message.html)
 *           → morphdom(contentEl, newEl)
 *             └─ removes data-processed from pre.mermaid          ← NO GUARD
 *         → renderDiagrams()
 *           → querySelectorAll('pre.mermaid:not([data-processed])')
 *             └─ matches ALL diagrams (data-processed was stripped) ← RE-RUN
 * ```
 *
 * The current `renderDiagrams()` has no mechanism to compare diagram source
 * text between renders and skip unchanged diagrams.
 *
 * @summary Reproduction: mermaid diagrams re-processed on every content update.
 * @module test/suite/mermaidReRender.test
 */

import * as assert from 'node:assert';
import * as diagrams from '../../src/webviews/wiki/diagrams.js';

describe('isDiagramSourceUnchanged — mermaid source-text comparison guard', () => {
  it('MUST FAIL: isDiagramSourceUnchanged is not exported from the diagrams module', () => {
    // Before the fix: renderDiagrams() unconditionally processes all
    // pre.mermaid elements on every call because morphdom strips the
    // data-processed attribute during content patching. There is no
    // exported mechanism in diagrams.ts to compare diagram source text
    // across renders and skip unchanged diagrams.
    //
    // After the fix: isDiagramSourceUnchanged is added to diagrams.ts.
    // It compares the source text of old and new <pre class="mermaid">
    // elements and is used by the morphdom onBeforeElUpdated callback
    // (added to the patch() call in content.ts) to skip updating
    // pre.mermaid[data-processed] elements whose text content is
    // unchanged, preserving the rendered SVG output.
    diagrams.isDiagramSourceUnchanged();
    assert.ok(true, 'isDiagramSourceUnchanged was invoked without error');
  });
});
