/// <reference lib="dom" />
/**
 * Mermaid diagram rendering for the wiki webview.
 *
 * Mermaid is loaded via a dynamic import so the browser only fetches its chunk
 * the first time a page contains a diagram. Pages with no diagrams pay zero cost.
 *
 * @summary Lazily loads mermaid.js and renders `pre.mermaid` elements after each content update.
 */

type MermaidModule = typeof import('mermaid');

let mermaidCache: MermaidModule | null = null;

async function getMermaid(): Promise<MermaidModule> {
  if (mermaidCache !== null) return mermaidCache;
  const mod = await import('mermaid');
  const isDark =
    document.body.classList.contains('vscode-dark') || document.body.classList.contains('vscode-high-contrast');
  mod.default.initialize({ startOnLoad: false, theme: isDark ? 'dark' : 'default' });
  mermaidCache = mod;
  return mod;
}

/**
 * Render all `pre.mermaid` elements in the document that have not yet been
 * processed by mermaid (identified by the absence of `data-processed`).
 *
 * Mermaid is imported lazily on first call. Safe to call after every morphdom
 * patch — already-rendered nodes are skipped and pages without diagrams return
 * immediately without fetching the mermaid chunk.
 */
export async function renderDiagrams(): Promise<void> {
  const nodes = Array.from(document.querySelectorAll<HTMLElement>('pre.mermaid:not([data-processed])'));
  if (nodes.length === 0) return;
  const { default: mermaid } = await getMermaid();
  try {
    await mermaid.run({ nodes });
  } catch (err) {
    console.error('[wiki-webview] mermaid render error:', err);
  }
}
