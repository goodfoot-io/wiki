/**
 * CustomTextEditorProvider that renders wiki markdown in a webview panel.
 *
 * Supports wikilink navigation via vscode.open, live reload on file changes,
 * and scroll position persistence via workspaceState memento.
 *
 * @summary CustomTextEditorProvider that renders wiki markdown in a webview panel.
 */

import { readFile } from 'node:fs/promises';
import * as path from 'node:path';
import * as vscode from 'vscode';
import { render } from '../rendering/MarkdownRenderer.js';
import { runWikiCommand } from '../utils/wikiBinary.js';
import type { WikiBinaryManager } from '../utils/wikiInstaller.js';
import type { HostMessage, RefEntry, WebviewMessage } from '../webviews/wiki/types.js';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/** Summary JSON returned by `wiki summary <path> --format json`. */
interface WikiSummaryJson {
  title: string;
  file: string;
  summary?: string;
  aliases?: string[];
  tags?: string[];
}

// ---------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------

/**
 * Registers as a custom text editor for *.md and *.wiki.md files.
 * Each open document gets its own webview panel with isolated state.
 */
export class WikiEditorProvider implements vscode.CustomTextEditorProvider {
  constructor(
    private readonly _extensionUri: vscode.Uri,
    private readonly _binaryManager: WikiBinaryManager,
    private readonly _context: vscode.ExtensionContext
  ) {}

  /**
   * Return the filesystem path of the first VS Code workspace folder, or undefined
   * if no folder is open. The wiki CLI requires a cwd inside the git repo.
   *
   * @returns The workspace root path, or undefined if no folder is open.
   */
  private _workspaceRoot(): string | undefined {
    return vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  }

  /**
   * Resolve the wiki directory for the current workspace.
   *
   * Resolution order:
   *  1. `$WIKI_DIR` is an absolute path → use it directly.
   *  2. `$WIKI_DIR` is a relative path → resolve relative to `workspaceRoot`.
   *  3. `$WIKI_DIR` is unset → default to `<workspaceRoot>/wiki/`.
   *
   * Returns null when `$WIKI_DIR` is relative (or unset) and no workspace
   * folder is open.
   *
   * @returns The resolved wiki directory path with a trailing separator, or null.
   */
  private _wikiDir(): string | null {
    const envWikiDir = process.env['WIKI_DIR'];
    if (envWikiDir != null && envWikiDir.length > 0) {
      if (path.isAbsolute(envWikiDir)) {
        return envWikiDir.endsWith(path.sep) ? envWikiDir : envWikiDir + path.sep;
      }
      const workspaceRoot = this._workspaceRoot();
      if (workspaceRoot == null) return null;
      const resolved = path.join(workspaceRoot, envWikiDir);
      return resolved.endsWith(path.sep) ? resolved : resolved + path.sep;
    }
    const workspaceRoot = this._workspaceRoot();
    if (workspaceRoot == null) return null;
    return path.join(workspaceRoot, 'wiki') + path.sep;
  }

  /**
   * Return true if `uri` should be opened in the wiki viewer.
   *
   * Two cases qualify:
   *  1. The file has a `.wiki.md` extension — matches anywhere in the workspace.
   *  2. The file has a plain `.md` extension and lives inside `$WIKI_DIR`.
   *
   * @param uri - The file URI to test.
   * @returns True if the file belongs to the wiki, false otherwise.
   */
  isWikiFile(uri: vscode.Uri): boolean {
    if (uri.fsPath.endsWith('.wiki.md')) return true;
    if (!uri.fsPath.endsWith('.md')) return false;
    const wikiDir = this._wikiDir();
    if (wikiDir == null) return false;
    return uri.fsPath.startsWith(wikiDir);
  }

  // --------------------------------------------------------------------------
  // CustomTextEditorProvider
  // --------------------------------------------------------------------------

