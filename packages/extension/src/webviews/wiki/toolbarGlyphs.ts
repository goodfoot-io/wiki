/**
 * Platform-aware modifier key glyphs for the wiki webview toolbar.
 *
 * Returns platform-appropriate labels: macOS uses Unicode glyphs (⇧⌘) while
 * Windows and Linux use text labels (Shift/Ctrl).
 *
 * Extracted into its own module so it can be tested without pulling in the
 * VS Code webview runtime (acquireVsCodeApi, etc.).
 *
 * @summary Platform-aware modifier key glyphs for the wiki toolbar.
 */

/**
 * Get modifier key glyphs for the given host platform.
 *
 * @param platform - The host platform identifier ('darwin', 'win32', 'linux').
 * @returns The shift and meta modifier glyphs for the given platform.
 */
export function getModifierGlyphs(platform: string): { shift: string; meta: string } {
  switch (platform) {
    case 'win32':
      return { shift: 'Shift', meta: 'Ctrl' };
    case 'linux':
      return { shift: 'Shift', meta: 'Ctrl' };
    default:
      return { shift: '⇧', meta: '⌘' };
  }
}

/**
 * Map `navigator.platform` to the platform identifier used by
 * `getModifierGlyphs`.  Intended for use in the webview context (Electron
 * Chromium), where the host OS is reflected by the browser's navigator object.
 *
 * @returns 'darwin', 'win32', or 'linux'.
 */
export function platformFromNavigator(): string {
  const p = navigator.platform;
  if (p.startsWith('Mac')) return 'darwin';
  if (p.startsWith('Win')) return 'win32';
  return 'linux';
}
