/**
 * Reproduction test: onDidChangeTextDocument directly calls _renderPage on
 * every change event without debounce.
 *
 * ## Bug
 * WikiEditorProvider.ts registers an `onDidChangeTextDocument` handler at
 * [~L135](../../src/providers/WikiEditorProvider.ts#L135-L137) that calls
 * `_renderPage` synchronously on every single text document change event.
 * Rapid typing (e.g., 10 keystrokes in quick succession) triggers 10 full
 * render cycles including mermaid re-render via `renderDiagrams()`.
 *
 * Expected: debounced render (~250ms idle) with diagram cache.
 *
 * ## What this test covers
 * 1. Creates a wiki fixture file and resolves the custom editor.
 * 2. Spies on `webview.postMessage` to capture render-related messages.
 * 3. Makes 5 rapid programmatic edits (simulating typing).
 * 4. Asserts that fewer render messages arrive than the number of edits,
 *    proving the render is debounced.
 *
 * **Against the UNFIXED code:**
 * Each edit triggers `_renderPage` directly. With 5 edits, 10 messages
 * arrive (5 `showLoading` + 5 `updateContent`). The assertion fails —
 * proving the bug.
 *
 * **After the fix (debounce ~250ms):**
 * Rapid edits coalesce into at most 1 render cycle, producing at most
 * 2 messages (1 `showLoading` + 1 `updateContent`). The assertion passes.
 *
 * @summary Reproduction: onDidChangeTextDocument must debounce renders.
 * @module test/suite/wikiEditorProvider.renderDebounce.test
 */

import * as assert from 'node:assert';
import * as vscode from 'vscode';
import { WikiEditorProvider } from '../../src/providers/WikiEditorProvider.js';
import type { WikiBinaryManager } from '../../src/utils/wikiInstaller.js';
import type { HostMessage } from '../../src/webviews/wiki/types.js';

/**
 * Polls `predicate` every 25 ms until it returns `true` or `timeoutMs`
 * elapses.
 *
 * @param predicate - Boolean-returning check.
 * @param message   - Assertion message on timeout.
 * @param timeoutMs - Maximum wait in milliseconds.
 */
async function waitFor(predicate: () => boolean, message: string, timeoutMs = 15000): Promise<void> {
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    if (predicate()) return;
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  throw new assert.AssertionError({ message });
}

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

