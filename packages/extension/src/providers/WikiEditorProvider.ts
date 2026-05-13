/**
 * CustomTextEditorProvider that renders wiki markdown in a webview panel.
 *
 * Resolves webview link clicks to filesystem paths (relative to the source
 * document), reloads on file change, and persists scroll position.
 *
 * @summary CustomTextEditorProvider that renders wiki markdown in a webview panel.
 */

import { readFile } from 'node:fs/promises';
import * as path from 'node:path';
import * as vscode from 'vscode';
import { render } from '../rendering/MarkdownRenderer.js';
import { hasWikiFrontmatter, readFrontmatter } from '../utils/frontmatter.js';
import { getSourceArgs } from '../utils/sourceMode.js';
import { runWikiCommand } from '../utils/wikiBinary.js';
import type { WikiBinaryManager } from '../utils/wikiInstaller.js';
import type { HostMessage, ResolvedRefEntry, WebviewMessage } from '../webviews/wiki/types.js';

interface WikiSummaryJson {
  title: string;
  file: string;
  summary?: string;
}

/**
 * Registers as a custom text editor for `.md` files. Each open document gets
 * its own webview panel with isolated state.
 */
export class WikiEditorProvider implements vscode.CustomTextEditorProvider {
  constructor(
    private readonly _extensionUri: vscode.Uri,
    private readonly _binaryManager: WikiBinaryManager,
    private readonly _context: vscode.ExtensionContext
  ) {}

  private _workspaceRoot(): string | undefined {
    return vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  }

  /**
   * Return true if `uri` should be opened in the wiki viewer.
   *
   * A markdown file qualifies only when it lives under the active workspace
   * and carries both a non-empty `title` and `summary` in its YAML
   * frontmatter. Plain markdown files without wiki frontmatter fall through
   * to VS Code's default markdown editor.
   *
   * @param uri - The file URI to test.
   * @returns True when the file is a wiki-aware markdown file.
   */
  async isWikiFile(uri: vscode.Uri): Promise<boolean> {
    if (!this._isInWorkspaceMarkdown(uri)) return false;
    const info = await readFrontmatter(uri.fsPath);
    return hasWikiFrontmatter(info);
  }

  private _isInWorkspaceMarkdown(uri: vscode.Uri): boolean {
    if (!uri.fsPath.endsWith('.md')) return false;
    const wsRoot = this._workspaceRoot();
    if (wsRoot == null) return false;
    return uri.fsPath.startsWith(wsRoot + path.sep) || uri.fsPath === wsRoot;
  }

  // --------------------------------------------------------------------------
  // CustomTextEditorProvider
  // --------------------------------------------------------------------------

