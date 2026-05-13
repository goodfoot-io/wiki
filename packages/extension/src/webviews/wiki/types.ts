/**
 * Discriminated union message types for the wiki webview host↔webview channel.
 *
 * Every message type defined here is both sent and received — no dead writes.
 *
 * @summary Discriminated union message types for the wiki webview host↔webview channel.
 */

/**
 * One incoming backlink as returned by `wiki refs --format json`.
 *
 * Each entry represents a single line on a page that links back to the
 * currently-viewed file.
 */
export type ResolvedRefEntry = {
  source_file: string;
  source_title: string;
  line: number;
  text: string;
};

// Host -> Webview messages
export type HostMessage =
  | { type: 'updateContent'; html: string; scrollY?: number; refs?: ResolvedRefEntry[] }
  | { type: 'showLoading' }
  | { type: 'showError'; message: string };

// Webview -> Host messages
export type WebviewMessage =
  | { type: 'navigate'; href: string; split: boolean }
  | { type: 'scrollPosition'; y: number }
  | { type: 'openInEditor'; split: boolean }
  | { type: 'openFile'; uri: string; split: boolean }
  | { type: 'openExternal'; uri: string }
  | { type: 'openSearch' }
  | { type: 'ready' };
