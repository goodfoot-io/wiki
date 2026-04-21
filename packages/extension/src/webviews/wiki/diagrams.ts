/// <reference lib="dom" />
/**
 * Mermaid diagram rendering for the wiki webview.
 *
 * @summary Initialises mermaid.js and renders `pre.mermaid` elements after each content update.
 */

import mermaid from 'mermaid';

/**
 * Initialise mermaid with a theme matching the current VSCode colour theme.
 * Must be called once before `renderDiagrams`.
 */
export function initMermaid(): void {
  const isDark =
    document.body.classList.contains('vscode-dark') || document.body.classList.contains('vscode-high-contrast');
  mermaid.initialize({ startOnLoad: false, theme: isDark ? 'dark' : 'default' });
}

/**
 * Render all `pre.mermaid` elements in the document that have not yet been
 * processed by mermaid (identified by the absence of `data-processed`).
 *
 * Safe to call after every morphdom patch — already-rendered nodes are skipped.
 */
export async function renderDiagrams(): Promise<void> {
  const nodes = Array.from(document.querySelectorAll<HTMLElement>('pre.mermaid:not([data-processed])'));
  if (nodes.length === 0) return;
  try {
    await mermaid.run({ nodes });
  } catch (err) {
    console.error('[wiki-webview] mermaid render error:', err);
  }
}
