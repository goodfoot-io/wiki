/**
 * Reproduction test: binary install failure must NOT block article rendering.
 *
 * ## What this test covers
 * When `WikiBinaryManager.ready()` rejects in the `'ready'` message handler
 * (WikiEditorProvider.ts:139-151), the catch block posts `showError` but never
 * calls `_renderPage()`. Users who cannot install the binary (offline,
 * permissions, unsupported platform) see an error message instead of the
 * article content — even though rendering is purely in-process and requires
 * no subprocess or network.
 *
 * ## How this test reproduces the bug
 * 1. A `WikiBinaryManager` is injected whose `ready()` always rejects.
 * 2. `resolveCustomTextEditor` is called with a real wiki fixture file.
 * 3. The webview sends the `'ready'` message, triggering the handler.
 * 4. The handler calls `_binaryManager.ready()`, which throws.
 * 5. The catch handler posts `showError` and **never calls `_renderPage()`**.
 * 6. The test asserts that `updateContent` was still posted and/or that
 *    `panel.title` was updated (set inside `_renderPage()`).
 *
 * Against the unfixed code, `_renderPage()` is never invoked, so:
 * - `panel.title` stays 'initial' (never set from frontmatter)
 * - `updateContent` is never posted
 * Both assertions fail, proving the bug exists.
 *
 * @summary Reproduction: binary install failure blocks article rendering.
 * @module test/suite/wikiEditorProvider.binaryFailure.test
 */

import * as assert from 'node:assert';
import * as vscode from 'vscode';
import { WikiEditorProvider } from '../../src/providers/WikiEditorProvider.js';
import type { WikiBinaryManager } from '../../src/utils/wikiInstaller.js';
import type { HostMessage } from '../../src/webviews/wiki/types.js';

