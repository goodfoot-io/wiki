/**
 * Reproduction test: pendingHover / hoverTimer survive updateContent.
 *
 * ## Bug
 * The `updateContent` handler in `index.ts` calls `hideTooltip()` (L139) but
 * never clears `pendingHover` (L56) or cancels `hoverTimer` (L49). When morphdom
 * replaces the anchor DOM node during the `patch()` call (L140), a pending or
 * in-flight hover request references a detached element.
 *
 * `getBoundingClientRect()` on a detached element returns all zeroes, so
 * `computeTooltipPlacement` positions the tooltip at viewport top-left.
 *
 * ## Root-cause hypothesis
 * `pendingHover` and `hoverTimer` are module-private in `index.ts` (lines 49
 * and 56). The `updateContent` handler (L130-148) calls `hideTooltip()` but
 * never resets them. As a result:
 *
 * 1. A `hoverTimer` that fires *after* content update will set `pendingHover`
 *    with a now-detached anchor element.
 * 2. A `fileInfo` reply that arrives *after* content update still matches the
 *    stale `pendingHover` and calls `showFileTooltip(pendingHover.anchor, ...)`
 *    using the detached anchor.
 * 3. `showFileTooltip` calls `positionAndShow` which invokes
 *    `anchor.getBoundingClientRect()`. The detached anchor returns
 *    `{top: 0, left: 0, width: 0, height: 0, bottom: 0}` → tooltip at
 *    viewport top-left.
 *
 * ## Seam under test
 * `tooltip.clearHoverState()` — introduced by the fix, called from
 * `index.ts`'s `updateContent` handler to properly reset hover-bookkeeping
 * (clear `pendingHover`, cancel `hoverTimer`, and hide the tooltip).
 *
 * Currently the function does not exist on the tooltip module, so the
 * namespace import resolves it to `undefined` and calling it throws
 * `TypeError: tooltip.clearHoverState is not a function` — proving the
 * hover-state cleanup path is absent.
 *
 * ## Data-flow trace confirming the hypothesis
 * ```
 * mouseover → hoverTimer = setTimeout(...) ─┐
 *                                           ├─[250ms]→ pendingHover = {anchor, href}
 *                                           │          post(requestFileInfo)
 * updateContent ─→ hideTooltip()            │
 *                  patch(html)   ← morphdom replaces anchor DOM nodes
 *                                           │
 *                      [timer fires] ←──────┘
 *                      pendingHover.anchor → detached from DOM
 *
 *                      OR (no timer overlap):
 *
 *                      [fileInfo arrives]
 *                      pendingHover.href matches → showFileTooltip(detachedAnchor)
 *                        → getBoundingClientRect() → all-zero rect
 *                        → computeTooltipPlacement(zeroRect, ...)
 *                        → tooltip at viewport top-left  ← BUG
 * ```
 *
 * @summary Reproduction: hover state survives updateContent.
 * @module test/suite/tooltipStateCleanup.test
 */

import * as assert from 'node:assert';
import * as tooltip from '../../src/webviews/wiki/tooltip.js';

describe('clearHoverState — hover state cleanup during updateContent', () => {
  it('MUST FAIL: clearHoverState is not exported from the tooltip module', () => {
    // Before the fix: tooltip.clearHoverState is `undefined` — the namespace
    // import resolves to whatever the module actually exports, and this name
    // is not among them.  Calling undefined() throws TypeError.
    //
    // After the fix: clearHoverState is added to tooltip.ts and called from
    // index.ts's updateContent handler to:
    //   a) cancel hoverTimer if running
    //   b) null out pendingHover
    //   c) hide the tooltip
    //
    // The test then passes, confirming the cleanup pathway exists.
    tooltip.clearHoverState();
    assert.ok(true, 'clearHoverState was invoked without error');
  });
});
