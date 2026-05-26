/**
 * Reproduction test for "Unable to open '<file>.md' — OverlayWebview has been
 * disposed" (card main-52).
 *
 * ## What this test covers
 * `wiki.viewer` is the default custom editor for every markdown file (the
 * package.json `customEditors` selector matches all `.md` files), so a plain
 * markdown file with no wiki frontmatter is also routed into
 * [WikiEditorProvider.resolveCustomTextEditor](../../src/providers/WikiEditorProvider.ts).
 *
 * The unfixed fallback disposes the supplied `webviewPanel` *synchronously,
 * inside* `resolveCustomTextEditor`, before the method returns, and then calls
 * `showTextDocument`. Disposing a custom-editor panel mid-resolution is the
 * lifecycle violation behind the user-visible modal: on VS Code 1.121 it
 * surfaces "Unable to open '<file>.md' — OverlayWebview has been disposed" when
 * VS Code later tries to lay out the overlay it expected the provider to leave
 * alive. (On the 1.101 test runtime VS Code tolerates it, so the symptom is
 * version-dependent — but the contract violation is not.)
 *
 * This test asserts the contract directly and deterministically: for a non-wiki
 * document, `resolveCustomTextEditor` must NOT dispose its panel during
 * resolution, and the fallback must still land the file in the built-in text
 * editor. On the unfixed code the panel is already disposed the instant
 * resolution returns, so the first assertion fails.
 *
 * @summary Non-wiki markdown fallback must not dispose its panel mid-resolution.
 * @module test/suite/wikiEditorProvider.nonWikiRedirect.test
 */

import * as assert from 'node:assert';
import * as vscode from 'vscode';
import { WikiEditorProvider } from '../../src/providers/WikiEditorProvider.js';
import { WikiBinaryManager } from '../../src/utils/wikiInstaller.js';

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

async function waitFor(predicate: () => boolean, message: string, timeoutMs = 5000): Promise<void> {
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    if (predicate()) return;
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  throw new assert.AssertionError({ message });
}

describe('WikiEditorProvider — non-wiki markdown fallback', () => {
  afterEach(async () => {
    await vscode.commands.executeCommand('workbench.action.closeAllEditors');
  });

  it('does not dispose its panel during resolution, then falls back to the text editor', async function () {
    this.timeout(30000);

    const workspaceFolder = vscode.workspace.workspaceFolders?.[0];
    assert.ok(workspaceFolder, 'Expected test workspace folder');

    const ext = vscode.extensions.getExtension('goodfoot.wiki-extension');
    assert.ok(ext, 'Expected wiki extension to be discoverable');
    if (!ext.isActive) await ext.activate();

    // A plain markdown file: no YAML frontmatter, so isWikiFile() is false and
    // the provider must fall back to the default text editor.
    const plainFile = vscode.Uri.joinPath(workspaceFolder.uri, 'plain-readme.md');
    await vscode.workspace.fs.writeFile(plainFile, Buffer.from('# Plain Readme\n\nNo frontmatter here.\n'));

    const context = makeContext(ext.extensionUri);
    const provider = new WikiEditorProvider(ext.extensionUri, new WikiBinaryManager(context), context);

    const document = await vscode.workspace.openTextDocument(plainFile);
    const panel = vscode.window.createWebviewPanel('wiki.viewer.test', 'initial', vscode.ViewColumn.One, {
      enableScripts: true
    });

    let disposedDuringResolve = false;
    let disposed = false;
    panel.onDidDispose(() => {
      disposed = true;
    });

    try {
      const tokenSource = new vscode.CancellationTokenSource();
      const resolvePromise = provider.resolveCustomTextEditor(document, panel, tokenSource.token);
      await resolvePromise;
      // Snapshot disposal state at the instant resolution completes: a custom
      // editor must not dispose its own panel before resolveCustomTextEditor
      // returns. The unfixed code disposes synchronously, so `disposed` is
      // already true here.
      disposedDuringResolve = disposed;

      assert.strictEqual(
        disposedDuringResolve,
        false,
        'resolveCustomTextEditor disposed its webview panel during resolution — this is the ' +
          'lifecycle violation that surfaces "OverlayWebview has been disposed". The fallback ' +
          'must defer the dispose until after resolution returns.'
      );

      // The fallback must still happen: the plain file ends up in the built-in
      // text editor, and the redundant custom panel is eventually disposed.
      await waitFor(() => {
        const active = vscode.window.tabGroups.activeTabGroup.activeTab;
        return (
          active?.input instanceof vscode.TabInputText &&
          (active.input as vscode.TabInputText).uri.fsPath === plainFile.fsPath
        );
      }, 'Plain markdown never reached the built-in text editor — the fallback did not reopen it.');

      await waitFor(() => disposed, 'Expected the redundant custom panel to be disposed after the text editor opened.');
    } finally {
      if (!disposed) panel.dispose();
    }
  });
});
