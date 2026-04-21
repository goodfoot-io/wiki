/**
 * Discriminated union message types for the wiki webview host↔webview channel.
 *
 * Every message type defined here is both sent and received — no dead writes.
 *
 * @summary Discriminated union message types for the wiki webview host↔webview channel.
 */

// Tooltip metadata for a resolved wikilink, as returned by `wiki refs --format json`.
export type ResolvedRefEntry = {
  wikilink: string;
  title: string;
  file: string;
  summary: string;
  aliases: string[];
  tags: string[];
};

export type RefEntry = ResolvedRefEntry | { wikilink: string; error: string };

// Host -> Webview messages
export type HostMessage =
  | { type: 'updateContent'; html: string; scrollY?: number; refs?: RefEntry[] }
  | { type: 'showLoading' }
  | { type: 'showError'; message: string };

// Webview -> Host messages
export type WebviewMessage =
  | { type: 'navigate'; pageName: string; split: boolean }
  | { type: 'scrollPosition'; y: number }
  | { type: 'openInEditor'; split: boolean }
  | { type: 'openFile'; uri: string; split: boolean }
  | { type: 'openExternal'; uri: string }
  | { type: 'openSearch' }
  | { type: 'ready' };
