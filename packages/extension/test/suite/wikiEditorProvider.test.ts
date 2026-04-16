/**
 * Integration tests for WikiEditorProvider.
 *
 * Verifies extension activation and custom editor command registration using the
 * VS Code extension API. Tests that require a file on disk (e.g., opening a .md
 * file in the custom editor) are not included here — that interaction requires
 * more complex workspace setup and is covered by manual EDH verification.
 *
 * @summary WikiEditorProvider registration and activation tests.
 * @module test/suite/wikiEditorProvider.test
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

describe('WikiEditorProvider', () => {
  afterEach(async () => {
    await vscode.commands.executeCommand('workbench.action.closeAllEditors');
  });

  it('extension activates successfully', async () => {
    const ext = vscode.extensions.getExtension('goodfoot.wiki-extension');
    assert.ok(ext, 'Extension not found — is publisher.name "goodfoot.wiki-extension"?');
    if (!ext.isActive) {
      await ext.activate();
    }
    assert.ok(ext.isActive, 'Extension did not activate');
  });

  it('wiki.openInEditor command is registered', async () => {
    const commands = await vscode.commands.getCommands();
    assert.ok(commands.includes('wiki.openInEditor'), 'wiki.openInEditor not registered');
  });

  it('opens wiki files in the custom webview during normal open', async () => {
    const workspaceFolder = vscode.workspace.workspaceFolders?.[0];
    assert.ok(workspaceFolder, 'Expected test workspace folder');

    const wikiDir = vscode.Uri.joinPath(workspaceFolder.uri, 'wiki');
    const wikiFile = vscode.Uri.joinPath(wikiDir, 'normal-open.md');

    await vscode.workspace.fs.createDirectory(wikiDir);
    await vscode.workspace.fs.writeFile(wikiFile, Buffer.from('# Normal Open\n'));

    await vscode.commands.executeCommand('vscode.open', wikiFile);

    const activeTab = await waitForTab(
      (tab) => tab?.input instanceof vscode.TabInputCustom,
      'Expected the wiki file to open in the custom viewer'
    );
    const tabInput = activeTab.input as vscode.TabInputCustom;
    assert.strictEqual(tabInput.viewType, 'wiki.viewer');
  });
});
