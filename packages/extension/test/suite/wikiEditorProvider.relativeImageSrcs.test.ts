/**
 * Reproduction test: relative image srcs in wiki articles are not rewritten
 * to webview resource URIs.
 *
 * ## What this test covers
 * When a wiki article contains `![diagram](./diagram.png)`,
 * `MarkdownRenderer.render()` produces `<img src="./diagram.png">` (a bare
 * relative path — the renderer has no webview or workspace context). This HTML
 * is posted to the webview unchanged by `WikiEditorProvider._renderPage()`
 * at [L339-L342](../../src/providers/WikiEditorProvider.ts#L339-L342).
 *
 * No step in the pipeline calls `webview.asWebviewUri()` on image `src`
 * attributes. The only `asWebviewUri` calls are in `_buildShellHtml()` for
 * five extension assets (script, CSS) — never for content images.
 * See [L362-L394](../../src/providers/WikiEditorProvider.ts#L362-L394).
 *
 * ## Hypothesis
 * **H2: No `asWebviewUri` rewrite for image `src` attributes.**
 * The rendered HTML contains bare relative paths. The renderer correctly
 * has no workspace context, but the host-side `_renderPage` must rewrite
 * these paths before posting them to the webview. No such rewrite exists.
 *
 * ## How this test reproduces the bug
 * 1. A wiki fixture file with `![diagram](./diagram.png)` is created.
 * 2. `resolveCustomTextEditor` opens the file in a webview panel.
 * 3. The webview sends a `ready` message, triggering `_renderPage()`.
 * 4. The test spies on `postMessage` to capture the `updateContent` html.
 * 5. The test asserts that every `<img>` tag's `src` has been rewritten
 *    to a `vscode-webview-resource://` URI.
 *
 * **Against the unfixed code:**
 * No rewrite occurs, so `<img src="./diagram.png">` passes through
 * unchanged. The assertion fails — proving the bug.
 *
 * **After the fix:**
 * The `_renderPage` method (or a new helper) resolves each relative
 * image src against the document's directory, creates a `vscode.Uri.file()`,
 * calls `webview.asWebviewUri()`, and substitutes the result. Absolute
 * URLs (`https://...`) are left untouched.
 *
 * @summary Reproduction: relative image srcs not rewritten to webview URIs.
 * @module test/suite/wikiEditorProvider.relativeImageSrcs.test
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

describe('WikiEditorProvider — relative image srcs', () => {
  afterEach(async () => {
    await vscode.commands.executeCommand('workbench.action.closeAllEditors');
  });

  it('rewrites relative image srcs to webview resource URIs', async function () {
    this.timeout(45000);

    const workspaceFolder = vscode.workspace.workspaceFolders?.[0];
    assert.ok(workspaceFolder, 'Expected test workspace folder');

    const ext = vscode.extensions.getExtension('goodfoot.wiki-extension');
    assert.ok(ext, 'Expected wiki extension to be discoverable');
    if (!ext.isActive) await ext.activate();

    // ------------------------------------------------------------------
    // Arrange: create a wiki fixture file with a relative image reference.
    // Both `title` and `summary` are required for isWikiFile() to return
    // true so the custom editor takes over.
    // ------------------------------------------------------------------
    const wikiDir = vscode.Uri.joinPath(workspaceFolder.uri, 'wiki');
    const wikiFile = vscode.Uri.joinPath(wikiDir, 'relative-image-src.md');
    const frontmatterTitle = 'Relative Image Src Test';
    const fixture =
      `---\ntitle: ${frontmatterTitle}\n` +
      `summary: Test page with a relative image reference.\n---\n\n` +
      `# Image Test\n\n![diagram](./diagram.png)\n`;

    await vscode.workspace.fs.createDirectory(wikiDir);
    await vscode.workspace.fs.writeFile(wikiFile, Buffer.from(fixture));

    // ------------------------------------------------------------------
    // Arrange: a no-op binary manager so the binary does not interfere.
    // The current code calls _renderPage() before awaiting binary.ready(),
    // but providing a working one avoids any edge-case timing issues.
    // ------------------------------------------------------------------
    const noopManager = {
      ready: async () => {},
      formatFailure: (_error: unknown) => String(_error)
    } as unknown as WikiBinaryManager;

    const context = makeContext(ext.extensionUri);
    const provider = new WikiEditorProvider(ext.extensionUri, noopManager, context);

    const document = await vscode.workspace.openTextDocument(wikiFile);

    // Create a real webview panel. Its onDidReceiveMessage will route
    // the frontend's 'ready' to the handler we aim to exercise.
    const panel = vscode.window.createWebviewPanel('wiki.viewer.test', 'initial', vscode.ViewColumn.One, {
      enableScripts: true
    });

    // ------------------------------------------------------------------
    // Arrange: spy on webview.postMessage to capture the rendered HTML.
    // We need the `updateContent` html to inspect image src attributes.
    // ------------------------------------------------------------------
    let capturedUpdateContent: string | null = null;

    try {
      const origPostMessage = panel.webview.postMessage.bind(panel.webview);
      panel.webview.postMessage = ((message: HostMessage) => {
        if (message.type === 'updateContent') {
          capturedUpdateContent = message.html;
        }
        return origPostMessage(message);
      }) as typeof panel.webview.postMessage;
    } catch {
      // In some VS Code test environments, webview.postMessage is read-only.
      // If the spy cannot be set up, the test cannot verify the HTML content
      // and should report the environment limitation rather than silently pass.
      this.skip();
      // this.skip() throws (Do not resolve this test), so no return needed.
    }

    try {
      // ------------------------------------------------------------------
      // Act: resolve the custom editor. The webview loads, the frontend
      // dispatches 'ready', and the handler calls _renderPage() which posts
      // updateContent with the rendered HTML.
      // ------------------------------------------------------------------
      const tokenSource = new vscode.CancellationTokenSource();
      await provider.resolveCustomTextEditor(document, panel, tokenSource.token);

      // ------------------------------------------------------------------
      // Wait for the updateContent message to arrive.
      // ------------------------------------------------------------------
      await waitFor(
        () => capturedUpdateContent != null,
        'updateContent was never posted to the webview. ' +
          'The webview may not have sent the "ready" message in this test environment.',
        15000
      );

      // ------------------------------------------------------------------
      // Assert: every <img> tag's src must be a webview resource URI.
      //
      // Against the UNFIXED code:
      //   _renderPage() posts the raw HTML from MarkdownRenderer.render()
      //   without any asWebviewUri rewrite. The img src is still the bare
      //   relative path "./diagram.png". This assertion FAILS — proving
      //   that no image src rewrite exists in the pipeline.
      //
      // After the fix:
      //   Relative image srcs are resolved against the document directory,
      //   converted via webview.asWebviewUri(), and the HTML is rewritten
      //   before posting. This assertion then PASSES.
      // ------------------------------------------------------------------

      // Find all img src values in the rendered content.
      // capturedUpdateContent is guaranteed non-null here because waitFor
      // above threw if it was still null after the timeout.
      const html: string = capturedUpdateContent as NonNullable<typeof capturedUpdateContent>;
      const imgSrcs: string[] = [];
      const imgRegex = /<img\b[^>]*\bsrc="([^"]+)"/g;
      let match: RegExpExecArray | null = imgRegex.exec(html);

      while (match !== null) {
        const src = match[1];
        if (src !== undefined) {
          imgSrcs.push(src);
        }
        match = imgRegex.exec(html);
      }

      assert.ok(
        imgSrcs.length > 0,
        'Precondition failed: no <img> tags found in rendered HTML. ' +
          'Expected the markdown image to produce at least one <img> tag. ' +
          `HTML excerpt: ${html.slice(0, 500)}`
      );

      // Every img src must be a webview resource URI (not a bare relative path)
      const nonWebviewSrcs = imgSrcs.filter((src) => !src.startsWith('vscode-webview-resource://'));
      assert.strictEqual(
        nonWebviewSrcs.length,
        0,
        `BUG REPRODUCED: ${nonWebviewSrcs.length} image src(s) were NOT rewritten to ` +
          `vscode-webview-resource:// URIs.\n\n` +
          `Unrewritten srcs: ${JSON.stringify(nonWebviewSrcs)}\n\n` +
          `Expected every <img src> to be a webview resource URI, but no ` +
          `asWebviewUri rewrite exists in the _renderPage pipeline. ` +
          `The HTML from MarkdownRenderer.render() is posted to the webview ` +
          `verbatim at WikiEditorProvider.ts:339-342.\n\n` +
          `HTML excerpt: ${html.slice(0, 300)}`
      );
    } finally {
      panel.dispose();
    }
  });
});
