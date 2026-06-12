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
 * Compare the source text of old and new `<pre class="mermaid">` elements.
 *
 * Used by the morphdom `onBeforeElUpdated` callback in `content.ts` to skip
 * updating `<pre class="mermaid" data-processed>` elements whose diagram source
 * text hasn't changed, preserving the rendered SVG output.
 *
 * @param oldEl - The existing DOM element (from the live document).
 * @param newEl - The new element (from the parsed HTML update).
 * @returns True when both elements are `<pre class="mermaid">` with identical
 *          text content. Returns `false` when arguments are missing, elements
 *          are not both `<pre class="mermaid">`, or text content differs.
 */
export function isDiagramSourceUnchanged(oldEl?: Element, newEl?: Element): boolean {
  if (oldEl == null || newEl == null) return false;
  if (oldEl.tagName !== 'PRE' || newEl.tagName !== 'PRE') return false;
  if (!oldEl.classList.contains('mermaid') || !newEl.classList.contains('mermaid')) return false;
  return oldEl.textContent === newEl.textContent;
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
