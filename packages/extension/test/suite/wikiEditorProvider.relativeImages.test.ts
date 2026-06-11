/**
 * Reproduction test: relative images in wiki articles cannot load.
 *
 * ## What this test covers
 * When a wiki article contains a relative image reference (`![diagram](./diagram.png)`),
 * the image can never load in the webview because:
 * 1. `webview.options.localResourceRoots` does not include the workspace directory
 *    (WikiEditorProvider.ts L116-L122), so VSCode's webview security model blocks
 *    serving workspace files as webview resources.
 * 2. Image srcs in the rendered HTML are not rewritten via `asWebviewUri`.
 *
 * ## How this test reproduces the bug
 * 1. A WikiEditorProvider is created with a real extension context.
 * 2. `resolveCustomTextEditor` is called with a wiki file containing a relative image.
 * 3. The test inspects `panel.webview.options.localResourceRoots` and the
 *    rendered HTML posted to the webview.
 *
 * Against the unfixed code, both assertions FAIL — proving the bug exists.
 *
 * @summary Reproduction: relative images in wiki articles cannot load.
 * @module test/suite/wikiEditorProvider.relativeImages.test
 */

import * as assert from 'node:assert';
import * as vscode from 'vscode';
import { WikiEditorProvider } from '../../src/providers/WikiEditorProvider.js';
import type { WikiBinaryManager } from '../../src/utils/wikiInstaller.js';
import type { HostMessage } from '../../src/webviews/wiki/types.js';

/**
 * Minimal real ExtensionContext: only the members the provider touches are
 * populated (extensionUri + a Map-backed workspaceState).
 *
 * @param extensionUri - The activated extension's root URI.
 * @returns A context exposing only `extensionUri` and `workspaceState`.
 */
function makeContext(extensionUri: vscode.Uri): vscode.ExtensionContext {
  const store = new Map<string, unknown>();
  const workspaceState: vscode.Memento = {
    keys: () => [...store.keys()],
    get: <T>(key: string, defaultValue?: T): T | undefined => (store.has(key) ? (store.get(key) as T) : defaultValue),
    update: (key: string, value: unknown) => {
      store.set(key, value);
      return Promise.resolve();
    }
  };
  return { extensionUri, workspaceState } as unknown as vscode.ExtensionContext;
}

async function waitFor(predicate: () => boolean, message: string, timeoutMs = 10000): Promise<void> {
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    if (predicate()) return;
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  throw new assert.AssertionError({ message });
}

