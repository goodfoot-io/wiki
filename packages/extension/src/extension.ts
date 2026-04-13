/**
 * VS Code extension entry point for the standalone wiki viewer.
 *
 * Registers the wiki custom editor provider (wiki.viewer) and commands
 * (wiki.search, wiki.openInEditor) on activation.
 *
 * @summary VS Code extension entry point for the standalone wiki viewer.
 */

import * as vscode from 'vscode';
import { wikiQuickPick } from './commands/wikiQuickPick.js';
import { WikiEditorProvider } from './providers/WikiEditorProvider.js';

/**
 * Called by VS Code when the extension is activated.
 * Registers the wiki custom editor and commands.
 *
 * @param context - The VS Code extension context providing subscriptions and URIs.
 */
export function activate(context: vscode.ExtensionContext): void {
  const provider = new WikiEditorProvider(context.extensionUri);

  context.subscriptions.push(
    vscode.window.registerCustomEditorProvider('wiki.viewer', provider, {
      webviewOptions: { retainContextWhenHidden: true }
    }),

    vscode.commands.registerCommand('wiki.search', () => wikiQuickPick()),

    vscode.commands.registerCommand('wiki.openInEditor', (uri: vscode.Uri) => vscode.window.showTextDocument(uri))
  );
}

/**
 * Called by VS Code when the extension is deactivated.
 * Individual webview panels dispose themselves via webviewPanel.onDidDispose.
 */
export function deactivate(): void {
  // No-op: provider cleans up per-panel in resolveCustomEditor.
}
