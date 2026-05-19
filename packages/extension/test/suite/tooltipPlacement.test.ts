/**
 * Reproduction test for the wiki tree-webview tooltip mispositioning bug.
 *
 * ## Bug
 * The file-link tooltip must always appear ABOVE the hovered link without
 * overlapping it, and re-orient horizontally at window edges:
 *   - link near the right edge → box extends leftward, downward arrow aligned
 *     to the right of the box, never clipped;
 *   - link near the left edge → box extends rightward, arrow aligned to the
 *     left, never clipped.
 *
 * Currently `positionAndShow()` (src/webviews/wiki/tooltip.ts:93-118) decides
 * `showAbove` from available vertical space (L103), so the tooltip flips BELOW
 * and over the link when the link is near the top; and it never re-orients
 * horizontally — it only centers + clamps the box and nudges the arrow inside.
 *
 * ## Seam under test
 * The geometry is currently inlined in `positionAndShow` and DOM-coupled, so
 * it cannot be unit tested. This test pins down the *intended* pure-geometry
 * contract by importing a proposed pure function:
 *
 *   computeTooltipPlacement(anchorRect, tipSize, viewport)
 *     -> { top, left, arrow: 'down' | 'up', arrowLeft }
 *
 * from `../../src/webviews/wiki/tooltip`. That function does not exist yet, so
 * it resolves to `undefined` and every assertion throws — an acceptable
 * failing reproduction: the contract does not exist.
 *
 * @summary Intended pure geometry contract for the always-above, edge-aware tooltip.
 * @module test/suite/tooltipPlacement.test
 */

import * as assert from 'node:assert';
import { computeTooltipPlacement } from '../../src/webviews/wiki/tooltip.js';

// Mirrors the layout constants in tooltip.ts.
const EDGE_MARGIN = 8;

interface Rect {
  top: number;
  left: number;
  width: number;
  height: number;
  bottom: number;
}

interface Size {
  width: number;
  height: number;
}

function rect(left: number, top: number, width: number, height: number): Rect {
  return { left, top, width, height, bottom: top + height };
}

const VIEWPORT: Size = { width: 1000, height: 800 };
const TIP: Size = { width: 240, height: 90 };

describe('computeTooltipPlacement — always-above geometry', () => {
  it('places the tooltip fully above the anchor for a mid-screen link', () => {
    const anchor = rect(400, 400, 120, 18);
    const p = computeTooltipPlacement(anchor, TIP, VIEWPORT);
    assert.strictEqual(p.arrow, 'down', 'arrow should point down at the anchor below it');
    assert.ok(
      p.top + TIP.height <= anchor.top,
      `tooltip bottom (${p.top + TIP.height}) must not overlap anchor top (${anchor.top})`
    );
  });

  it('STILL places the tooltip above even when the anchor is near the top edge', () => {
    // anchor.top is smaller than tipHeight — current code flips BELOW here.
    const anchor = rect(400, 20, 120, 18);
    const p = computeTooltipPlacement(anchor, TIP, VIEWPORT);
    assert.strictEqual(p.arrow, 'down', 'arrow must still point down (tooltip above)');
    assert.ok(
      p.top + TIP.height <= anchor.top,
      `tooltip must remain above the anchor; got top=${p.top}, anchor.top=${anchor.top}`
    );
  });

  it('re-orients leftward for a link near the right edge, arrow aligned right', () => {
    const anchor = rect(VIEWPORT.width - 40, 400, 30, 18);
    const p = computeTooltipPlacement(anchor, TIP, VIEWPORT);
    // Box must stay inside the viewport by EDGE_MARGIN.
    assert.ok(
      p.left + TIP.width <= VIEWPORT.width - EDGE_MARGIN,
      `box right (${p.left + TIP.width}) must be within viewport minus margin (${VIEWPORT.width - EDGE_MARGIN})`
    );
    assert.ok(p.left >= EDGE_MARGIN, `box left (${p.left}) must respect edge margin`);
    // Arrow must align toward the RIGHT side of the box (pointing at the
    // right-edge anchor), i.e. in the right half of the box.
    assert.ok(
      p.arrowLeft > TIP.width / 2,
      `arrow should be in the right half of the box; arrowLeft=${p.arrowLeft}, half=${TIP.width / 2}`
    );
  });

  it('re-orients rightward for a link near the left edge, arrow aligned left', () => {
    const anchor = rect(6, 400, 30, 18);
    const p = computeTooltipPlacement(anchor, TIP, VIEWPORT);
    assert.ok(p.left >= EDGE_MARGIN, `box left (${p.left}) must respect edge margin`);
    assert.ok(p.left + TIP.width <= VIEWPORT.width - EDGE_MARGIN, `box right must stay within viewport`);
    // Arrow must align toward the LEFT side of the box.
    assert.ok(
      p.arrowLeft < TIP.width / 2,
      `arrow should be in the left half of the box; arrowLeft=${p.arrowLeft}, half=${TIP.width / 2}`
    );
  });

  it('never lets the box exit the viewport by EDGE_MARGIN on either edge', () => {
    for (const left of [0, 2, VIEWPORT.width - 30, VIEWPORT.width - 5]) {
      const anchor = rect(left, 400, 30, 18);
      const p = computeTooltipPlacement(anchor, TIP, VIEWPORT);
      assert.ok(p.left >= EDGE_MARGIN, `left=${p.left} for anchor.left=${left}`);
      assert.ok(
        p.left + TIP.width <= VIEWPORT.width - EDGE_MARGIN,
        `right=${p.left + TIP.width} for anchor.left=${left}`
      );
    }
  });
});
