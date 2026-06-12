/**
 * Reproduction test: `showLoading` flashes on every document-change-triggered re-render.
 *
 * ## What this test covers
 * When the document text changes (e.g., keystroke), `onDidChangeTextDocument` fires,
 * triggering `_renderPage` which unconditionally posts `showLoading` first
 * ([WikiEditorProvider.ts:322](../../src/providers/WikiEditorProvider.ts#L322)).
 * This causes the progress ring to flash on every keystroke.
 *
 * ## Hypothesis
 * **showLoading is always posted at the start of `_renderPage`, even for
 * document-change-triggered re-renders.** It should only show for explicit user
 * actions (save, manual refresh), not for document-change-triggered re-renders.
 *
 * ## How this test reproduces the bug
 * 1. A wiki fixture file is created and opened via `resolveCustomTextEditor`.
 * 2. After the initial render completes (`updateContent` received), the test
 *    records the number of `showLoading` messages posted so far.
 * 3. A workspace edit simulates a keystroke, triggering `onDidChangeTextDocument`.
 * 4. The test waits for the second `updateContent` (re-render triggered by the change).
 * 5. The test asserts that no new `showLoading` message was posted.
 *
 * **Against the unfixed code:**
 * `_renderPage` always posts `showLoading` first
 * ([WikiEditorProvider.ts:322](../../src/providers/WikiEditorProvider.ts#L322)),
 * so the count increases by 1. The assertion FAILS -- proving the bug.
 *
 * **After the fix:**
 * Document-change-triggered renders skip the `showLoading` post. The count
 * stays the same. The assertion PASSES.
 *
 * @summary Reproduction: showLoading flashes on every document-change re-render.
 * @module test/suite/wikiEditorProvider.showLoadingOnDocumentChange.test
 */

import * as assert from 'node:assert';
import * as vscode from 'vscode';
import { WikiEditorProvider } from '../../src/providers/WikiEditorProvider.js';
import type { WikiBinaryManager } from '../../src/utils/wikiInstaller.js';
import type { HostMessage } from '../../src/webviews/wiki/types.js';

/**
 * Minimal real ExtensionContext: only the members the provider touches are
 * populated (`extensionUri` + a `Map`-backed `workspaceState`).
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

/**
 * Polls `predicate` every 25 ms until it returns `true` or `timeoutMs` elapses.
 *
 * @param predicate - Boolean-returning check.
 * @param message   - Assertion message on timeout.
 * @param timeoutMs - Maximum wait in milliseconds (default 15000).
 */
async function waitFor(predicate: () => boolean, message: string, timeoutMs = 15000): Promise<void> {
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    if (predicate()) return;
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  throw new assert.AssertionError({ message });
}

