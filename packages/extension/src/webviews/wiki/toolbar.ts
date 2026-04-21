/// <reference lib="dom" />
/**
 * Toolbar for the wiki webview.
 *
 * Mounts the search hint and Edit button into `#toolbar` using
 * vscode-elements `<vscode-button>` web components.
 *
 * @summary Mounts the search hint and edit toolbar button into #toolbar.
 */

import '@vscode-elements/elements/dist/vscode-button/index.js';
import { post } from './messaging.js';

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

  const editBtn = document.getElementById('wiki-btn-edit');
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
