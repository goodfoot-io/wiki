/**
 * Reproduction test for "Unable to open '<file>.md' — OverlayWebview has been
 * disposed" (card main-52).
 *
 * ## What this test covers
 * `wiki.viewer` is registered as the default custom editor for every markdown
 * file (the package.json `customEditors` selector matches all `.md` files). A
 * plain markdown file with no wiki frontmatter is therefore routed into
 * [WikiEditorProvider.resolveCustomTextEditor](../../src/providers/WikiEditorProvider.ts),
 * which is expected to fall back to VS Code's built-in text editor.
 *
 * The current fallback disposes the supplied `webviewPanel` synchronously
 * *inside* `resolveCustomTextEditor` and then calls `showTextDocument`. Doing
 * so mid-resolution leaves a zombie custom-editor tab (its overlay webview is
 * already disposed) instead of a clean text editor, and corrupts the custom
 * editor lifecycle so subsequent webview opens fail with
 * "OverlayWebview has been disposed".
 *
 * This test opens a non-wiki `.md` file the way the user does — through
 * `vscode.open`, which honors the default editor association — and asserts the
 * file ends up in a plain text editor (`TabInputText`). On the unfixed code the
 * file is stranded in a `wiki.viewer` custom tab, so the assertion fails.
 *
 * @summary Non-wiki markdown must fall back to the text editor, not a zombie webview.
 * @module test/suite/wikiEditorProvider.nonWikiRedirect.test
 */

import * as assert from 'node:assert';
import * as vscode from 'vscode';

async function waitForTab(
  predicate: (tab: vscode.Tab | undefined) => boolean,
  message: string,
  timeoutMs = 5000
): Promise<vscode.Tab> {
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    const activeTab = vscode.window.tabGroups.activeTabGroup.activeTab;
    if (predicate(activeTab)) {
      return activeTab as vscode.Tab;
    }
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  throw new assert.AssertionError({ message });
}

describe('WikiEditorProvider — non-wiki markdown fallback', () => {
  afterEach(async () => {
    await vscode.commands.executeCommand('workbench.action.closeAllEditors');
  });

  it('opens a frontmatter-less .md in the built-in text editor, not the wiki webview', async () => {
    const workspaceFolder = vscode.workspace.workspaceFolders?.[0];
    assert.ok(workspaceFolder, 'Expected test workspace folder');

    const ext = vscode.extensions.getExtension('goodfoot.wiki-extension');
    assert.ok(ext, 'Expected wiki extension to be discoverable');
    if (!ext.isActive) await ext.activate();

    // A plain markdown file: no YAML frontmatter, so isWikiFile() is false and
    // the provider must fall back to the default text editor.
    const plainFile = vscode.Uri.joinPath(workspaceFolder.uri, 'plain-readme.md');
    await vscode.workspace.fs.writeFile(plainFile, Buffer.from('# Plain Readme\n\nNo frontmatter here.\n'));

    await vscode.commands.executeCommand('vscode.open', plainFile);

    const tab = await waitForTab(
      (t) => t?.input instanceof vscode.TabInputText || t?.input instanceof vscode.TabInputCustom,
      'Expected a tab to open for the plain markdown file'
    );

    if (tab.input instanceof vscode.TabInputCustom) {
      assert.fail(
        `Plain markdown opened in custom editor "${tab.input.viewType}" instead of the built-in ` +
          'text editor — the dispose-and-redirect fallback stranded the file in a wiki.viewer tab.'
      );
    }

    assert.ok(tab.input instanceof vscode.TabInputText, 'Expected the plain markdown file to open as a text editor');
    assert.strictEqual(
      (tab.input as vscode.TabInputText).uri.fsPath,
      plainFile.fsPath,
      'Expected the text editor to show the plain markdown file'
    );
  });
});
