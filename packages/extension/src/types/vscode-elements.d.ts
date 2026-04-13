/**
 * TypeScript type definitions for @vscode-elements/elements custom elements.
 *
 * Ambient declarations for the vscode-button and vscode-progress-ring web
 * components used in the wiki-extension webview. Registered as custom HTML
 * elements — typed here so DOM queries and innerHTML usage type-check cleanly.
 *
 * @summary Ambient element declarations for vscode-elements web components.
 */

// These custom elements are registered via side-effect imports in the webview
// bundle. No additional type augmentation is required for vanilla TS usage —
// HTMLElement covers all attribute setting. This file is kept as a placeholder
// for future typed query selector augmentation if needed.

export {};
