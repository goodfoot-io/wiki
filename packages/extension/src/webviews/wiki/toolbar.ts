/// <reference lib="dom" />
/**
 * Toolbar for the wiki webview.
 *
 * Mounts back, forward, and Edit buttons into `#toolbar` using
 * vscode-elements `<vscode-button>` web components.
 *
 * @summary Mounts back/forward/edit toolbar buttons into #toolbar.
 */

import '@vscode-elements/elements/dist/vscode-button/index.js';
import { post } from './messaging.js';

let backBtn: HTMLElement | null = null;
let forwardBtn: HTMLElement | null = null;

/**
 * Mount the toolbar into the `#toolbar` element.
 */
export function mount(): void {
  const toolbarEl = document.getElementById('toolbar');
  if (toolbarEl == null) return;

  toolbarEl.style.display = 'flex';
  toolbarEl.style.width = '100%';
  toolbarEl.style.justifyContent = 'space-between';
  toolbarEl.style.alignItems = 'center';
  toolbarEl.style.paddingBottom = '1em';

  toolbarEl.innerHTML = `
    <div style="display:flex;align-items:center">
      <vscode-button secondary icon="arrow-left" icon-only title="Go back" disabled id="wiki-btn-back" style="--vscode-button-border:transparent;--vscode-button-secondaryBackground:transparent"></vscode-button>
      <vscode-button secondary icon="arrow-right" icon-only title="Go forward" disabled id="wiki-btn-forward" style="--vscode-button-border:transparent;--vscode-button-secondaryBackground:transparent"></vscode-button>
    </div>
    <div id="wiki-btn-search-hint" style="display:flex;align-items:center;gap:3.6px;cursor:pointer">
      <kbd style="display:inline-flex;align-items:center;justify-content:center;font-size:10.8px;line-height:1;padding:1.2px 3.6px;border:1px solid var(--vscode-descriptionForeground);border-radius:2px;opacity:0.7;font-family:inherit">⇧</kbd>
      <kbd style="display:inline-flex;align-items:center;justify-content:center;font-size:10.8px;line-height:1;padding:1.2px 3.6px;border:1px solid var(--vscode-descriptionForeground);border-radius:2px;opacity:0.7;font-family:inherit">⌘</kbd>
      <kbd style="display:inline-flex;align-items:center;justify-content:center;font-size:10.8px;line-height:1;padding:1.2px 3.6px;border:1px solid var(--vscode-descriptionForeground);border-radius:2px;opacity:0.7;font-family:inherit">L</kbd>
      <span style="font-family:monospace;font-size:12px;color:var(--vscode-descriptionForeground);opacity:0.7;margin-left:3.6px">to search</span>
    </div>
    <div>
      <vscode-button secondary icon="edit" icon-only title="Edit" id="wiki-btn-edit" style="--vscode-button-border:transparent;--vscode-button-secondaryBackground:transparent"></vscode-button>
    </div>
  `;

  backBtn = document.getElementById('wiki-btn-back');
  forwardBtn = document.getElementById('wiki-btn-forward');
  const editBtn = document.getElementById('wiki-btn-edit');

  if (backBtn != null) {
    backBtn.addEventListener('click', () => {
      post({ type: 'goBack' });
    });
  }

  if (forwardBtn != null) {
    forwardBtn.addEventListener('click', () => {
      post({ type: 'goForward' });
    });
  }

  if (editBtn != null) {
    editBtn.addEventListener('click', (event: Event) => {
      const mouseEvent = event as MouseEvent;
      post({ type: 'openInEditor', split: mouseEvent.metaKey || mouseEvent.ctrlKey });
    });
  }

  const searchHint = document.getElementById('wiki-btn-search-hint');
  if (searchHint != null) {
    searchHint.addEventListener('click', () => {
      post({ type: 'openSearch' });
    });
  }
}

/**
 * Update the enabled/disabled state of the back button.
 *
 * @param canGoBack - Whether backward navigation is available.
 */
export function setCanGoBack(canGoBack: boolean): void {
  if (backBtn == null) return;
  if (canGoBack) {
    backBtn.removeAttribute('disabled');
  } else {
    backBtn.setAttribute('disabled', '');
  }
}

/**
 * Update the enabled/disabled state of the forward button.
 *
 * @param canGoForward - Whether forward navigation is available.
 */
export function setCanGoForward(canGoForward: boolean): void {
  if (forwardBtn == null) return;
  if (canGoForward) {
    forwardBtn.removeAttribute('disabled');
  } else {
    forwardBtn.setAttribute('disabled', '');
  }
}
