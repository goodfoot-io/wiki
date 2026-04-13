/**
 * Discriminated union message types for the wiki webview host↔webview channel.
 *
 * Every message type defined here is both sent and received — no dead writes.
 *
 * @summary Discriminated union message types for the wiki webview host↔webview channel.
 */

// Host -> Webview messages
export type HostMessage =
  | { type: 'updateContent'; html: string; scrollY?: number }
  | { type: 'getScrollPosition' }
  | { type: 'showLoading' }
  | { type: 'showError'; message: string }
  | { type: 'updateNavigation'; canGoBack: boolean; canGoForward: boolean };

// Webview -> Host messages
export type WebviewMessage =
  | { type: 'navigate'; pageName: string; split: boolean }
  | { type: 'scrollPosition'; y: number }
  | { type: 'openInEditor'; split: boolean }
  | { type: 'openFile'; uri: string; split: boolean }
  | { type: 'openExternal'; uri: string }
  | { type: 'openSearch' }
  | { type: 'goBack' }
  | { type: 'goForward' }
  | { type: 'ready' };
