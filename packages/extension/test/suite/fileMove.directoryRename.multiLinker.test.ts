/**
 * Behavior-preservation test for directory renames with multiple inbound linkers.
 *
 * ## What this test covers
 * {@link WikiLanguageFeatures.buildDirectoryMoveEdit} must rewrite incoming
 * markdown links from EVERY file outside the renamed directory when a nested
 * directory containing multiple markdown files is renamed. The performance
 * restructure collapsed the previous F×(full workspace scan) loop into ONE
 * enumeration+read+scan pass over the workspace corpus; these tests pin the
 * exact observable output so the optimization cannot change it:
 *
 * - exact WorkspaceEdit entry set: which URIs are edited, how many edits each
 *   carries, and each edit's precise range + replacement text;
 * - fragments (`#anchors`) are preserved and appended to rewritten hrefs;
 * - relative-href computation matches `path.relative` semantics from the
 *   linking file's directory (including `../` climbs);
 * - files whose links do NOT target the renamed directory are untouched;
 * - moved files' own relative intra-directory links stay untouched.
 *
 * @summary Directory rename rewrites links from every inbound linker exactly.
 * @module test/suite/fileMove.directoryRename.multiLinker.test
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

/** One expected replacement inside the resulting WorkspaceEdit. */
interface ExpectedEdit {
  uri: vscode.Uri;
  line: number;
  startChar: number;
  endChar: number;
  newText: string;
}

/**
 * Assert the WorkspaceEdit contains exactly `expected`: same edited URI set,
 * same per-file edit counts, and byte-exact ranges + replacement texts.
 *
 * @param edit - The WorkspaceEdit produced by buildDirectoryMoveEdit.
 * @param expected - Every replacement that must appear, and nothing else.
 */
function assertExactEdits(edit: vscode.WorkspaceEdit, expected: ExpectedEdit[]): void {
  const byUri = new Map<string, vscode.TextEdit[]>();
  for (const [uri, edits] of edit.entries()) {
    byUri.set(uri.toString(), edits);
  }

  const expectedByUri = new Map<string, ExpectedEdit[]>();
  for (const exp of expected) {
    const list = expectedByUri.get(exp.uri.toString()) ?? [];
    list.push(exp);
    expectedByUri.set(exp.uri.toString(), list);
  }

  assert.strictEqual(
    edit.size,
    expectedByUri.size,
    `WorkspaceEdit should touch ${expectedByUri.size} file(s); got ${edit.size}: ${[...byUri.keys()].join(', ')}`
  );
  assert.strictEqual(byUri.size, expectedByUri.size, 'entries() should yield the same URI set as expected');

  for (const [uriKey, expList] of expectedByUri) {
    const actList = byUri.get(uriKey);
    assert.ok(actList, `Expected edits for ${uriKey}`);
    assert.strictEqual(actList!.length, expList.length, `Expected ${expList.length} edit(s) on ${uriKey}`);

    for (const exp of expList) {
      const match = actList!.find(
        (te) =>
          te.range.start.line === exp.line &&
          te.range.start.character === exp.startChar &&
          te.range.end.line === exp.line &&
          te.range.end.character === exp.endChar &&
          te.newText === exp.newText
      );
      assert.ok(
        match,
        `Missing edit on ${uriKey} line ${exp.line} chars [${exp.startChar},${exp.endChar}) → ${JSON.stringify(exp.newText)}; got: ${actList!
          .map(
            (te) =>
              `${te.range.start.line}:${te.range.start.character}-${te.range.end.line}:${te.range.end.character}→${JSON.stringify(te.newText)}`
          )
          .join('; ')}`
      );
    }
  }
}

