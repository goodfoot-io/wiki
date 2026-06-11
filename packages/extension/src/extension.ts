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
import { formatLogError, getWikiLogger, registerWikiLogger } from './utils/logger.js';
import { WikiBinaryManager, wasManagedInstall } from './utils/wikiInstaller.js';

/**
 * Called by VS Code when the extension is activated.
 * Registers the wiki custom editor and commands.
 *
 * @param context - The VS Code extension context providing subscriptions and URIs.
 */
export function activate(context: vscode.ExtensionContext): void {
  registerWikiLogger(context);
  const log = getWikiLogger();
  log.info('Activating wiki extension v%s', context.extension.packageJSON.version);

  const binaryManager = new WikiBinaryManager(context);

  // ---------------------------------------------------------------------------
  // Language feature providers (completions, hover, diagnostics, references, rename)
  // ---------------------------------------------------------------------------
  const languageFeatures = new WikiLanguageFeatures(binaryManager);
  context.subscriptions.push(...languageFeatures.register());

  const provider = new WikiEditorProvider(context.extensionUri, binaryManager, context);

  // ---------------------------------------------------------------------------
  // File-move rename: rewrite incoming markdown links when a `.md` file or
  // directory containing `.md` files moves.
  // ---------------------------------------------------------------------------
  context.subscriptions.push(
    vscode.workspace.onDidRenameFiles(async (event) => {
      try {
        const aggregate = new vscode.WorkspaceEdit();
        let hasEdits = false;

        /**
         * Merge a partial WorkspaceEdit into the aggregate.
         *
         * @param partial - A WorkspaceEdit from a single buildFileMoveEdit call.
         */
        const mergeEdits = (partial: vscode.WorkspaceEdit): void => {
          for (const [uri, edits] of partial.entries()) {
            for (const e of edits) {
              aggregate.replace(uri, e.range, e.newText);
              hasEdits = true;
            }
          }
        };

        for (const rename of event.files) {
          if (rename.oldUri.fsPath.endsWith('.md')) {
            // Single .md file rename — existing behavior.
            mergeEdits(await languageFeatures.buildFileMoveEdit(rename.oldUri.fsPath, rename.newUri.fsPath));
            continue;
          }

          // Check for a directory rename — enumerate .md files within it.
          let newStat: vscode.FileStat;
          try {
            newStat = await vscode.workspace.fs.stat(rename.newUri);
          } catch {
            continue;
          }
          if ((newStat.type & vscode.FileType.Directory) !== 0) {
            mergeEdits(await languageFeatures.buildDirectoryMoveEdit(rename.oldUri.fsPath, rename.newUri.fsPath));
          }
        }

        if (hasEdits) {
          await vscode.workspace.applyEdit(aggregate);
        }
      } catch (error) {
        log.error('onDidRenameFiles handler: %s', formatLogError(error));
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
      log.error('Failed to prepare managed wiki CLI: %s', formatLogError(error));
    });

  context.subscriptions.push(
    vscode.window.registerCustomEditorProvider('wiki.viewer', provider, {
      supportsMultipleEditorsPerDocument: true,
      webviewOptions: { retainContextWhenHidden: true, enableFindWidget: true }
    }),

    vscode.commands.registerCommand('wiki.search', () => wikiQuickPick(binaryManager, context)),

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
      (uri?: vscode.Uri, options?: vscode.TextDocumentShowOptions | vscode.ViewColumn) => {
        const resolvedUri = uri ?? vscode.window.activeTextEditor?.document.uri;
        if (!resolvedUri) {
          void vscode.window.showInformationMessage('Open a wiki file first to use this command.');
          return;
        }
        const showOptions: vscode.TextDocumentShowOptions =
          typeof options === 'number' ? { viewColumn: options, preview: false } : (options ?? { preview: false });
        return vscode.window.showTextDocument(resolvedUri, showOptions);
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