describe('WikiEditorProvider — relative images cannot load', () => {
  afterEach(async () => {
    await vscode.commands.executeCommand('workbench.action.closeAllEditors');
  });

  it('localResourceRoots must include the workspace directory and image srcs must be rewritten via asWebviewUri', async function () {
    this.timeout(45000);

    const workspaceFolder = vscode.workspace.workspaceFolders?.[0];
    assert.ok(workspaceFolder, 'Expected test workspace folder');

    const ext = vscode.extensions.getExtension('goodfoot.wiki-extension');
    assert.ok(ext, 'Expected wiki extension to be discoverable');
    if (!ext.isActive) await ext.activate();

    // ------------------------------------------------------------------
    // Arrange: create a wiki fixture file with a relative image reference.
    // ------------------------------------------------------------------
    const wikiDir = vscode.Uri.joinPath(workspaceFolder.uri, 'wiki');
    const wikiFile = vscode.Uri.joinPath(wikiDir, 'relative-image.md');
    const fixture =
      `---\ntitle: Relative Image Test\nsummary: Page with relative image.\n---\n\n` +
      `# Relative Image\n\n` +
      `![diagram](./diagram.png)\n`;

    await vscode.workspace.fs.createDirectory(wikiDir);
    await vscode.workspace.fs.writeFile(wikiFile, Buffer.from(fixture));

    // ------------------------------------------------------------------
    // Arrange: a working binary manager so rendering is not blocked.
    // ------------------------------------------------------------------
    const workingManager = {
      ready: async () => {},
      formatFailure: (_error: unknown) => `Simulated: ${_error}`
    } as unknown as WikiBinaryManager;

    // ------------------------------------------------------------------
    // Arrange: provider, document, webview panel, postMessage spy.
    // ------------------------------------------------------------------
    const context = makeContext(ext.extensionUri);
    const provider = new WikiEditorProvider(ext.extensionUri, workingManager, context);

    const document = await vscode.workspace.openTextDocument(wikiFile);
    const panel = vscode.window.createWebviewPanel('wiki.viewer.test', 'initial', vscode.ViewColumn.One, {
      enableScripts: true
    });

    // Spy on webview postMessage to capture rendered HTML.
    const postedMessages: HostMessage[] = [];
    let postMessageSpyError: Error | null = null;
    try {
      const origPostMessage = panel.webview.postMessage.bind(panel.webview);
      panel.webview.postMessage = ((message: HostMessage) => {
        postedMessages.push(message);
        return origPostMessage(message);
      }) as typeof panel.webview.postMessage;
    } catch (err) {
      postMessageSpyError = err instanceof Error ? err : new Error(String(err));
    }

    try {
      // ------------------------------------------------------------------
      // Act: resolve the custom editor.
      // ------------------------------------------------------------------
      const tokenSource = new vscode.CancellationTokenSource();
      await provider.resolveCustomTextEditor(document, panel, tokenSource.token);

      // ------------------------------------------------------------------
      // Wait for the webview to send messages (ready -> updateContent).
      // ------------------------------------------------------------------
      await waitFor(
        () => postedMessages.some((m) => m.type === 'updateContent'),
        'Precondition failed: updateContent was never posted.',
        15000
      );

      // ------------------------------------------------------------------
      // PRIMARY ASSERTION: localResourceRoots must include the workspace
      // root (or at minimum the document's directory).
      //
      // Against the UNFIXED code:
      //   localResourceRoots at WikiEditorProvider.ts L116-L122 only
      //   contains extension 'dist' and 'media' directories, not the
      //   workspace folder. Even if image srcs were rewritten to
      //   vscode-webview-resource:// URIs, VSCode would refuse to serve
      //   them because the workspace directory is not in the approved
      //   local resource roots.
      //
      // After the fix:
      //   The workspace root or document directory is added to
      //   localResourceRoots, and this assertion PASSES.
      // ------------------------------------------------------------------
      const workspaceRootFsPath = workspaceFolder.uri.fsPath;
      const localResourceRoots = panel.webview.options.localResourceRoots ?? [];
      const hasWorkspaceRoot = localResourceRoots.some((uri) => uri.fsPath === workspaceRootFsPath);

      assert.ok(
        hasWorkspaceRoot,
        `BUG REPRODUCED: localResourceRoots does not include the workspace directory.\n` +
          `Workspace root: "${workspaceRootFsPath}"\n` +
          `localResourceRoots:\n` +
          localResourceRoots.map((u) => `  - ${u.fsPath}`).join('\n') +
          '\n' +
          `At WikiEditorProvider.ts L116-L122, localResourceRoots only contains ` +
          `extension 'dist' and 'media' directories, not the workspace folder. ` +
          `Without the workspace (or document directory) in localResourceRoots, ` +
          `VSCode's webview security model blocks serving workspace files ` +
          `(images, etc.) as webview resources.`
      );

      // ------------------------------------------------------------------
      // SECONDARY ASSERTION: The rendered HTML must use asWebviewUri to
      // rewrite image srcs to vscode-webview-resource:// URIs.
      //
      // Against the UNFIXED code:
      //   The markdown-it renderer produces `<img src="./diagram.png">`
      //   with no URI rewrite. The image src remains a relative path that
      //   cannot resolve in the webview context.
      //
      // After the fix:
      //   Image srcs in the rendered HTML are rewritten via asWebviewUri
      //   to use vscode-webview-resource:// URIs pointing to workspace
      //   files.
      // ------------------------------------------------------------------
      if (postMessageSpyError == null) {
        const updateContentMsg = postedMessages.find(
          (m): m is HostMessage & { type: 'updateContent' } => m.type === 'updateContent'
        );
        assert.ok(updateContentMsg, 'updateContent message was found');

        const html = updateContentMsg.html;
        assert.ok(
          html.includes('<img'),
          `Expected rendered HTML to contain an <img> tag for the relative image.\nRendered HTML:\n${html}`
        );

        // Check that the image src uses vscode-webview-resource:// URIs
        // (i.e., it was rewritten via asWebviewUri). Against unfixed code,
        // the src will be a raw relative path like "./diagram.png" — no
        // vscode-webview-resource:// prefix anywhere in the HTML.
        const hasRewrittenSrc = /vscode-webview-resource:\/\//.test(html);
        assert.strictEqual(
          hasRewrittenSrc,
          true,
          `BUG REPRODUCED: Image src was not rewritten via asWebviewUri.\n` +
            `Expected rendered HTML to contain a vscode-webview-resource:// URI ` +
            `for the image, but the HTML still contains a raw relative path.\n` +
            `At WikiEditorProvider.ts, asWebviewUri is only called for extension ` +
            `resources (script, CSS, line numbers 363-367), never for workspace ` +
            `file references embedded in the rendered markdown.\n` +
            `Rendered HTML:\n${html}`
        );
      }
    } finally {
      panel.dispose();
    }
  });
});
