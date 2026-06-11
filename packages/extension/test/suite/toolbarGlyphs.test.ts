/**
 * Reproduction test for hard-coded macOS modifier glyphs in toolbar.ts.
 *
 * The toolbar's modifier key display must show platform-appropriate glyphs:
 * - macOS (darwin): `⇧` and `⌘`
 * - Windows (win32): `Shift` and `Ctrl`
 * - Linux: `Shift` and `Ctrl`
 *
 * This extracts the modifier glyph selection into a pure function
 * `getModifierGlyphs(platform)` and verifies that it respects the platform.
 * The current implementation always returns macOS glyphs regardless of the
 * platform argument, so the non-macOS assertions must fail.
 *
 * @summary Reproduction: toolbar always shows macOS modifier glyphs.
 * @module test/suite/toolbarGlyphs.test
 */

import * as assert from 'node:assert';
import { getModifierGlyphs } from '../../src/webviews/wiki/toolbarGlyphs.js';

describe('toolbar modifier glyphs', () => {
  it('(a) macOS: returns ⌘ and ⇧ glyphs', () => {
    const { shift, meta } = getModifierGlyphs('darwin');
    assert.strictEqual(shift, '⇧');
    assert.strictEqual(meta, '⌘');
  });

  it('(b) Windows: returns Ctrl and Shift text labels (NOT ⌘ and ⇧)', () => {
    const { shift, meta } = getModifierGlyphs('win32');
    // BUG: current implementation returns '⌘' for all platforms
    assert.strictEqual(meta, 'Ctrl');
    // BUG: current implementation returns '⇧' for all platforms
    assert.strictEqual(shift, 'Shift');
  });

  it('(c) Linux: returns Ctrl and Shift text labels (NOT ⌘ and ⇧)', () => {
    const { shift, meta } = getModifierGlyphs('linux');
    // BUG: current implementation returns '⌘' for all platforms
    assert.strictEqual(meta, 'Ctrl');
    // BUG: current implementation returns '⇧' for all platforms
    assert.strictEqual(shift, 'Shift');
  });
});
