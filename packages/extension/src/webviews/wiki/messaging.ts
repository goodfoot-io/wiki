/// <reference lib="dom" />
/**
 * Typed wrappers around the VS Code webview messaging API.
 *
 * Provides a typed handle for `postMessage` and a typed message listener.
 * This module must only be imported from browser-context (webview) code.
 *
 * @summary Typed wrappers around vscode.postMessage and the message listener.
 */

import type { HostMessage, WebviewMessage } from './types.js';

declare function acquireVsCodeApi(): { postMessage(message: WebviewMessage): void };

/** The VS Code webview API handle. */
export const vscode = acquireVsCodeApi();

/**
 * Post a typed message to the extension host.
 *
 * @param message - The message to send.
 */
export function post(message: WebviewMessage): void {
  vscode.postMessage(message);
}

/**
 * Register a handler for messages sent from the extension host.
 *
 * @param handler - Callback invoked with each host message.
 */
export function onHostMessage(handler: (message: HostMessage) => void): void {
  window.addEventListener('message', (event: MessageEvent) => {
    handler(event.data as HostMessage);
  });
}