async function waitFor(predicate: () => boolean, message: string, timeoutMs = 10000): Promise<void> {
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

describe('WikiEditorProvider — binary install failure does not block article render', () => {
  afterEach(async () => {
    await vscode.commands.executeCommand('workbench.action.closeAllEditors');
  });

  it('renders article content even when binary ready() rejects', async function () {
    this.timeout(45000);

    const workspaceFolder = vscode.workspace.workspaceFolders?.[0];
    assert.ok(workspaceFolder, 'Expected test workspace folder');

    const ext = vscode.extensions.getExtension('goodfoot.wiki-extension');
    assert.ok(ext, 'Expected wiki extension to be discoverable');
    if (!ext.isActive) await ext.activate();

    // ------------------------------------------------------------------
    // Arrange: create a wiki fixture file with frontmatter (title +
    // summary so isWikiFile returns true).
    // ------------------------------------------------------------------
    const wikiDir = vscode.Uri.joinPath(workspaceFolder.uri, 'wiki');
    const wikiFile = vscode.Uri.joinPath(wikiDir, 'binary-fail.md');
    const frontmatterTitle = 'Binary Failure Should Not Block Render';
    const fixture =
      `---\ntitle: ${frontmatterTitle}\n` +
      `summary: Wiki page opened when binary install fails.\n---\n\n# Body Heading\nBody text.\n`;

    await vscode.workspace.fs.createDirectory(wikiDir);
    await vscode.workspace.fs.writeFile(wikiFile, Buffer.from(fixture));

    // ------------------------------------------------------------------
    // Arrange: a WikiBinaryManager that always rejects ready().
    // Track calls so the test can confirm the handler was exercised.
    // ------------------------------------------------------------------
    let binaryReadyCalled = false;

    const failingManager = {
      ready: async () => {
        binaryReadyCalled = true;
        throw new Error('Simulated binary install failure — offline / permissions / unsupported platform');
      },
      formatFailure: (_error: unknown) => `Simulated failure: ${_error}`
    } as unknown as WikiBinaryManager;

    // ------------------------------------------------------------------
    // Arrange: provider, document, webview panel, postMessage spy.
    // ------------------------------------------------------------------
    const context = makeContext(ext.extensionUri);
    const provider = new WikiEditorProvider(ext.extensionUri, failingManager, context);

    const document = await vscode.workspace.openTextDocument(wikiFile);
    const panel = vscode.window.createWebviewPanel('wiki.viewer.test', 'initial', vscode.ViewColumn.One, {
      enableScripts: true
    });

    // Spy on webview postMessage to capture HostMessage types posted
    // to the webview. This verifies whether the 'ready' handler sends
    // showError (bug) or updateContent + a non-replacing warning (fix).
    const postedMessages: HostMessage[] = [];
    let postMessageSpyError: Error | null = null;

    // VS Code's Webview.postMessage may be read-only in some environments.
    // Attempt to spy; if it fails, the test still runs with the
    // panel.title check as the primary assertion.
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
      // Act: resolve the custom editor. The webview loads and posts
      // 'ready'; the handler calls the failing binary manager.
      // ------------------------------------------------------------------
      const tokenSource = new vscode.CancellationTokenSource();
      await provider.resolveCustomTextEditor(document, panel, tokenSource.token);

      // ------------------------------------------------------------------
      // Assert precondition: the 'ready' handler ran (binary ready() was
      // called). This confirms the test is exercising the correct code
      // path and the webview delivered its 'ready' message.
      // ------------------------------------------------------------------
      await waitFor(
        () => binaryReadyCalled,
        'Precondition failed: binary manager ready() was never called. ' +
          'The webview may not have sent the "ready" message in this test environment.',
        15000
      );

      // ------------------------------------------------------------------
      // Primary assertion: panel.title must be updated from frontmatter,
      // proving _renderPage() was called and article content was rendered
      // despite the binary install failure.
      //
      // Against the UNFIXED code:
      //   _renderPage() is never called because the catch handler at
      //   WikiEditorProvider.ts:144-149 posts showError and skips the
      //   render entirely. panel.title remains 'initial'.
      //   This assertion FAILS — proving the bug.
      //
      // After the fix:
      //   _renderPage() runs before or alongside the binary check, the
      //   title is set from frontmatter, and the assertion PASSES.
      // ------------------------------------------------------------------
      assert.strictEqual(
        panel.title,
        frontmatterTitle,
        `BUG REPRODUCED: Binary install failure blocked article rendering.\n` +
          `Expected panel.title to be "${frontmatterTitle}" (article was rendered), ` +
          `but it is "${panel.title}".\n` +
          `The 'ready' handler at WikiEditorProvider.ts:139-151 catches ` +
          `binary manager rejection and posts showError but never calls _renderPage().`
      );

      // ------------------------------------------------------------------
      // Secondary assertion: verify the message type posted to the webview.
      // The fix should post a warning/notification (not showError) alongside
      // the rendered article. Against the unfixed code, only showError is
      // posted and updateContent is absent.
      // ------------------------------------------------------------------
      if (postMessageSpyError != null) {
        // Cannot verify message type — postMessage is read-only in this
        // environment. The panel.title assertion above still proves the
        // bug.
        console.warn('[binaryFailure.test] Cannot spy on webview.postMessage: %s', postMessageSpyError.message);
      } else if (postedMessages.length > 0) {
        // Verify that updateContent was posted (article rendered)
        const hasUpdateContent = postedMessages.some((m) => m.type === 'updateContent');
        assert.ok(
          hasUpdateContent,
          `BUG REPRODUCED: No updateContent message was posted. ` +
            `Messages: ${JSON.stringify(postedMessages.map((m) => m.type))}. ` +
            `Expected article HTML to be delivered to the webview.`
        );

        // Verify that showError was NOT the terminal message (the fix
        // should use a warning that doesn't replace the rendered page).
        const hasShowError = postedMessages.some((m) => m.type === 'showError');
        // Note: against unfixed code, showError IS posted. This assertion
        // is also expected to fail — confirming the bug. After the fix,
        // the error should be a non-replacing warning.
        assert.strictEqual(
          hasShowError,
          false,
          `BUG REPRODUCED: A showError message was posted instead of a non-replacing ` +
            `warning. The error replaces the rendered article entirely. Messages: ` +
            `${JSON.stringify(postedMessages.map((m) => m.type))}.`
        );
      }
    } finally {
      if (postMessageSpyError == null) {
        // Restore original postMessage if we spied on it
        // Note: we can't restore since we lost the reference in the try.
        // The panel will be disposed anyway.
      }
      panel.dispose();
    }
  });
});