  async resolveCustomTextEditor(
    document: vscode.TextDocument,
    webviewPanel: vscode.WebviewPanel,
    _token: vscode.CancellationToken
  ): Promise<void> {
    if (!(await this.isWikiFile(document.uri))) {
      webviewPanel.dispose();
      await vscode.window.showTextDocument(document.uri, { preview: false });
      return;
    }

    webviewPanel.iconPath = new vscode.ThemeIcon('library');

    webviewPanel.webview.options = {
      enableScripts: true,
      localResourceRoots: [
        vscode.Uri.joinPath(this._extensionUri, 'dist'),
        vscode.Uri.joinPath(this._extensionUri, 'media')
      ]
    };

    webviewPanel.webview.html = this._buildShellHtml(webviewPanel.webview);

    const scrollKey = `scroll:${document.uri.toString()}`;

    const onDocumentChange = async (changedDocument: vscode.TextDocument) => {
      if (changedDocument.uri.toString() !== document.uri.toString()) return;
      await this._renderPage(webviewPanel.webview, document.uri, webviewPanel);
    };

    const changeDisposable = vscode.workspace.onDidChangeTextDocument((event) => {
      void onDocumentChange(event.document);
    });

    const messageDisposable = webviewPanel.webview.onDidReceiveMessage(async (message: WebviewMessage) => {
      switch (message.type) {
        case 'ready': {
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
          // The href is a plain markdown link target (e.g. "../foo.md",
          // "wiki/index.md", "wiki/index.md#section"). Resolve it relative
          // to the linking document's directory — standard markdown semantics.
          await this._navigate(webviewPanel.webview, document.uri, message.href, message.split);
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
            if (await this.isWikiFile(fileUri)) {
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

    webviewPanel.onDidDispose(() => {
      changeDisposable.dispose();
      messageDisposable.dispose();
    });
  }

  // --------------------------------------------------------------------------
  // Private helpers
  // --------------------------------------------------------------------------

  /**
   * Resolve `href` relative to the linking document, then open it as a wiki
   * page (if a markdown file) or in a plain editor (anything else).
   *
   * @param webview     - Target webview, used to post error messages.
   * @param documentUri - URI of the document the navigation originated from.
   * @param href        - Raw markdown link target.
   * @param split       - When true, open beside the current editor.
   */
  private async _navigate(
    webview: vscode.Webview,
    documentUri: vscode.Uri,
    href: string,
    split: boolean
  ): Promise<void> {
    // Strip URL fragment for path resolution; markdown anchors are not
    // resolved server-side.
    const hashIdx = href.indexOf('#');
    const rawPath = hashIdx >= 0 ? href.slice(0, hashIdx) : href;

    if (rawPath === '') {
      // pure fragment — ignore
      return;
    }

    const sourceDir = path.dirname(documentUri.fsPath);
    const absPath = path.isAbsolute(rawPath) ? rawPath : path.resolve(sourceDir, rawPath);
    const targetUri = vscode.Uri.file(absPath);

    try {
      const stat = await vscode.workspace.fs.stat(targetUri);
      if (stat.type === vscode.FileType.Directory) {
        await vscode.commands.executeCommand('revealInExplorer', targetUri);
        return;
      }
    } catch {
      this._postMessage(webview, {
        type: 'showError',
        message: `Could not find file: "${href}"`
      });
      return;
    }

    const viewColumn = split ? vscode.ViewColumn.Beside : vscode.ViewColumn.Active;
    if (absPath.endsWith('.md') && (await this.isWikiFile(targetUri))) {
      await vscode.commands.executeCommand('vscode.openWith', targetUri, 'wiki.viewer', viewColumn);
    } else {
      await vscode.window.showTextDocument(targetUri, { viewColumn, preview: false });
    }
  }

  private async _renderPage(
    webview: vscode.Webview,
    uri: vscode.Uri,
    panel: vscode.WebviewPanel,
    scrollY?: number
  ): Promise<void> {
    this._postMessage(webview, { type: 'showLoading' });

    let text: string;
    let summaryResult: Awaited<ReturnType<typeof runWikiCommand>> | null;
    let refsResult: Awaited<ReturnType<typeof runWikiCommand>> | null;

    try {
      const handle = await this._binaryManager.ready();
      const sourceArgs = getSourceArgs();
      [text, summaryResult, refsResult] = await Promise.all([
        this._readDocumentText(uri),
        runWikiCommand(
          handle.path,
          [...sourceArgs, 'summary', uri.fsPath, '--format', 'json'],
          undefined,
          this._workspaceRoot()
        ).catch(() => null),
        runWikiCommand(
          handle.path,
          [...sourceArgs, 'refs', uri.fsPath, '--format', 'json'],
          undefined,
          this._workspaceRoot()
        ).catch(() => null)
      ]);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      console.error('[wiki-extension] Failed to read wiki file:', err);
      this._postMessage(webview, { type: 'showError', message: `Failed to load wiki page: ${message}` });
      return;
    }

    const html = render(text);

    if (summaryResult != null && summaryResult.exitCode === 0 && summaryResult.stdout.trim() !== '') {
      try {
        const summary = JSON.parse(summaryResult.stdout) as WikiSummaryJson;
        panel.title = summary.title;
      } catch (parseErr) {
        console.warn('[wiki-extension] Failed to parse wiki summary JSON:', parseErr);
      }
    }

    let refs: ResolvedRefEntry[] | undefined;
    if (refsResult != null && refsResult.exitCode === 0 && refsResult.stdout.trim() !== '') {
      try {
        refs = JSON.parse(refsResult.stdout) as ResolvedRefEntry[];
      } catch (parseErr) {
        console.warn('[wiki-extension] Failed to parse wiki refs JSON:', parseErr);
      }
    }

    const updateMessage: HostMessage = { type: 'updateContent', html, scrollY, refs };
    this._postMessage(webview, updateMessage);
  }

  private async _readDocumentText(uri: vscode.Uri): Promise<string> {
    const openDocument = vscode.workspace.textDocuments.find((document) => document.uri.toString() === uri.toString());
    if (openDocument != null) {
      return openDocument.getText();
    }
    return readFile(uri.fsPath, 'utf8');
  }

  private _postMessage(webview: vscode.Webview, message: HostMessage): void {
    webview.postMessage(message).then(
      () => {},
      (err: unknown) => {
        console.error('[wiki-extension] Failed to post message to webview:', err);
      }
    );
  }

  private _buildShellHtml(webview: vscode.Webview): string {
    const scriptUri = webview.asWebviewUri(vscode.Uri.joinPath(this._extensionUri, 'dist', 'wiki.js'));
    const codiconUri = webview.asWebviewUri(vscode.Uri.joinPath(this._extensionUri, 'dist', 'codicons', 'codicon.css'));
    const markdownCssUri = webview.asWebviewUri(vscode.Uri.joinPath(this._extensionUri, 'media', 'markdown.css'));
    const highlightCssUri = webview.asWebviewUri(vscode.Uri.joinPath(this._extensionUri, 'media', 'highlight.css'));
    const tooltipCssUri = webview.asWebviewUri(vscode.Uri.joinPath(this._extensionUri, 'media', 'tooltip.css'));

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
