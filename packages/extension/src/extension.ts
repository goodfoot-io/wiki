/**
 * VS Code extension entry point for the standalone wiki viewer.
 *
 * Registers the wiki custom text editor provider (wiki.viewer) and commands
 * (wiki.search, wiki.openInEditor) on activation.
 *
 * @summary VS Code extension entry point for the standalone wiki viewer.
 */

import * as vscode from 'vscode';
import { wikiQuickPick } from './commands/wikiQuickPick.js';
import { WikiEditorProvider } from './providers/WikiEditorProvider.js';
import { WikiBinaryManager, wasManagedInstall } from './utils/wikiInstaller.js';

/**
 * Called by VS Code when the extension is activated.
 * Registers the wiki custom editor and commands.
 *
 * @param context - The VS Code extension context providing subscriptions and URIs.
 */
export function activate(context: vscode.ExtensionContext): void {
  const binaryManager = new WikiBinaryManager(context);
  const provider = new WikiEditorProvider(context.extensionUri, binaryManager);

  void binaryManager
    .start()
    .then((result) => {
      if (wasManagedInstall(result)) {
        void vscode.window.showInformationMessage(
          '`wiki` is installed for this extension. New integrated terminals will have it on PATH.'
        );
      }
    })
    .catch((error) => {
      console.error('[wiki-extension] Failed to prepare managed wiki CLI:', error);
    });

  context.subscriptions.push(
    vscode.window.registerCustomEditorProvider('wiki.viewer', provider, {
      supportsMultipleEditorsPerDocument: true,
      webviewOptions: { retainContextWhenHidden: true, enableFindWidget: true }
    }),

    vscode.commands.registerCommand('wiki.search', () => wikiQuickPick(binaryManager)),

    vscode.commands.registerCommand('wiki.retryInstall', async () => {
      try {
        const result = await vscode.window.withProgress(
          { location: vscode.ProgressLocation.Notification, title: 'Installing wiki CLI…' },
          () => binaryManager.retry()
        );
        if (wasManagedInstall(result)) {
          void vscode.window.showInformationMessage(
            '`wiki` is installed for this extension. New integrated terminals will have it on PATH.'
          );
        }
      } catch (error) {
        void vscode.window.showErrorMessage(`Wiki: ${binaryManager.formatFailure(error)}`);
      }
    }),

    vscode.commands.registerCommand(
      'wiki.openInEditor',
      (uri: vscode.Uri, options?: vscode.TextDocumentShowOptions | vscode.ViewColumn) => {
        const showOptions: vscode.TextDocumentShowOptions =
          typeof options === 'number' ? { viewColumn: options, preview: false } : (options ?? { preview: false });
        return vscode.window.showTextDocument(uri, showOptions);
      }
    )
  );
}

/**
 * Called by VS Code when the extension is deactivated.
 * Individual webview panels dispose themselves via webviewPanel.onDidDispose.
 */
export function deactivate(): void {
  // No-op: provider cleans up per-panel in resolveCustomEditor.
}
