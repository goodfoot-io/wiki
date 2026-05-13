/// <reference lib="dom" />
/**
 * Entry point for the wiki webview bundle.
 *
 * Wires together the toolbar, content, and messaging modules. Registers
 * delegated link click interception and host message handlers, then signals
 * readiness to the extension host.
 *
 * @summary Entry point for the wiki webview — wires toolbar, content, and messaging.
 */

import { getScrollY, patch, scrollTo } from './content.js';
import { renderDiagrams } from './diagrams.js';
import { onHostMessage, post } from './messaging.js';
import { mount as mountToolbar } from './toolbar.js';
import { hideTooltip, initTooltip, showFileTooltip } from './tooltip.js';
import type { HostMessage } from './types.js';
import '@vscode-elements/elements/dist/vscode-progress-ring/index.js';

// ---------------------------------------------------------------------------
// Toolbar
// ---------------------------------------------------------------------------

mountToolbar();
initTooltip();

/**
 * Internal links are plain markdown link hrefs whose target is a file path or
 * fragment relative to the source document. They are NOT absolute URLs.
 *
 * @param href - The href attribute to classify.
 * @returns True when the href is an internal (workspace-relative) link.
 */
function isInternalLink(href: string): boolean {
  return (
    href.length > 0 &&
    !href.startsWith('#') &&
    !href.startsWith('file:///') &&
    !href.startsWith('http://') &&
    !href.startsWith('https://') &&
    !href.startsWith('mailto:')
  );
}

// ---------------------------------------------------------------------------
// Delegated link hover — shows file-path tooltip after 250 ms
// ---------------------------------------------------------------------------

let hoverTimer: ReturnType<typeof setTimeout> | null = null;

document.addEventListener('mouseover', (e: MouseEvent) => {
  const anchor = (e.target as Element).closest('a');
  if (anchor == null) return;
  const href = anchor.getAttribute('href');
  if (href == null) return;
  if (hoverTimer != null) clearTimeout(hoverTimer);
  if (isInternalLink(href)) {
    hoverTimer = setTimeout(() => {
      showFileTooltip(anchor as HTMLElement, href);
    }, 250);
  }
});

document.addEventListener('mouseout', (e: MouseEvent) => {
  const anchor = (e.target as Element).closest('a');
  if (anchor == null) return;
  if (anchor.contains(e.relatedTarget as Node | null)) return;
  const href = anchor.getAttribute('href');
  if (href == null || !isInternalLink(href)) return;
  if (hoverTimer != null) {
    clearTimeout(hoverTimer);
    hoverTimer = null;
  }
  hideTooltip();
});

// ---------------------------------------------------------------------------
// Delegated link click interceptor
// ---------------------------------------------------------------------------

document.addEventListener('click', (e: MouseEvent) => {
  const anchor = (e.target as Element).closest('a');
  if (anchor == null) return;

  const href = anchor.getAttribute('href');
  if (href == null || href === '' || href.startsWith('#')) return;

  e.preventDefault();

  const split = e.metaKey || e.ctrlKey;

  if (href.startsWith('file:///')) {
    post({ type: 'openFile', uri: href, split });
  } else if (href.startsWith('http://') || href.startsWith('https://') || href.startsWith('mailto:')) {
    post({ type: 'openExternal', uri: href });
  } else {
    // Internal markdown link — the host resolves it relative to the
    // current document's directory.
    post({ type: 'navigate', href, split });
  }
});

// ---------------------------------------------------------------------------
// Debounced scroll position reporter
// ---------------------------------------------------------------------------

let scrollSaveTimeout: ReturnType<typeof setTimeout> | null = null;
window.addEventListener('scroll', () => {
  if (scrollSaveTimeout != null) clearTimeout(scrollSaveTimeout);
  scrollSaveTimeout = setTimeout(() => {
    post({ type: 'scrollPosition', y: getScrollY() });
  }, 200);
});

// ---------------------------------------------------------------------------
// Host message handler
// ---------------------------------------------------------------------------

onHostMessage((message: HostMessage) => {
  switch (message.type) {
    case 'updateContent': {
      const loadingEl = document.getElementById('loading');
      if (loadingEl != null) {
        loadingEl.style.display = 'none';
      }
      const errorEl = document.getElementById('error');
      if (errorEl != null) {
        errorEl.style.display = 'none';
      }
      hideTooltip();
      patch(message.html);
      void renderDiagrams();
      if (message.scrollY != null) {
        const y = message.scrollY;
        requestAnimationFrame(() => {
          scrollTo(y);
        });
      }
      break;
    }
    case 'showLoading': {
      const loadingEl = document.getElementById('loading');
      if (loadingEl != null) {
        loadingEl.style.display = '';
      }
      const errorEl = document.getElementById('error');
      if (errorEl != null) {
        errorEl.style.display = 'none';
      }
      break;
    }
    case 'showError': {
      const loadingEl = document.getElementById('loading');
      if (loadingEl != null) {
        loadingEl.style.display = 'none';
      }
      const errorEl = document.getElementById('error');
      if (errorEl != null) {
        errorEl.textContent = message.message;
        errorEl.style.display = '';
      }
      break;
    }
    default: {
      const _exhaustive: never = message;
      console.warn('[wiki-webview] Unhandled host message:', _exhaustive);
    }
  }
});

// ---------------------------------------------------------------------------
// Initialise
// ---------------------------------------------------------------------------

document.fonts.load('16px codicon').catch((err: unknown) => {
  console.warn('[wiki-webview] Failed to load codicon font:', err);
});

post({ type: 'ready' });
