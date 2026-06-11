/**
 * Reproduction test: `WikiEditorProvider` needlessly blocks initial article
 * rendering on `WikiBinaryManager.ready()`, which can hang or fail when
 * offline.
 *
 * ## What this test covers
 * The `ready` webview-message handler
 * ([WikiEditorProvider.ts:139-151](../../src/providers/WikiEditorProvider.ts))
 * awaits `this._binaryManager.ready()` BEFORE calling `_renderPage()`. Since
 * `_renderPage()` reads the file, parses frontmatter, and renders markdown
 * entirely in-process — without ever invoking the wiki CLI binary — rendering
 * is needlessly blocked on binary initialization (which downloads the binary
 * from GitHub releases and can hang or fail offline).
 *
 * This test drives `WikiEditorProvider.resolveCustomTextEditor` with a
 * `WikiBinaryManager` whose `ready()` never resolves, simulating an offline
 * or hung binary installation. It waits for the webview to load and dispatch
 * its `ready` message, then asserts that `panel.title` changed from its
 * initial value to the frontmatter-derived title — proving that `_renderPage`
 * executed despite the binary never becoming ready.
 *
 * **This assertion MUST FAIL on the current unfixed code** because the
 * `ready` handler awaits the never-resolving `binary.ready()` first, so
 * `_renderPage` never runs and `panel.title` stays at the initial value.
 *
 * After the fix — moving `_renderPage` before the `await binary.ready()`
 * call — the assertion should pass.
 *
 * @summary Initial article render is needlessly blocked on binary ready().
 * @module test/suite/wikiEditorProvider.renderBlockedOnBinary.test
 */

import * as assert from 'node:assert';
import * as vscode from 'vscode';
import { WikiEditorProvider } from '../../src/providers/WikiEditorProvider.js';
import type { WikiBinaryManager } from '../../src/utils/wikiInstaller.js';

/**
 * A stub `WikiBinaryManager` whose `ready()` returns a promise that never
 * settles. This simulates the scenario where the wiki CLI binary is
 * unavailable (offline network, slow installation, or infrastructure
 * failure) — exactly the condition under which article rendering should
 * NOT be blocked.
 */
class NeverReadyWikiBinaryManager {
  ready(): Promise<never> {
    return new Promise<never>(() => {
      /* never resolves */
    });
  }

  formatFailure(error: unknown): string {
    return String(error);
  }
}

/**
 * Minimal real ExtensionContext: only the members the render path touches
 * are populated (`extensionUri` + a `Map`-backed `workspaceState`).
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

async function waitFor(predicate: () => boolean, message: string, timeoutMs = 15000): Promise<void> {
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    if (predicate()) return;
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  throw new assert.AssertionError({ message });
}

describe('WikiEditorProvider — render blocked on binary ready', () => {
  afterEach(async () => {
    await vscode.commands.executeCommand('workbench.action.closeAllEditors');
  });

  it('renders the page even when binary ready() never resolves', async function () {
    this.timeout(45000);

    const workspaceFolder = vscode.workspace.workspaceFolders?.[0];
    assert.ok(workspaceFolder, 'Expected test workspace folder');

    const ext = vscode.extensions.getExtension('goodfoot.wiki-extension');
    assert.ok(ext, 'Expected wiki extension to be discoverable');
    if (!ext.isActive) await ext.activate();

    // ------------------------------------------------------------------
    // Arrange: a wiki fixture file with frontmatter (both title + summary
    // for isWikiFile()). Use a fresh path so parallel test runs don't
    // contend.
    // ------------------------------------------------------------------
    const wikiDir = vscode.Uri.joinPath(workspaceFolder.uri, 'wiki');
    const wikiFile = vscode.Uri.joinPath(wikiDir, 'render-before-binary.md');
    const frontmatterTitle = 'Render Before Binary';
    const fixture =
      `---\ntitle: ${frontmatterTitle}\n` +
      `summary: A fixture page for the render-blocked-on-binary regression test.\n---\n\n# Body\n`;

    await vscode.workspace.fs.createDirectory(wikiDir);
    await vscode.workspace.fs.writeFile(wikiFile, Buffer.from(fixture));

    // ------------------------------------------------------------------
    // Arrange: a provider whose binary manager never becomes ready.
    // ------------------------------------------------------------------
    const context = makeContext(ext.extensionUri);
    const binaryManager = new NeverReadyWikiBinaryManager();
    const provider = new WikiEditorProvider(ext.extensionUri, binaryManager as unknown as WikiBinaryManager, context);

    const document = await vscode.workspace.openTextDocument(wikiFile);

    // Create a real webview panel. Its onDidReceiveMessage will route
    // the frontend's 'ready' to the handler we aim to exercise.
    const panel = vscode.window.createWebviewPanel('wiki.viewer.test', 'initial', vscode.ViewColumn.One, {
      enableScripts: true
    });

    try {
      const tokenSource = new vscode.CancellationTokenSource();
      await provider.resolveCustomTextEditor(document, panel, tokenSource.token);

      // ------------------------------------------------------------------
      // Act + Assert: wait for the page to render.
      //
      // resolveCustomTextEditor has now:
      //  1. Set the webview shell HTML (loads the frontend JS)
      //  2. Registered the onDidReceiveMessage handler for 'ready'
      //
      // The frontend will load and dispatch 'ready'. On the current code,
      // the handler awaits binaryManager.ready() (which never resolves)
      // BEFORE calling _renderPage(), so _renderPage() never runs and
      // panel.title stays at "initial".
      //
      // After the fix, _renderPage() runs before awaiting binary.ready(),
      // so panel.title changes to the frontmatter title.
      //
      // This waitFor will time out (throwing AssertionError) on the
      // unfixed code, making the test fail — which is the expected
      // outcome for a correct reproduction test.
      // ------------------------------------------------------------------
      await waitFor(
        () => panel.title === frontmatterTitle,
        `panel.title never changed from "${panel.title}" to ` +
          `"${frontmatterTitle}" — _renderPage() did not execute because ` +
          `the 'ready' handler is blocked on binaryManager.ready().`
      );

      // If we reach here, the test PASSES — indicating that _renderPage
      // ran despite the binary never becoming ready. On the current code
      // this branch is unreachable (waitFor throws).
    } finally {
      panel.dispose();
    }
  });
});
