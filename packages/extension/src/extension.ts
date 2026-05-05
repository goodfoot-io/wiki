/**
 * VS Code extension entry point for the standalone wiki viewer.
 *
 * Registers the wiki custom text editor provider (wiki.viewer) and commands
 * (wiki.search, wiki.openInEditor) on activation. Also initialises the
 * namespace cache, file watchers for wiki.toml changes, and a status bar
 * item showing the active document's namespace.
 *
 * @summary VS Code extension entry point for the standalone wiki viewer.
 */

import * as vscode from 'vscode';
import { wikiQuickPick } from './commands/wikiQuickPick.js';
import { WikiEditorProvider } from './providers/WikiEditorProvider.js';
import { WikiBinaryManager, wasManagedInstall } from './utils/wikiInstaller.js';
import { attributeFileToNamespace } from './wiki/attribution.js';
import { NamespaceCache } from './wiki/namespaceCache.js';

/**
 * Debounce a function: wait `delay` ms after the last call before invoking.
 *
 * @param fn   - The function to debounce.
 * @param delay - Debounce delay in milliseconds.
 * @returns Debounced wrapper.
 */
function debounce(fn: () => void, delay: number): () => void {
  let timer: ReturnType<typeof setTimeout> | undefined;
  return () => {
    if (timer !== undefined) {
      clearTimeout(timer);
    }
    timer = setTimeout(() => {
      timer = undefined;
      fn();
    }, delay);
  };
}

/**
 * Called by VS Code when the extension is activated.
 * Registers the wiki custom editor and commands.
 *
 * @param context - The VS Code extension context providing subscriptions and URIs.
 */
export function activate(context: vscode.ExtensionContext): void {
  const binaryManager = new WikiBinaryManager(context);

  // ---------------------------------------------------------------------------
  // Namespace cache
  // ---------------------------------------------------------------------------
  const diagnosticsCollection = vscode.languages.createDiagnosticCollection('wiki-namespaces');
  context.subscriptions.push(diagnosticsCollection);

  const namespaceCache = new NamespaceCache(binaryManager, diagnosticsCollection);

  const provider = new WikiEditorProvider(context.extensionUri, binaryManager, context, namespaceCache);

  // ---------------------------------------------------------------------------
  // Status bar item
  // ---------------------------------------------------------------------------
  const statusBarItem = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Right, 100);
  statusBarItem.command = 'wiki.search'; // Deferred: namespace-picker command in follow-on.
  context.subscriptions.push(statusBarItem);

  /**
   * Update the status bar to reflect the active editor's namespace.
   */
  function updateNamespaceStatusBar(): void {
    const editor = vscode.window.activeTextEditor;
    if (editor == null) {
      statusBarItem.hide();
      return;
    }

    const workspaceFolders = vscode.workspace.workspaceFolders;
    if (workspaceFolders == null || workspaceFolders.length === 0) {
      statusBarItem.hide();
      return;
    }

    const wsRoot = workspaceFolders[0]!.uri.fsPath;
    const ns = attributeFileToNamespace(editor.document.uri.fsPath, wsRoot, namespaceCache);
    statusBarItem.text = `$(book) Wiki: ${ns}`;
    statusBarItem.tooltip = `Namespace: ${ns}`;
    statusBarItem.show();
  }

  // Initial update and subscribe to editor changes.
  context.subscriptions.push(
    vscode.window.onDidChangeActiveTextEditor(() => {
      updateNamespaceStatusBar();
    })
  );

  // ---------------------------------------------------------------------------
  // File watcher for wiki.toml (namespace configuration changes)
  // ---------------------------------------------------------------------------
  const watcher = vscode.workspace.createFileSystemWatcher('**/wiki.toml');
  const refreshNamespaceCache = debounce(() => {
    void namespaceCache.refresh().then(() => {
      updateNamespaceStatusBar();
    });
  }, 500);

  watcher.onDidChange(() => refreshNamespaceCache());
  watcher.onDidCreate(() => refreshNamespaceCache());
  watcher.onDidDelete(() => refreshNamespaceCache());
  context.subscriptions.push(watcher);

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

      // Initial namespace refresh after the binary is ready.
      void namespaceCache.refresh().then(() => {
        updateNamespaceStatusBar();
      });
    })
    .catch((error) => {
      console.error('[wiki-extension] Failed to prepare managed wiki CLI:', error);
    });

  context.subscriptions.push(
    vscode.window.registerCustomEditorProvider('wiki.viewer', provider, {
      supportsMultipleEditorsPerDocument: true,
      webviewOptions: { retainContextWhenHidden: true, enableFindWidget: true }
    }),

    vscode.commands.registerCommand('wiki.search', () => wikiQuickPick(binaryManager, namespaceCache)),

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
