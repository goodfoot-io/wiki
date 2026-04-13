/// <reference lib="dom" />
/**
 * Manages the `#content` div in the wiki webview.
 *
 * Uses morphdom for incremental DOM patching to preserve DOM state (focus,
 * scroll, custom element internals) across content updates.
 *
 * @summary Manages the #content div with incremental DOM patching via morphdom.
 */

import morphdom from 'morphdom';

/**
 * Incrementally patch the `#content` element with new HTML.
 *
 * @param html - The inner HTML to render inside the content div.
 */
export function patch(html: string): void {
  const contentEl = document.getElementById('content');
  if (contentEl == null) return;
  const parser = new DOMParser();
  const doc = parser.parseFromString(`<div id="content" class="markdown-body vscode-body">${html}</div>`, 'text/html');
  const newEl = doc.body.firstElementChild;
  if (newEl == null) return;
  morphdom(contentEl, newEl);
}

/**
 * Scroll the window to the given vertical position.
 *
 * @param y - The scroll position in pixels.
 */
export function scrollTo(y: number): void {
  window.scrollTo({ top: y, behavior: 'instant' as ScrollBehavior });
}

/**
 * Return the current vertical scroll position.
 *
 * @returns The current `window.scrollY` value.
 */
export function getScrollY(): number {
  return window.scrollY;
}
