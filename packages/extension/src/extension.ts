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
  const suppressedTextOpens = new Set<string>();
  const openingInViewer = new Set<string>();

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

  const withSuppressedTextOpen = async (
    uri: vscode.Uri,
    open: () => Thenable<unknown> | Promise<unknown>
  ): Promise<void> => {
    const key = uri.toString();
    suppressedTextOpens.add(key);
    try {
      await open();
    } finally {
      setTimeout(() => suppressedTextOpens.delete(key), 0);
    }
  };
  const toShowOptions = (
    options?: vscode.TextDocumentShowOptions | vscode.ViewColumn
  ): vscode.TextDocumentShowOptions => {
    if (typeof options === 'number') {
      return { viewColumn: options, preview: false };
    }
    return options ?? { preview: false };
  };

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
      (uri: vscode.Uri, options?: vscode.TextDocumentShowOptions | vscode.ViewColumn) =>
        withSuppressedTextOpen(uri, () => vscode.window.showTextDocument(uri, toShowOptions(options)))
    ),

    vscode.window.onDidChangeActiveTextEditor((editor) => {
      if (editor == null) return;

      const uri = editor.document.uri;
      if (!provider.isWikiFile(uri)) return;

      const openInViewer = vscode.workspace.getConfiguration('wiki').get<boolean>('openFilesInViewer', true);
      if (!openInViewer) return;

      const activeTab = vscode.window.tabGroups.activeTabGroup.activeTab;
      if (activeTab?.input instanceof vscode.TabInputTextDiff) return;

      const key = uri.toString();
      if (suppressedTextOpens.has(key) || openingInViewer.has(key)) return;

      openingInViewer.add(key);
      void Promise.resolve(
        vscode.commands.executeCommand('vscode.openWith', uri, 'wiki.viewer', {
          viewColumn: editor.viewColumn,
          preview: false
        })
      ).finally(() => {
        openingInViewer.delete(key);
      });
    })
  );
}

/**
 * Called by VS Code when the extension is deactivated.
 * Individual webview panels dispose themselves via webviewPanel.onDidDispose.
 */
export function deactivate(): void {
  // No-op: provider cleans up per-panel in resolveCustomEditor.
}
