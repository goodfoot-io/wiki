/// <reference lib="dom" />
/**
 * Entry point for the wiki webview bundle.
 *
 * Wires together the toolbar, content, and messaging modules. Registers delegated
 * link click interception and host message handlers, then signals readiness to the
 * extension host.
 *
 * @summary Entry point for the wiki webview — wires toolbar, content, and messaging.
 */

import { getScrollY, patch, scrollTo } from './content.js';
import { initMermaid, renderDiagrams } from './diagrams.js';
import { onHostMessage, post } from './messaging.js';
import { mount as mountToolbar } from './toolbar.js';
import type { HostMessage } from './types.js';
import '@vscode-elements/elements/dist/vscode-progress-ring/index.js';

// ---------------------------------------------------------------------------
// Toolbar
// ---------------------------------------------------------------------------

mountToolbar();
initMermaid();

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
    // Source fragment link — open in text editor.
    post({ type: 'openFile', uri: href, split });
  } else if (href.startsWith('http://') || href.startsWith('https://') || href.startsWith('mailto:')) {
    // External link — ask host to open in system browser.
    post({ type: 'openExternal', uri: href });
  } else {
    // Wikilink: href is URL-encoded page name with leading slash, e.g. "/My%20Page".
    // Strip any heading anchor fragment — the host resolves page names, not fragment IDs.
    const decoded = decodeURIComponent(href.replace(/^\//, ''));
    const hashIdx = decoded.indexOf('#');
    const pageName = hashIdx >= 0 ? decoded.slice(0, hashIdx) : decoded;
    post({ type: 'navigate', pageName, split });
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

// Eagerly load the codicon font so vscode-elements icons render inside shadow DOM.
document.fonts.load('16px codicon').catch((err: unknown) => {
  console.warn('[wiki-webview] Failed to load codicon font:', err);
});

// Notify the host that the webview is ready to receive content.
post({ type: 'ready' });