describe('WikiEditorProvider -- showLoading on document change', () => {
  afterEach(async () => {
    await vscode.commands.executeCommand('workbench.action.closeAllEditors');
  });

  it('does not flash showLoading on document-change-triggered re-render', async function () {
    this.timeout(45000);

    const workspaceFolder = vscode.workspace.workspaceFolders?.[0];
    assert.ok(workspaceFolder, 'Expected test workspace folder');

    const ext = vscode.extensions.getExtension('goodfoot.wiki-extension');
    assert.ok(ext, 'Expected wiki extension to be discoverable');
    if (!ext.isActive) await ext.activate();

    // ------------------------------------------------------------------
    // Arrange: create a wiki fixture file with frontmatter.
    // Both `title` and `summary` are required for isWikiFile() to return
    // true so the custom editor takes over.
    // ------------------------------------------------------------------
    const wikiDir = vscode.Uri.joinPath(workspaceFolder.uri, 'wiki');
    const wikiFile = vscode.Uri.joinPath(wikiDir, 'showloading-on-doc-change.md');
    const frontmatterTitle = 'No ShowLoading on Change';
    const fixture =
      `---\ntitle: ${frontmatterTitle}\n` +
      `summary: A fixture page for the showLoading-on-document-change regression test.\n---\n\n# Initial\n`;

    await vscode.workspace.fs.createDirectory(wikiDir);
    await vscode.workspace.fs.writeFile(wikiFile, Buffer.from(fixture));

    // ------------------------------------------------------------------
    // Arrange: a working binary manager so rendering is not blocked.
    // ------------------------------------------------------------------
    const workingManager = {
      ready: async () => {},
      formatFailure: (_error: unknown) => String(_error)
    } as unknown as WikiBinaryManager;

    const context = makeContext(ext.extensionUri);
    const provider = new WikiEditorProvider(ext.extensionUri, workingManager, context);

    const document = await vscode.workspace.openTextDocument(wikiFile);

    // Create a real webview panel. Its onDidReceiveMessage will route
    // the frontend's 'ready' to the handler we aim to exercise.
    const panel = vscode.window.createWebviewPanel('wiki.viewer.test', 'initial', vscode.ViewColumn.One, {
      enableScripts: true
    });

    // ------------------------------------------------------------------
    // Arrange: spy on webview.postMessage to capture all host messages.
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
      this.skip();
      // this.skip() throws (Do not resolve this test), so no return needed.
    }

    try {
      // ------------------------------------------------------------------
      // Act: resolve the custom editor. The webview loads, the frontend
      // dispatches 'ready', and the handler calls _renderPage() which posts
      // showLoading then updateContent.
      // ------------------------------------------------------------------
      const tokenSource = new vscode.CancellationTokenSource();
      await provider.resolveCustomTextEditor(document, panel, tokenSource.token);

      // ------------------------------------------------------------------
      // Wait for the initial render to complete (updateContent arrives).
      // ------------------------------------------------------------------
      await waitFor(
        () => postedMessages.some((m) => m.type === 'updateContent'),
        'Initial updateContent was never posted.',
        15000
      );

      // Record the showLoading count BEFORE the document change.
      const showLoadingCountBeforeEdit = postedMessages.filter((m) => m.type === 'showLoading').length;

      // ------------------------------------------------------------------
      // Act: simulate a keystroke by applying a workspace edit to the
      // open document. This triggers onDidChangeTextDocument, which the
      // provider is listening for (resolveCustomTextEditor L135-L137).
      //
      // The handler calls _renderPage again, which -- on the unfixed code --
      // unconditionally posts a new showLoading message.
      // ------------------------------------------------------------------
      const edit = new vscode.WorkspaceEdit();
      edit.insert(wikiFile, new vscode.Position(5, 0), '# Edited\n');
      const applied = await vscode.workspace.applyEdit(edit);
      assert.ok(applied, 'Precondition failed: workspace edit should be applied successfully');

      // ------------------------------------------------------------------
      // Wait for the re-render triggered by the document change (a second
      // updateContent message).
      // ------------------------------------------------------------------
      await waitFor(
        () => postedMessages.filter((m) => m.type === 'updateContent').length >= 2,
        'Second updateContent (triggered by document change) was never posted.\n' +
          'The onDidChangeTextDocument handler may not have fired in this test environment.',
        15000
      );

      // ------------------------------------------------------------------
      // Assert: no showLoading was posted during the document-change-
      // triggered re-render.
      //
      // Against the UNFIXED code:
      //   _renderPage() at WikiEditorProvider.ts:322 unconditionally posts
      //   showLoading BEFORE reading the document and rendering. Every call
      //   -- including the one triggered by onDidChangeTextDocument --
      //   shows the progress ring, causing a jarring flash at typing speed.
      //
      //   showLoadingCountAfterEdit === showLoadingCountBeforeEdit + 1
      //   This assertion FAILS.
      //
      // After the fix:
      //   Document-change-triggered renders skip the showLoading post.
      //   showLoadingCountAfterEdit === showLoadingCountBeforeEdit
      //   This assertion PASSES.
      // ------------------------------------------------------------------
      const showLoadingCountAfterEdit = postedMessages.filter((m) => m.type === 'showLoading').length;

      assert.strictEqual(
        showLoadingCountAfterEdit,
        showLoadingCountBeforeEdit,
        `BUG REPRODUCED: showLoading was posted during document-change-triggered render.\n` +
          `showLoading count before edit: ${showLoadingCountBeforeEdit}\n` +
          `showLoading count after edit: ${showLoadingCountAfterEdit}\n\n` +
          `At WikiEditorProvider.ts:322, _renderPage unconditionally posts ` +
          `showLoading before every render -- even those triggered by ` +
          `onDidChangeTextDocument (L135-L137). This flashes the progress ` +
          `ring on every keystroke.\n\n` +
          `All message types: ${JSON.stringify(postedMessages.map((m) => m.type))}`
      );
    } finally {
      panel.dispose();
    }
  });
});
