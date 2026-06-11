/**
 * Reproduction test: renaming a directory does not rewrite incoming markdown links.
 *
 * The onDidRenameFiles handler in extension.ts filters out non-.md URIs at line 46,
 * so directory rename events are silently skipped. The fix adds
 * {@link WikiLanguageFeatures.buildDirectoryMoveEdit}, which enumerates .md files
 * inside the renamed directory and delegates to {@link WikiLanguageFeatures.buildFileMoveEdit}
 * for each file pair.
 *
 * This test calls buildDirectoryMoveEdit directly because onDidRenameFiles only
 * fires for user-initiated renames and cannot be triggered programmatically.
 * Before the fix, the method does not exist and the test fails at the call site.
 *
 * @summary Directory renames skip link rewriting because the handler filters on .md extension.
 * @module test/suite/fileMove.directoryRename.test
 */

import * as assert from 'node:assert';
import * as path from 'node:path';
import * as vscode from 'vscode';
import { WikiLanguageFeatures } from '../../src/providers/WikiLanguageFeatures.js';
import type { WikiBinaryManager } from '../../src/utils/wikiInstaller.js';

function wsRoot(): string {
  const folder = vscode.workspace.workspaceFolders?.[0];
  assert.ok(folder, 'Expected a workspace folder');
  return folder.uri.fsPath;
}

describe('buildDirectoryMoveEdit', () => {
  const subDirName = `subdir-${Date.now()}`;
  const newDirName = `renamed-${Date.now()}`;

  afterEach(async () => {
    await vscode.commands.executeCommand('workbench.action.closeAllEditors');
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

  it('rewrites incoming links when a directory of markdown files is renamed', async () => {
    const root = wsRoot();

    // --- Arrange: create a-linker.md linking to subdir/b.md, and subdir/b.md ---
    const subDirUri = vscode.Uri.file(path.join(root, subDirName));
    await vscode.workspace.fs.createDirectory(subDirUri);

    const bFileUri = vscode.Uri.file(path.join(root, subDirName, 'b.md'));
    await vscode.workspace.fs.writeFile(bFileUri, Buffer.from('# B\n\nContent of B.\n', 'utf8'));

    const aFileUri = vscode.Uri.file(path.join(root, 'a-linker.md'));
    const linkText = `# A\n\nLink to [B](./${subDirName}/b.md).\n`;
    await vscode.workspace.fs.writeFile(aFileUri, Buffer.from(linkText, 'utf8'));

    // --- Act: rename the directory on disk, open the linker file so
    // WorkspaceEdit can modify its text buffer, then call buildDirectoryMoveEdit. ---
    const newDirUri = vscode.Uri.file(path.join(root, newDirName));
    await vscode.workspace.fs.rename(subDirUri, newDirUri, { overwrite: true });

    // Give VS Code's file index a moment to pick up the rename.
    await new Promise((resolve) => setTimeout(resolve, 500));

    // Open the linker file — applyEdit requires an open text document to
    // modify file content in the test environment.
    const doc = await vscode.workspace.openTextDocument(aFileUri);
    await vscode.window.showTextDocument(doc, { preview: false });

    const languageFeatures = new WikiLanguageFeatures(null as unknown as WikiBinaryManager);

    // buildDirectoryMoveEdit is the testable unit that the onDidRenameFiles
    // handler delegates to for directory renames. Before the fix this method
    // does not exist — the test fails here at runtime.
    const edit = await languageFeatures.buildDirectoryMoveEdit(
      path.join(root, subDirName),
      path.join(root, newDirName)
    );

    assert.ok(edit.size > 0, 'buildDirectoryMoveEdit should generate at least one edit');

    const applied = await vscode.workspace.applyEdit(edit);
    assert.ok(applied, 'applyEdit of the directory-move edit should succeed');

    // Flush the edit to disk and read the document text.
    await doc.save();
    const content = doc.getText();

    // --- Assert: the link was rewritten to point to the new directory ---
    // buildFileMoveEdit uses path.relative which strips "./" from the href,
    // so the rewritten href will be "renamed-.../b.md" rather than "./renamed-.../b.md".
    // Both forms are valid relative markdown links; check that the old directory
    // name is gone and the new one appears.
    const oldLink = `${subDirName}/b.md`;
    const newLink = `${newDirName}/b.md`;
    assert.ok(
      !content.includes(oldLink),
      `Expected old link "${oldLink}" to be removed. Content: ${JSON.stringify(content)}`
    );
    assert.ok(
      content.includes(newLink),
      `Expected new link "${newLink}" to appear. Content: ${JSON.stringify(content)}`
    );
  });
});
