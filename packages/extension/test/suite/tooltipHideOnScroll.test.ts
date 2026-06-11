/**
 * Reproduction test for the wiki webview tooltip scroll-hide bug.
 *
 * ## Bug
 * The file-link tooltip uses `position: fixed` but is not hidden on scroll.
 * When the user scrolls while the pointer is over a link, the tooltip stays at
 * its fixed viewport position, floating over the wrong text because no scroll
 * listener calls `hideTooltip()`.
 *
 * ## Data flow
 * `index.ts` (L117-122) registers a debounced scroll event listener on `window`
 * that only posts the scroll position to the host — it does NOT call
 * `hideTooltip()`.  Meanwhile `hideTooltip()` (tooltip.ts L158-160), which
 * removes the `wiki-tooltip--visible` CSS class, IS imported in `index.ts`
 * (L16) and IS called on `mouseout` (L83) and on `updateContent` (L139), but
 * is NOT called on scroll.
 *
 * ## Reproduction strategy
 * Because `index.ts` uses browser DOM APIs (`window`, `document`) at module
 * level, it cannot be imported from Node.js test bundles.  This test reads the
 * source file directly and checks that the scroll event handler body includes
 * a `hideTooltip()` call.  The current code lacks this call, so the assertion
 * fails — encoding a contract the fix must satisfy.
 *
 * @summary Reproduction: the `position:fixed` tooltip is not hidden on scroll.
 * @module test/suite/tooltipHideOnScroll.test
 */

import * as assert from 'node:assert';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { hideTooltip } from '../../src/webviews/wiki/tooltip.js';

describe('tooltip hidden on scroll', () => {
  it('the scroll event handler must call hideTooltip()', () => {
    // 1. Verify hideTooltip is exported and is a function (pure-function import).
    assert.strictEqual(typeof hideTooltip, 'function', 'hideTooltip must be a function exported from tooltip.ts');

    // 2. Read index.ts source and verify the scroll handler calls hideTooltip.
    const sourcePath = path.resolve(__dirname, '../../../src/webviews/wiki/index.ts');
    const source = fs.readFileSync(sourcePath, 'utf-8');

    // Locate the window scroll event listener registration.
    const scrollIdx = source.indexOf(`window.addEventListener('scroll'`);
    assert.ok(scrollIdx >= 0, 'index.ts must register a scroll event listener on window');

    // Find the opening brace of the arrow-function body.
    const bodyStart = source.indexOf('{', scrollIdx);
    assert.ok(
      bodyStart >= 0 && bodyStart < scrollIdx + 300,
      'Scroll handler must have a function body within 300 chars of registration'
    );

    // Brace-count to find the matching closing brace.
    let depth = 0;
    let bodyEnd = -1;
    for (let i = bodyStart; i < source.length; i++) {
      if (source[i] === '{') depth++;
      if (source[i] === '}') {
        depth--;
        if (depth === 0) {
          bodyEnd = i;
          break;
        }
      }
    }
    assert.ok(bodyEnd >= 0, 'Scroll handler body must have a matching closing brace');

    const handlerBody = source.slice(bodyStart, bodyEnd + 1);

    // The scroll handler MUST call hideTooltip().  Currently it does not —
    // this assertion fails against the unfixed code.
    assert.ok(
      /hideTooltip\s*\(/.test(handlerBody),
      'Scroll event handler must call hideTooltip() to prevent the ' +
        'position:fixed tooltip from remaining visible at its original ' +
        'viewport position after the user scrolls'
    );
  });
});