  async resolveCustomTextEditor(
    document: vscode.TextDocument,
    webviewPanel: vscode.WebviewPanel,
    _token: vscode.CancellationToken
  ): Promise<void> {
    // If the user has disabled the wiki viewer, open the file as a text document instead.
    const openInViewer = vscode.workspace.getConfiguration('wiki').get<boolean>('openFilesInViewer', true);
    if (!openInViewer) {
      webviewPanel.dispose();
      await vscode.window.showTextDocument(document.uri, { preview: false });
      return;
    }

    // Only render files that actually belong to the wiki:
    //   • any *.wiki.md file (anywhere in the workspace), or
    //   • *.md files inside <workspaceRoot>/wiki/ — i.e. $WIKI_DIR, whose
    //     default is the "wiki" subdirectory of the git root (parent of .git).
    // Files that match the manifest selector but fall outside $WIKI_DIR
    // (e.g. /home/node/wiki/README.md when the git root IS ~/wiki/) are
    // redirected to the text editor so they open normally.
    if (!this.isWikiFile(document.uri)) {
      webviewPanel.dispose();
      await vscode.window.showTextDocument(document.uri, { preview: false });
      return;
    }

    // Set the tab icon to the library codicon.
    webviewPanel.iconPath = new vscode.ThemeIcon('library');

    // Configure webview security and resource roots.
    webviewPanel.webview.options = {
      enableScripts: true,
      localResourceRoots: [
        vscode.Uri.joinPath(this._extensionUri, 'dist'),
        vscode.Uri.joinPath(this._extensionUri, 'media')
      ]
    };

    // Set the initial HTML shell (loads dist/wiki.js).
    webviewPanel.webview.html = this._buildShellHtml(webviewPanel.webview);

    const scrollKey = `scroll:${document.uri.toString()}`;

    const onDocumentChange = async (changedDocument: vscode.TextDocument) => {
      if (changedDocument.uri.toString() !== document.uri.toString()) return;
      await this._renderPage(webviewPanel.webview, document.uri, webviewPanel);
    };

    const changeDisposable = vscode.workspace.onDidChangeTextDocument((event) => {
      void onDocumentChange(event.document);
    });

    // Handle messages from the webview.
    const messageDisposable = webviewPanel.webview.onDidReceiveMessage(async (message: WebviewMessage) => {
      switch (message.type) {
        case 'ready': {
          // Render the initial page only once the webview signals readiness.
          try {
            await this._binaryManager.ready();
            const savedScrollY = this._context.workspaceState.get<number>(scrollKey);
            await this._renderPage(webviewPanel.webview, document.uri, webviewPanel, savedScrollY);
          } catch (error) {
            this._postMessage(webviewPanel.webview, {
              type: 'showError',
              message: `Failed to install wiki CLI for this extension: ${this._binaryManager.formatFailure(error)}`
            });
          }
          break;
        }

        case 'navigate': {
          // Resolve the target page URI via wiki summary.
          const targetUri = await this._resolvePageUri(message.pageName);
          if (targetUri == null) {
            // Fallback: treat as a workspace-relative file path when the name
            // looks like a path (e.g. [[packages/foo/bar.ts]] or
            // [[public/plugins/runtime/skills/card/SKILL.md]]).
            const ext = path.extname(message.pageName).toLowerCase();
            const isFilePath = ext !== '' && message.pageName.includes('/');
            if (isFilePath) {
              const workspaceRoot = this._workspaceRoot();
              if (workspaceRoot != null) {
                const fileUri = vscode.Uri.file(path.join(workspaceRoot, message.pageName));
                try {
                  await vscode.workspace.fs.stat(fileUri);
                  const viewColumn = message.split ? vscode.ViewColumn.Beside : vscode.ViewColumn.Active;
                  if (ext === '.md') {
                    await vscode.commands.executeCommand('vscode.open', fileUri, viewColumn);
                  } else {
                    await vscode.window.showTextDocument(fileUri, { viewColumn, preview: false });
                  }
                  return;
                } catch (err) {
                  const isNotFound = err instanceof vscode.FileSystemError && err.code === 'FileNotFound';
                  if (!isNotFound) {
                    console.warn('[wiki-extension] Unexpected error checking workspace path:', fileUri.fsPath, err);
                  }
                }
              }
            }
            this._postMessage(webviewPanel.webview, {
              type: 'showError',
              message: `Could not find wiki page: "${message.pageName}"`
            });
            return;
          }

          if (message.split) {
            await vscode.commands.executeCommand('vscode.openWith', targetUri, 'wiki.viewer', {
              viewColumn: vscode.ViewColumn.Beside
            });
          } else {
            await vscode.commands.executeCommand('vscode.open', targetUri, vscode.ViewColumn.Active);
          }
          break;
        }

        case 'openInEditor': {
          const viewColumn = message.split ? vscode.ViewColumn.Beside : vscode.ViewColumn.Active;
          await vscode.commands.executeCommand('wiki.openInEditor', document.uri, { viewColumn, preview: false });
          break;
        }

        case 'openFile': {
          try {
            const fileUri = vscode.Uri.parse(message.uri);
            if (this.isWikiFile(fileUri)) {
              const viewColumn = message.split ? vscode.ViewColumn.Beside : vscode.ViewColumn.Active;
              await vscode.commands.executeCommand('wiki.openInEditor', fileUri, { viewColumn, preview: false });
            } else {
              const viewColumn = message.split ? vscode.ViewColumn.Beside : vscode.ViewColumn.Active;
              await vscode.window.showTextDocument(fileUri, { viewColumn, preview: false });
            }
          } catch (err) {
            console.error('[wiki-extension] Failed to open file URI:', message.uri, err);
          }
          break;
        }

        case 'openExternal': {
          void vscode.env.openExternal(vscode.Uri.parse(message.uri));
          break;
        }

        case 'openSearch': {
          await vscode.commands.executeCommand('wiki.search');
          break;
        }

        case 'scrollPosition': {
          void this._context.workspaceState.update(scrollKey, message.y);
          break;
        }

        default: {
          const _exhaustive: never = message;
          console.warn('[wiki-extension] Unhandled webview message:', _exhaustive);
        }
      }
    });

    // Clean up on panel close.
    webviewPanel.onDidDispose(() => {
      changeDisposable.dispose();
      messageDisposable.dispose();
    });
  }

