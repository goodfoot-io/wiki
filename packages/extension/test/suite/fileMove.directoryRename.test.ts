/**
 * Reproduction test: renaming a directory does not rewrite incoming markdown links.
 *
 * The onDidRenameFiles handler in extension.ts filters out non-.md URIs at line 46,
 * so directory rename events are silently skipped. Links pointing into the renamed
 * directory remain stale.
 *
 * @summary Directory renames skip link rewriting because the handler filters on .md extension.
 * @module test/suite/fileMove.directoryRename.test
 */

import * as assert from 'node:assert';
import * as path from 'path';
import * as vscode from 'vscode';

function wsRoot(): string {
  const folder = vscode.workspace.workspaceFolders?.[0];
  assert.ok(folder, 'Expected a workspace folder');
  return folder.uri.fsPath;
}

async function readText(fileUri: vscode.Uri): Promise<string> {
  const bytes = await vscode.workspace.fs.readFile(fileUri);
  return Buffer.from(bytes).toString('utf8');
}

describe('onDidRenameFiles — directory rename', () => {
  const subDirName = 'subdir-' + Date.now();
  const newDirName = 'renamed-' + Date.now();

  afterEach(async () => {
    // Clean up any files created during the test
    const root = wsRoot();
    for (const name of [subDirName, newDirName, 'a-linker.md']) {
      const uri = vscode.Uri.file(path.join(root, name));
      try {
        await vscode.workspace.fs.delete(uri, { recursive: true, useTrash: false });
      } catch (_err) {
        void _err;
        // File may not exist after a prior cleanup or failed setup — non-fatal.
      }
    }
  });

  it('directory rename: links into the renamed directory are not rewritten', async () => {
    const root = wsRoot();

    // --- Arrange: create a.md linking to subdir/b.md, and subdir/b.md ---
    const subDir = vscode.Uri.file(path.join(root, subDirName));
    await vscode.workspace.fs.createDirectory(subDir);

    const bFile = vscode.Uri.file(path.join(root, subDirName, 'b.md'));
    await vscode.workspace.fs.writeFile(bFile, Buffer.from('# B\n\nContent of B.\n', 'utf8'));

    const aFile = vscode.Uri.file(path.join(root, 'a-linker.md'));
    const linkText = `# A\n\nLink to [B](./${subDirName}/b.md).\n`;
    await vscode.workspace.fs.writeFile(aFile, Buffer.from(linkText, 'utf8'));

    // --- Act: rename the directory ---
    const oldUri = vscode.Uri.file(path.join(root, subDirName));
    const newUri = vscode.Uri.file(path.join(root, newDirName));

    const edit = new vscode.WorkspaceEdit();
    edit.renameFile(oldUri, newUri);
    const applied = await vscode.workspace.applyEdit(edit);
    assert.ok(applied, 'applyEdit should succeed');

    // Allow the onDidRenameFiles handler time to fire and complete.
    // (applyEdit resolves after the rename hits disk; the handler fires asynchronously.)
    await new Promise((resolve) => setTimeout(resolve, 1000));

    // --- Assert: the link was rewritten to the new directory ---
    const content = await readText(aFile);

    // After a directory rename, incoming links must point to the new location.
    // This assertion FAILS before the fix because the handler skips directory URIs.
    assert.ok(
      content.includes(`./${newDirName}/b.md`),
      `Expected link in a-linker.md to be rewritten to ./${newDirName}/b.md after directory rename, ` +
        `but handler filtered out the directory URI. Content: ${content}`
    );
  });
});