describe('buildDirectoryMoveEdit — multi-linker nested rename', () => {
  const stamp = Date.now();
  const oldDirName = `nest-${stamp}`;
  const newDirName = `moved-${stamp}`;
  const outerDirName = 'multi-linker-notes';

  const aContent = `# A\n\n[Inner](${oldDirName}/inner.md)\n\n[Deep](${oldDirName}/deep/deep.md#section-2)\n`;
  const bContent = `# B\n\nBackref [Inner](../${oldDirName}/inner.md#top)\n`;
  const otherContent = `# Other\n\n[Ext](https://example.com/x.md) and [Local](./a-linker.md)\n`;
  const innerContent = `# Inner\n\nIntra [Deep](./deep/deep.md)\n`;
  const deepContent = `# Deep\n\nNo links.\n`;

  /**
   * Build an expectation for the href occurrence of `href` on `lineIdx`.
   *
   * @param content - The fixture content the file was written with.
   * @param fileUri - URI of the linker file.
   * @param lineIdx - Zero-based line carrying the link.
   * @param href - Full href as written in the fixture (fragment included).
   * @param newText - Expected replacement text for the whole href range.
   * @returns The exact-edit expectation for this replacement.
   */
  function expectHrefRewrite(
    content: string,
    fileUri: vscode.Uri,
    lineIdx: number,
    href: string,
    newText: string
  ): ExpectedEdit {
    const lineText = content.split('\n')[lineIdx]!;
    const hrefStart = lineText.indexOf(`](${href})`) + 2;
    assert.ok(hrefStart >= 2, `Test bug: href "${href}" not found on line ${lineIdx} of fixture`);
    return { uri: fileUri, line: lineIdx, startChar: hrefStart, endChar: hrefStart + href.length, newText };
  }

  afterEach(async function () {
    this.timeout(10000);
    await vscode.commands.executeCommand('workbench.action.closeAllEditors');
    const root = wsRoot();
    for (const name of [oldDirName, newDirName, outerDirName, 'a-linker.md', 'other.md']) {
      const uri = vscode.Uri.file(path.join(root, name));
      try {
        await vscode.workspace.fs.delete(uri, { recursive: true, useTrash: false });
      } catch (_err) {
        void _err;
        // File may not exist after a prior cleanup or failed setup — non-fatal.
      }
    }
  });

  it('rewrites links from multiple external files into the renamed nested directory', async function () {
    this.timeout(10000);
    const root = wsRoot();

    // --- Arrange: renamed dir holds inner.md + deep/deep.md; TWO files
    // outside it (one at root, one nested under notes/) link INTO it. ---
    const innerUri = vscode.Uri.file(path.join(root, oldDirName, 'inner.md'));
    const deepUri = vscode.Uri.file(path.join(root, oldDirName, 'deep', 'deep.md'));
    await vscode.workspace.fs.createDirectory(vscode.Uri.file(path.join(root, oldDirName, 'deep')));
    await vscode.workspace.fs.writeFile(innerUri, Buffer.from(innerContent, 'utf8'));
    await vscode.workspace.fs.writeFile(deepUri, Buffer.from(deepContent, 'utf8'));

    const aUri = vscode.Uri.file(path.join(root, 'a-linker.md'));
    await vscode.workspace.fs.writeFile(aUri, Buffer.from(aContent, 'utf8'));

    const bUri = vscode.Uri.file(path.join(root, outerDirName, 'linker-b.md'));
    await vscode.workspace.fs.createDirectory(vscode.Uri.file(path.join(root, outerDirName)));
    await vscode.workspace.fs.writeFile(bUri, Buffer.from(bContent, 'utf8'));

    const otherUri = vscode.Uri.file(path.join(root, 'other.md'));
    await vscode.workspace.fs.writeFile(otherUri, Buffer.from(otherContent, 'utf8'));

    // --- Act: rename the directory on disk, then build the edit. ---
    const oldDirUri = vscode.Uri.file(path.join(root, oldDirName));
    const newDirUri = vscode.Uri.file(path.join(root, newDirName));
    await vscode.workspace.fs.rename(oldDirUri, newDirUri, { overwrite: true });

    // Give VS Code's file index a moment to pick up the rename.
    await new Promise((resolve) => setTimeout(resolve, 500));

    const languageFeatures = new WikiLanguageFeatures(null as unknown as WikiBinaryManager);
    const edit = await languageFeatures.buildDirectoryMoveEdit(
      path.join(root, oldDirName),
      path.join(root, newDirName)
    );

    // --- Assert: exact edit set — two rewrites in a-linker.md, one in
    // notes/linker-b.md; fragments preserved; nothing else touched. ---
    const expected: ExpectedEdit[] = [
      expectHrefRewrite(aContent, aUri, 2, `${oldDirName}/inner.md`, `${newDirName}/inner.md`),
      expectHrefRewrite(
        aContent,
        aUri,
        4,
        `${oldDirName}/deep/deep.md#section-2`,
        `${newDirName}/deep/deep.md#section-2`
      ),
      expectHrefRewrite(bContent, bUri, 2, `../${oldDirName}/inner.md#top`, `../${newDirName}/inner.md#top`)
    ];
    assertExactEdits(edit, expected);

    // Moved files' own relative intra-directory links must stay untouched.
    const movedInnerUri = vscode.Uri.file(path.join(root, newDirName, 'inner.md'));
    assert.strictEqual(
      edit.get(movedInnerUri).length,
      0,
      'Relative intra-directory links inside moved files must not be rewritten'
    );

    // Unrelated linker must be absent from the edit entirely.
    assert.strictEqual(edit.get(otherUri).length, 0, 'Files with no links into the renamed dir must be untouched');
  });
});