  // --------------------------------------------------------------------------
  // Private helpers
  // --------------------------------------------------------------------------

  /**
   * Render the given wiki file URI into the webview.
   * Updates the panel title from the wiki summary.
   *
   * @param webview - Target webview to post messages to.
   * @param uri - File URI of the wiki page to render.
   * @param panel - Parent webview panel (used to update the title).
   * @param scrollY - Optional scroll position to restore after render.
   */
  private async _renderPage(
    webview: vscode.Webview,
    uri: vscode.Uri,
    panel: vscode.WebviewPanel,
    scrollY?: number
  ): Promise<void> {
    this._postMessage(webview, { type: 'showLoading' });

    let text: string;
    let summaryResult: Awaited<ReturnType<typeof runWikiCommand>>;
    let refsResult: Awaited<ReturnType<typeof runWikiCommand>> | null;

    try {
      const handle = await this._binaryManager.ready();
      // Read file content, run summary, and pre-fetch tooltip refs concurrently.
      // refs is best-effort: its failure is caught inline so it never rejects the Promise.all.
      [text, summaryResult, refsResult] = await Promise.all([
        this._readDocumentText(uri),
        runWikiCommand(handle.path, ['summary', uri.fsPath, '--format', 'json'], undefined, this._workspaceRoot()),
        runWikiCommand(handle.path, ['refs', uri.fsPath, '--format', 'json'], undefined, this._workspaceRoot()).catch(
          () => null
        )
      ]);
    } catch (err) {
      // File read error or spawn error (e.g. ENOENT — binary not found after initial check).
      const message = err instanceof Error ? err.message : String(err);
      console.error('[wiki-extension] Failed to read wiki file or run summary command:', err);
      this._postMessage(webview, { type: 'showError', message: `Failed to load wiki page: ${message}` });
      return;
    }

    const html = render(text);

    // Update panel title from summary (best-effort; don't fail render if summary fails).
    if (summaryResult.exitCode === 0 && summaryResult.stdout.trim() !== '') {
      try {
        const summary = JSON.parse(summaryResult.stdout) as WikiSummaryJson;
        panel.title = summary.title;
      } catch (parseErr) {
        console.warn('[wiki-extension] Failed to parse wiki summary JSON:', parseErr);
      }
    }

    let refs: RefEntry[] | undefined;
    if (refsResult != null && refsResult.exitCode === 0 && refsResult.stdout.trim() !== '') {
      try {
        refs = JSON.parse(refsResult.stdout) as RefEntry[];
      } catch (parseErr) {
        console.warn('[wiki-extension] Failed to parse wiki refs JSON:', parseErr);
      }
    }

    const updateMessage: HostMessage = { type: 'updateContent', html, scrollY, refs };
    this._postMessage(webview, updateMessage);
  }

