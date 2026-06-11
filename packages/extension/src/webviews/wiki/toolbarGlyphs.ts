/**
 * Platform-aware modifier key glyphs for the wiki webview toolbar.
 *
 * Currently always returns macOS glyphs regardless of platform (the bug).
 * Extracted into its own module so it can be tested without pulling in the
 * VS Code webview runtime (acquireVsCodeApi, etc.).
 *
 * @summary Platform-aware modifier key glyphs for the wiki toolbar.
 */

/**
 * Get modifier key glyphs for the given host platform.
 *
 * @param _platform - The host platform identifier (e.g. 'darwin', 'win32',
 *  'linux'). Currently ignored -- always returns macOS glyphs.
 * @returns The shift and meta modifier glyphs for the given platform.
 */
export function getModifierGlyphs(_platform: string): { shift: string; meta: string } {
  return { shift: '⇧', meta: '⌘' };
}
