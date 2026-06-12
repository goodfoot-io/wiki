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
import { isDiagramSourceUnchanged } from './diagrams.js';

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
  morphdom(contentEl, newEl, {
    onBeforeElUpdated: (fromEl, toEl) => {
      // Preserve already-rendered mermaid diagrams whose source text is
      // unchanged. Without this guard, morphdom replaces the rendered SVG
      // with raw diagram source and strips the `data-processed` attribute,
      // forcing renderDiagrams() to re-process every diagram on every
      // content update — even when only unrelated text changed.
      if (fromEl.hasAttribute('data-processed') && isDiagramSourceUnchanged(fromEl, toEl)) {
        return false;
      }
      return true;
    }
  });
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