  /**
   * Resolve a wiki page name to a VS Code URI by running `wiki summary`.
   * Returns null if the page cannot be found.
   *
   * @param pageName - Decoded wiki page title to resolve.
   * @returns The VS Code URI for the page's file, or null if not found.
   */
  private async _resolvePageUri(pageName: string): Promise<vscode.Uri | null> {
    const handle = await this._binaryManager.ready();
    const result = await runWikiCommand(
      handle.path,
      ['summary', pageName, '--format', 'json'],
      undefined,
      this._workspaceRoot()
    );
    if (result.exitCode !== 0 || result.stdout.trim() === '') {
      console.warn(`[wiki-extension] Could not resolve page "${pageName}":`, result.stderr.trim());
      return null;
    }
    try {
      const summary = JSON.parse(result.stdout) as WikiSummaryJson;
      return vscode.Uri.file(summary.file);
    } catch (parseErr) {
      console.error('[wiki-extension] Failed to parse wiki summary for page:', pageName, parseErr);
      return null;
    }
  }

  /**
   * Read the current text for a URI, preferring an already-open TextDocument so
   * the webview stays in sync with unsaved in-memory edits.
   *
   * @param uri - File URI of the wiki page.
   * @returns The current text content.
   */
  private async _readDocumentText(uri: vscode.Uri): Promise<string> {
    const openDocument = vscode.workspace.textDocuments.find((document) => document.uri.toString() === uri.toString());
    if (openDocument != null) {
      return openDocument.getText();
    }
    return readFile(uri.fsPath, 'utf8');
  }

  /**
   * Post a typed host message to the webview.
   *
   * @param webview - Target webview.
   * @param message - Typed host message to send.
   */
  private _postMessage(webview: vscode.Webview, message: HostMessage): void {
    webview.postMessage(message).then(
      () => {},
      (err: unknown) => {
        console.error('[wiki-extension] Failed to post message to webview:', err);
      }
    );
  }

  /**
   * Build the HTML shell that loads the bundled webview script.
   *
   * @param webview - The webview instance used to generate secure resource URIs.
   * @returns The complete HTML string for the webview shell.
   */
  private _buildShellHtml(webview: vscode.Webview): string {
    const scriptUri = webview.asWebviewUri(vscode.Uri.joinPath(this._extensionUri, 'dist', 'wiki.js'));
    const codiconUri = webview.asWebviewUri(vscode.Uri.joinPath(this._extensionUri, 'dist', 'codicons', 'codicon.css'));
    const markdownCssUri = webview.asWebviewUri(vscode.Uri.joinPath(this._extensionUri, 'media', 'markdown.css'));
    const highlightCssUri = webview.asWebviewUri(vscode.Uri.joinPath(this._extensionUri, 'media', 'highlight.css'));
    const tooltipCssUri = webview.asWebviewUri(vscode.Uri.joinPath(this._extensionUri, 'media', 'tooltip.css'));

    // Content security policy: allow scripts and fonts from the extension's dist and media origins.
    const cspSource = webview.cspSource;

    return `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1.0" />
  <meta
    http-equiv="Content-Security-Policy"
    content="default-src 'none'; script-src ${cspSource} 'unsafe-inline'; style-src ${cspSource} 'unsafe-inline'; font-src ${cspSource}; img-src ${cspSource} https: data:;"
  />
  <link href="${codiconUri}" rel="stylesheet" id="vscode-codicon-stylesheet" />
  <link href="${markdownCssUri}" rel="stylesheet" />
  <link href="${highlightCssUri}" rel="stylesheet" />
  <link href="${tooltipCssUri}" rel="stylesheet" />
  <title>Wiki Viewer</title>
</head>
<body class="vscode-body">
  <div class="wiki-toolbar" id="toolbar"></div>
  <vscode-progress-ring id="loading"></vscode-progress-ring>
  <div id="error" style="display:none" role="alert"></div>
  <div id="content" class="markdown-body vscode-body"></div>
  <script type="module" src="${scriptUri}"></script>
</body>
</html>`;
  }
}
