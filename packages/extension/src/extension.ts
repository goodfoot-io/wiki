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
import { WikiLanguageFeatures } from './providers/WikiLanguageFeatures.js';
import { WikiBinaryManager, wasManagedInstall } from './utils/wikiInstaller.js';

/**
 * Called by VS Code when the extension is activated.
 * Registers the wiki custom editor and commands.
 *
 * @param context - The VS Code extension context providing subscriptions and URIs.
 */
export function activate(context: vscode.ExtensionContext): void {
  const binaryManager = new WikiBinaryManager(context);

  // ---------------------------------------------------------------------------
  // Language feature providers (completions, hover, diagnostics, references, rename)
  // ---------------------------------------------------------------------------
  const languageFeatures = new WikiLanguageFeatures(binaryManager);
  context.subscriptions.push(...languageFeatures.register());

  const provider = new WikiEditorProvider(context.extensionUri, binaryManager, context);

  // ---------------------------------------------------------------------------
  // File-move rename: rewrite incoming markdown links when a `.md` file moves.
  // ---------------------------------------------------------------------------
  context.subscriptions.push(
    vscode.workspace.onDidRenameFiles(async (event) => {
      const aggregate = new vscode.WorkspaceEdit();
      let hasEdits = false;
      for (const rename of event.files) {
        if (!rename.oldUri.fsPath.endsWith('.md')) continue;
        const partial = await languageFeatures.buildFileMoveEdit(rename.oldUri.fsPath, rename.newUri.fsPath);
        for (const [uri, edits] of partial.entries()) {
          for (const e of edits) {
            aggregate.replace(uri, e.range, e.newText);
            hasEdits = true;
          }
        }
      }
      if (hasEdits) {
        await vscode.workspace.applyEdit(aggregate);
      }
    })
  );

  // ---------------------------------------------------------------------------
  // Binary lifecycle
  // ---------------------------------------------------------------------------
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
 */
export function deactivate(): void {
  // No-op: provider cleans up per-panel in resolveCustomEditor.
}