describe('WikiEditorProvider — debounced text change handler', () => {
  afterEach(async () => {
    await vscode.commands.executeCommand('workbench.action.closeAllEditors');
  });

  it('onDidChangeTextDocument must debounce renders instead of rendering on every keystroke', async function () {
    this.timeout(45000);

    const workspaceFolder = vscode.workspace.workspaceFolders?.[0];
    assert.ok(workspaceFolder, 'Expected test workspace folder');

    const ext = vscode.extensions.getExtension('goodfoot.wiki-extension');
    assert.ok(ext, 'Expected wiki extension to be discoverable');
    if (!ext.isActive) await ext.activate();

    // ------------------------------------------------------------------
    // Arrange: create a wiki fixture file.
    // Both `title` and `summary` are required for isWikiFile() to return
    // true so the custom editor takes over and registers the
    // onDidChangeTextDocument handler we are testing.
    // ------------------------------------------------------------------
    const wikiDir = vscode.Uri.joinPath(workspaceFolder.uri, 'wiki');
    const wikiFile = vscode.Uri.joinPath(wikiDir, 'render-debounce-test.md');
    const fixture =
      '---\ntitle: Render Debounce Test\nsummary: Test debounce behavior on text changes.\n---\n\n' +
      'Initial content\n';

    await vscode.workspace.fs.createDirectory(wikiDir);
    await vscode.workspace.fs.writeFile(wikiFile, Buffer.from(fixture));

    // ------------------------------------------------------------------
    // Arrange: a no-op binary manager so binary installation does not
    // interfere with the test.
    // ------------------------------------------------------------------
    const noopManager = {
      ready: async () => {},
      formatFailure: (_error: unknown) => String(_error)
    } as unknown as WikiBinaryManager;

    const context = makeContext(ext.extensionUri);
    const provider = new WikiEditorProvider(ext.extensionUri, noopManager, context);

    const document = await vscode.workspace.openTextDocument(wikiFile);

    // Create a real webview panel. Its onDidReceiveMessage will route the
    // frontend's 'ready' message to the handler that calls _renderPage.
    const panel = vscode.window.createWebviewPanel('wiki.viewer.test', 'Debounce Test', vscode.ViewColumn.One, {
      enableScripts: true
    });

    // ------------------------------------------------------------------
    // Arrange: spy on webview.postMessage to count render messages.
    // We track showLoading and updateContent messages to count renders.
    // ------------------------------------------------------------------
    const postedMessages: HostMessage[] = [];
    try {
      const origPostMessage = panel.webview.postMessage.bind(panel.webview);
      panel.webview.postMessage = ((message: HostMessage) => {
        postedMessages.push(message);
        return origPostMessage(message);
      }) as typeof panel.webview.postMessage;
    } catch {
      // In some VS Code test environments, webview.postMessage is read-only.
      // If the spy cannot be set up, the test cannot count renders and
      // should report the environment limitation rather than silently pass.
      this.skip();
    }

    try {
      // ------------------------------------------------------------------
      // Act: resolve the custom editor. This registers the
      // onDidChangeTextDocument handler. The webview then loads, the
      // frontend dispatches 'ready', and the handler calls _renderPage()
      // once for the initial render.
      // ------------------------------------------------------------------
      const tokenSource = new vscode.CancellationTokenSource();
      await provider.resolveCustomTextEditor(document, panel, tokenSource.token);

      // Wait for the initial render to complete. After this point, the
      // handler is live and we reset the message counter.
      await waitFor(
        () => postedMessages.some((m) => m.type === 'updateContent'),
        'Initial render did not complete within timeout. ' +
          'The webview may not have sent the "ready" message in this test environment.'
      );

      // Clear the count — we only want to count renders triggered by our
      // programmatic edits, not the initial render from 'ready'.
      postedMessages.length = 0;

      // ------------------------------------------------------------------
      // Act: make 5 rapid programmatic edits simulating typing.
      // Each edit triggers onDidChangeTextDocument → onDocumentChange →
      // _renderPage. With no debounce, each edit produces its own render.
      // ------------------------------------------------------------------
      const editCount = 5;
      for (let i = 0; i < editCount; i++) {
        const edit = new vscode.WorkspaceEdit();
        const position = new vscode.Position(document.lineCount, 0);
        edit.insert(document.uri, position, `Edit ${i}\n`);
        const applied = await vscode.workspace.applyEdit(edit);
        assert.ok(applied, `Edit ${i} was not applied — precondition failed.`);
      }

      // ------------------------------------------------------------------
      // Wait well below the expected 250ms debounce threshold.
      // If a debounce exists, no render fires during this window.
      // ------------------------------------------------------------------
      await new Promise((resolve) => setTimeout(resolve, 100));

      // ------------------------------------------------------------------
      // Assert: count the render-related messages.
      // ------------------------------------------------------------------
      const renderMessages = postedMessages.filter((m) => m.type === 'showLoading' || m.type === 'updateContent');

      const showLoadingCount = renderMessages.filter((m) => m.type === 'showLoading').length;
      const updateContentCount = renderMessages.filter((m) => m.type === 'updateContent').length;

      // With NO debounce: 5 edits → 5 renders → 10 messages.
      // With debounce (~250ms): 0 renders → 0-2 messages in the 100ms window.
      //
      // Allow up to 2 messages (1 showLoading + 1 updateContent) for the
      // edge case where the debounce fires just before our wait window ends.
      assert.ok(
        renderMessages.length <= 2,
        `BUG REPRODUCED: onDidChangeTextDocument calls _renderPage synchronously ` +
          `on every change event without debounce.\n\n` +
          `Made ${editCount} rapid edits in sequence. With no debounce, each edit ` +
          `triggered a full render cycle:\n` +
          `  • ${showLoadingCount} × showLoading messages\n` +
          `  • ${updateContentCount} × updateContent messages\n` +
          `  • ${renderMessages.length} total render-related messages\n\n` +
          `Expected at most 2 messages (1 showLoading + 1 updateContent) with a ` +
          `~250ms debounce. Instead, ${editCount} edits produced ${renderMessages.length} ` +
          `messages — one render per keystroke.\n\n` +
          `Root cause: WikiEditorProvider.ts ~L130-L137: onDidChangeTextDocument ` +
          `handler calls this._renderPage() synchronously on every change event.\n\n` +
          `All posted messages: ${JSON.stringify(renderMessages)}`
      );
    } finally {
      panel.dispose();
    }
  });
});
