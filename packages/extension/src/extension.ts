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
import { WikiLanguageFeatures } from './providers/WikiLanguageFeatures.js';
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

  // ---------------------------------------------------------------------------
  // Language feature providers (completions, hover, diagnostics, references, rename)
  // ---------------------------------------------------------------------------
  const languageFeatures = new WikiLanguageFeatures(binaryManager, namespaceCache);
  context.subscriptions.push(...languageFeatures.register());

  // ---------------------------------------------------------------------------
  // Skip set: URIs the user explicitly chose to open as text.
  // Entries are NOT consumed by the observer; they persist for the lifetime of
  // the text tab. A tab-close listener removes the entry when the text tab closes.
  // Only .md URIs are accepted to prevent unbounded growth (non-.md URIs cannot
  // be cleaned up by the tab-close listener which filters on .md-ending paths).
  // ---------------------------------------------------------------------------
  const openAsTextOnce = new Set<string>();

  const markOpenAsText = (uri: vscode.Uri): void => {
    if (uri.fsPath.endsWith('.md')) {
      openAsTextOnce.add(uri.toString());
    }
  };

  const provider = new WikiEditorProvider(context.extensionUri, binaryManager, context, namespaceCache, markOpenAsText);

  // Remove skip-set entries when their corresponding text tab closes.
  context.subscriptions.push(
    vscode.window.tabGroups.onDidChangeTabs((event) => {
      for (const tab of event.closed) {
        if (tab.input instanceof vscode.TabInputText && tab.input.uri.fsPath.endsWith('.md')) {
          openAsTextOnce.delete(tab.input.uri.toString());
        }
      }
    })
  );

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
        markOpenAsText(uri);
        const showOptions: vscode.TextDocumentShowOptions =
          typeof options === 'number' ? { viewColumn: options, preview: false } : (options ?? { preview: false });
        return vscode.window.showTextDocument(uri, showOptions);
      }
    ),

    vscode.window.onDidChangeVisibleTextEditors(async (editors) => {
      for (const editor of editors) {
        const uri = editor.document.uri;
        if (uri.scheme !== 'file') continue;
        if (!uri.fsPath.endsWith('.md')) continue;

        const uriKey = uri.toString();

        // If the user explicitly chose text, leave alone (entry persists until tab closes).
        if (openAsTextOnce.has(uriKey)) continue;

        if (!vscode.workspace.getConfiguration('wiki').get<boolean>('openFilesInViewer', true)) continue;
        if (!provider.isWikiFile(uri)) continue;

        // Collect ALL text tabs for this URI across all tab groups (F6).
        const matchingTabs: vscode.Tab[] = [];
        for (const group of vscode.window.tabGroups.all) {
          for (const tab of group.tabs) {
            if (tab.input instanceof vscode.TabInputText && tab.input.uri.toString() === uriKey) {
              matchingTabs.push(tab);
            }
          }
        }
        if (matchingTabs.length === 0) continue;

        // Swap each matching text tab to a webview (F6: handle all split groups).
        for (const foundTab of matchingTabs) {
          const wasPinned = foundTab.isPinned;

          // Open as webview before closing the text tab to preserve the tab group.
          await vscode.commands.executeCommand('vscode.openWith', uri, 'wiki.viewer', {
            viewColumn: foundTab.group.viewColumn,
            preview: foundTab.isPreview
          });
          await vscode.window.tabGroups.close(foundTab);

          // Re-pin the new webview tab if the text tab was pinned (F3).
          if (wasPinned) {
            await vscode.commands.executeCommand('workbench.action.pinEditor');
          }
        }
      }
    }),

    vscode.workspace.onDidChangeConfiguration((e) => {
      if (!e.affectsConfiguration('wiki.openFilesInViewer')) return;
      // Re-run observer logic over currently visible text editors so that
      // newly-enabled viewer state swaps existing wiki text tabs immediately (F4).
      void (async () => {
        for (const editor of vscode.window.visibleTextEditors) {
          const uri = editor.document.uri;
          if (uri.scheme !== 'file') continue;
          if (!uri.fsPath.endsWith('.md')) continue;

          const uriKey = uri.toString();
          if (openAsTextOnce.has(uriKey)) continue;
          if (!vscode.workspace.getConfiguration('wiki').get<boolean>('openFilesInViewer', true)) continue;
          if (!provider.isWikiFile(uri)) continue;

          const matchingTabs: vscode.Tab[] = [];
          for (const group of vscode.window.tabGroups.all) {
            for (const tab of group.tabs) {
              if (tab.input instanceof vscode.TabInputText && tab.input.uri.toString() === uriKey) {
                matchingTabs.push(tab);
              }
            }
          }

          for (const foundTab of matchingTabs) {
            const wasPinned = foundTab.isPinned;
            await vscode.commands.executeCommand('vscode.openWith', uri, 'wiki.viewer', {
              viewColumn: foundTab.group.viewColumn,
              preview: foundTab.isPreview
            });
            await vscode.window.tabGroups.close(foundTab);
            if (wasPinned) {
              await vscode.commands.executeCommand('workbench.action.pinEditor');
            }
          }
        }
      })();
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
