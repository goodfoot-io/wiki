/**
 * Characterization test: directory-move link rewriting keeps its exact
 * semantics while scanning the workspace in a single pass.
 *
 * Pins the observable outcomes that {@link WikiLanguageFeatures.buildDirectoryMoveEdit}
 * must preserve across the single-pass refactor: relative hrefs, `/`-rooted
 * hrefs, fragments (`page.md#anchor`), nested pages inside subdirectories,
 * multiple links on one line targeting different moved pages, and targets
 * that must stay untouched (external URLs, links to non-moved files).
 *
 * These assertions hold against both the pre- and post-refactor
 * implementation — they guard equivalence, they do not discriminate the bug.
 *
 * @summary Directory-move rewriting preserves href semantics for relative, rooted, fragment, multi-link, and untouched targets.
 * @module test/suite/fileMove.directoryScanSemantics.test
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

/**
 * Poll findFiles until every expected path is visible to the search index.
 *
 * @param expected - Absolute paths that must all appear in an enumeration.
 * @param label - Human-readable phase name for the timeout error message.
 * @returns Resolves once every path is present; rejects on timeout.
 */
async function waitForFiles(expected: string[], label: string): Promise<void> {
  const deadline = Date.now() + 5000;
  for (;;) {
    const uris = await vscode.workspace.findFiles('**/*.md', '**/node_modules/**');
    const present = new Set(uris.map((u) => u.fsPath));
    const missing = expected.filter((p) => !present.has(p));
    if (missing.length === 0) return;
    if (Date.now() > deadline)
      throw new Error(`Workspace index did not settle (${label}); missing: ${missing.join(', ')}`);
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
}

describe('buildDirectoryMoveEdit semantics', () => {
  const srcDirName = `sem-src-${Date.now()}`;
  const dstDirName = `sem-dst-${Date.now()}`;

  /** Linker file name -> exact expected content after the move. */
  const expectations = new Map<string, string>();

  afterEach(async () => {
    await vscode.commands.executeCommand('workbench.action.closeAllEditors');
    const root = wsRoot();
    for (const name of [srcDirName, dstDirName]) {
      try {
        await vscode.workspace.fs.delete(vscode.Uri.file(path.join(root, name)), {
          recursive: true,
          useTrash: false
        });
      } catch (_err) {
        void _err;
      }
    }
    for (const name of [...expectations.keys(), 'plain-target.md']) {
      try {
        await vscode.workspace.fs.delete(vscode.Uri.file(path.join(root, name)), { useTrash: false });
      } catch (_err) {
        void _err;
      }
    }
    expectations.clear();
  });

  it('rewrites exactly the expected hrefs and leaves everything else untouched', async () => {
    const root = wsRoot();
    const src = (rel: string): string => path.join(root, srcDirName, rel);
    const dst = (rel: string): string => path.join(root, dstDirName, rel);

    // --- Arrange: moved pages (top-level and nested), then five linkers ---
    await vscode.workspace.fs.createDirectory(vscode.Uri.file(path.join(root, srcDirName, 'nested')));
    await vscode.workspace.fs.writeFile(vscode.Uri.file(src('page-a.md')), Buffer.from('# Page A\n', 'utf8'));
    await vscode.workspace.fs.writeFile(
      vscode.Uri.file(src(path.join('nested', 'page-b.md'))),
      Buffer.from('# Page B\n', 'utf8')
    );

    const writeLinker = async (name: string, before: string, after: string): Promise<void> => {
      expectations.set(name, after);
      await vscode.workspace.fs.writeFile(vscode.Uri.file(path.join(root, name)), Buffer.from(before, 'utf8'));
    };

    // Relative href to a top-level moved page.
    await writeLinker(
      'rel-linker.md',
      `# Rel\n\nSee [A](./${srcDirName}/page-a.md).\n`,
      `# Rel\n\nSee [A](${dstDirName}/page-a.md).\n`
    );

    // Workspace-root-absolute href to a nested moved page.
    await writeLinker(
      'rooted-linker.md',
      `# Rooted\n\nSee [B](/${srcDirName}/nested/page-b.md).\n`,
      `# Rooted\n\nSee [B](${dstDirName}/nested/page-b.md).\n`
    );

    // Fragment on a moved target survives the rewrite.
    await writeLinker(
      'fragment-linker.md',
      `# Frag\n\nSee [A](./${srcDirName}/page-a.md#anchor).\n`,
      `# Frag\n\nSee [A](${dstDirName}/page-a.md#anchor).\n`
    );

    // External URL and a link to a non-moved file are untouched.
    await writeLinker(
      'untouched-linker.md',
      `# Untouched\n\nSite [x](https://example.com/x). Local [y](./plain-target.md).\n`,
      `# Untouched\n\nSite [x](https://example.com/x). Local [y](./plain-target.md).\n`
    );
    await vscode.workspace.fs.writeFile(
      vscode.Uri.file(path.join(root, 'plain-target.md')),
      Buffer.from('# Plain\n', 'utf8')
    );

    // Two links on one line, each targeting a different moved page.
    await writeLinker(
      'multi-linker.md',
      `# Multi\n\nPair [A](./${srcDirName}/page-a.md) and [B](./${srcDirName}/nested/page-b.md).\n`,
      `# Multi\n\nPair [A](${dstDirName}/page-a.md) and [B](${dstDirName}/nested/page-b.md).\n`
    );

    await vscode.commands.executeCommand('workbench.action.closeAllEditors');
    const expectedPaths = [
      src('page-a.md'),
      src(path.join('nested', 'page-b.md')),
      ...[...expectations.keys()].map((name) => path.join(root, name))
    ];
    await waitForFiles(expectedPaths, 'fixture creation');

    // --- Act: rename the directory, wait for the index, rewrite ---
    await vscode.workspace.fs.rename(
      vscode.Uri.file(path.join(root, srcDirName)),
      vscode.Uri.file(path.join(root, dstDirName)),
      { overwrite: true }
    );
    await waitForFiles([dst('page-a.md'), dst(path.join('nested', 'page-b.md'))], 'directory rename');

    const languageFeatures = new WikiLanguageFeatures(null as unknown as WikiBinaryManager);
    const edit = await languageFeatures.buildDirectoryMoveEdit(
      path.join(root, srcDirName),
      path.join(root, dstDirName)
    );
    assert.ok(edit.size > 0, 'directory move should produce link edits');

    const docs = new Map<string, vscode.TextDocument>();
    for (const name of expectations.keys()) {
      const uri = vscode.Uri.file(path.join(root, name));
      const doc = await vscode.workspace.openTextDocument(uri);
      await vscode.window.showTextDocument(doc, { preview: true });
      docs.set(name, doc);
    }

    assert.ok(await vscode.workspace.applyEdit(edit), 'applyEdit of the directory-move edit should succeed');

    // --- Assert: every linker file matches its expected content exactly ---
    for (const [name, expected] of expectations) {
      const doc = docs.get(name);
      assert.ok(doc, `document ${name} was not opened`);
      await doc.save();
      const actual = doc.getText();
      assert.strictEqual(actual, expected, `content mismatch for ${name}`);
    }
  });
});
