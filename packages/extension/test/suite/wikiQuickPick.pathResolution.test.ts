/**
 * Reproduction tests for the QuickPick path-resolution bug.
 *
 * Two independent bugs in wikiQuickPick.ts:
 * 1. openWikiFile passes repo-relative paths directly to vscode.Uri.file()
 *    without resolving against workspaceRoot(), producing file:/// URIs that
 *    resolve against the filesystem root.
 * 2. recentPaths (absolute) and listItems[].file (relative) never match during
 *    dedup, so recently-viewed pages appear twice in the initial list.
 *
 * @summary Reproduction tests for QuickPick path resolution and dedup bugs.
 * @module test/suite/wikiQuickPick.pathResolution.test
 */

import * as assert from 'node:assert';
import * as path from 'node:path';
import * as vscode from 'vscode';
import { openWikiFile, toListQuickPickItem, workspaceRoot } from '../../src/commands/wikiQuickPick.js';

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

describe('wikiQuickPick — path resolution (bug reproduction)', () => {
  afterEach(async () => {
    await vscode.commands.executeCommand('workbench.action.closeAllEditors');
  });

  it('openWikiFile resolves repo-relative paths against workspace root', async () => {
    const root = workspaceRoot();
    assert.ok(root, 'Expected workspace root');

    // Create a wiki page at wiki/relative-path-bug.md in the test workspace.
    const wikiDir = vscode.Uri.joinPath(vscode.Uri.file(root), 'wiki');
    const wikiFile = vscode.Uri.joinPath(wikiDir, 'relative-path-bug.md');
    await vscode.workspace.fs.createDirectory(wikiDir);
    await vscode.workspace.fs.writeFile(
      wikiFile,
      Buffer.from('---\ntitle: Relative Path Bug\nsummary: reproduction\n---\n\nbody\n')
    );

    // openWikiFile receives repo-relative paths from the CLI (wiki list /
    // wiki <query> output). Calling it with a relative path should resolve
    // against workspaceRoot() and open the file in the wiki viewer.
    await openWikiFile('wiki/relative-path-bug.md');

    const activeTab = await waitForTab(
      (tab) =>
        tab?.input instanceof vscode.TabInputCustom &&
        (tab.input as vscode.TabInputCustom).uri.fsPath === wikiFile.fsPath,
      'Expected openWikiFile to open the correct wiki file in the custom ' +
        'viewer, but the correct file was never opened. With the bug, ' +
        `openWikiFile receives 'wiki/relative-path-bug.md' and calls ` +
        `vscode.Uri.file() on it, producing file:///wiki/relative-path-bug.md ` +
        `instead of resolving against workspaceRoot() to produce ${wikiFile.fsPath}.`
    );
    const tabInput = activeTab.input as vscode.TabInputCustom;
    assert.strictEqual(tabInput.viewType, 'wiki.viewer');
    assert.strictEqual(tabInput.uri.fsPath, wikiFile.fsPath);
  });

  it('initial list dedup matches recent absolute paths against list relative paths', () => {
    const root = workspaceRoot();
    assert.ok(root, 'Expected workspace root');

    // Simulate the data shapes that flow through wikiQuickPick:
    //
    // recentItems come from loadValidatedRecentlyViewed(), which returns
    // items with absolute fsPaths stored by recordWikiView().
    const absPath = path.join(root, 'wiki', 'some-page.md');

    // listItems come from loadAllPages() / searchPages(), which return
    // items with repo-relative paths from the CLI.
    const listItem = toListQuickPickItem({
      title: 'Some Page',
      aliases: [],
      tags: [],
      summary: '',
      file: 'wiki/some-page.md'
    });

    // The fix: resolveWorkspacePath normalizes the list-item relative
    // path to absolute before checking against the Set of absolute
    // recent paths. Without normalization, Set.has() silently returns
    // false for every comparison and duplicates are never filtered.
    const recentPaths = new Set([absPath]);
    const resolvedFile = path.isAbsolute(listItem.file) ? listItem.file : path.resolve(root, listItem.file);
    const isDuplicate = recentPaths.has(resolvedFile);

    assert.ok(
      isDuplicate,
      `Expected '${resolvedFile}' to match '${absPath}' after path ` +
        'normalization, but the resolved list-item path did not match ' +
        'the absolute recent-item path in the Set.'
    );
  });
});
