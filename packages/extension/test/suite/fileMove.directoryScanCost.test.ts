/**
 * Reproduction test: renaming a directory re-reads every workspace markdown
 * file once per moved page.
 *
 * {@link WikiLanguageFeatures.buildDirectoryMoveEdit} enumerates the `.md`
 * files inside the renamed directory and calls
 * {@link WikiLanguageFeatures.buildFileMoveEdit} once per moved page. Each of
 * those calls runs a full-workspace `findFiles` walk and awaits `readFile` on
 * every found file, so moving K pages costs K×N content reads for a workspace
 * of N markdown files instead of one read pass (~N reads).
 *
 * This test counts actual file-content reads by wrapping
 * `fs.promises.readFile` — the shared builtin module object that both the
 * test bundle and the extension bundle resolve to — while performing a single
 * directory move of K=6 pages alongside N=10 linker files. The discriminating
 * assertion is the read bound: at most one full pass over the workspace's
 * markdown files, which the per-page implementation exceeds by a factor of K.
 *
 * @summary Directory moves cost K×N file reads because link rewriting reruns a whole-workspace scan per moved page.
 * @module test/suite/fileMove.directoryScanCost.test
 */

import * as assert from 'node:assert';
import * as fsMod from 'node:fs';
import * as path from 'node:path';
import * as vscode from 'vscode';
import { WikiLanguageFeatures } from '../../src/providers/WikiLanguageFeatures.js';
import type { WikiBinaryManager } from '../../src/utils/wikiInstaller.js';

const MOVED_PAGE_COUNT = 6;
const LINKER_FILE_COUNT = 10;

type ReadFileLike = (...args: unknown[]) => Promise<unknown>;

function wsRoot(): string {
  const folder = vscode.workspace.workspaceFolders?.[0];
  assert.ok(folder, 'Expected a workspace folder');
  return folder.uri.fsPath;
}

/**
 * Poll findFiles until `predicate` holds over the workspace markdown paths.
 * VS Code's search index lags behind filesystem writes, so callers wait for
 * visibility rather than sleeping a fixed interval.
 *
 * @param predicate - Completion test evaluated over each enumeration's paths.
 * @param label - Human-readable phase name for the timeout error message.
 * @returns The paths from the first enumeration satisfying `predicate`.
 */
async function waitForMarkdown(predicate: (paths: string[]) => boolean, label: string): Promise<string[]> {
  const deadline = Date.now() + 5000;
  for (;;) {
    const uris = await vscode.workspace.findFiles('**/*.md', '**/node_modules/**');
    const paths = uris.map((u) => u.fsPath);
    if (predicate(paths)) return paths;
    if (Date.now() > deadline) throw new Error(`Workspace index did not settle: ${label}`);
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
}

describe('directory move scan cost', () => {
  const srcDirName = `scan-cost-src-${Date.now()}`;
  const dstDirName = `scan-cost-dst-${Date.now()}`;

  afterEach(async () => {
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
    for (let i = 0; i < LINKER_FILE_COUNT; i++) {
      try {
        await vscode.workspace.fs.delete(vscode.Uri.file(path.join(root, `linker-${i}.md`)), {
          useTrash: false
        });
      } catch (_err) {
        void _err;
      }
    }
  });

  it('reads each workspace markdown file at most once per directory move', async () => {
    const root = wsRoot();

    // --- Arrange: K pages under one directory, N linker files outside it ---
    const srcDirUri = vscode.Uri.file(path.join(root, srcDirName));
    await vscode.workspace.fs.createDirectory(srcDirUri);
    for (let p = 0; p < MOVED_PAGE_COUNT; p++) {
      const pageUri = vscode.Uri.file(path.join(root, srcDirName, `page-${p}.md`));
      await vscode.workspace.fs.writeFile(pageUri, Buffer.from(`# Page ${p}\n\nBody.\n`, 'utf8'));
    }
    for (let l = 0; l < LINKER_FILE_COUNT; l++) {
      const target = `page-${l % MOVED_PAGE_COUNT}`;
      const linkerUri = vscode.Uri.file(path.join(root, `linker-${l}.md`));
      const text = `# Linker ${l}\n\nSee [${target}](./${srcDirName}/${target}.md).\n`;
      await vscode.workspace.fs.writeFile(linkerUri, Buffer.from(text, 'utf8'));
    }

    // Wait until every fixture is visible to the workspace index before moving.
    await waitForMarkdown(
      (paths) =>
        paths.filter((p) => p.includes(`${path.sep}${srcDirName}${path.sep}`)).length === MOVED_PAGE_COUNT &&
        paths.filter((p) => p.endsWith('.md') && p.startsWith(path.join(root, 'linker-'))).length === LINKER_FILE_COUNT,
      'fixture creation'
    );

    // --- Act: rename the directory, then count markdown content reads during
    // buildDirectoryMoveEdit by wrapping fs.promises.readFile on the shared
    // builtin module object that the bundled provider code resolves to. ---
    const dstDirUri = vscode.Uri.file(path.join(root, dstDirName));
    await vscode.workspace.fs.rename(srcDirUri, dstDirUri, { overwrite: true });
    await waitForMarkdown(
      (paths) => paths.filter((p) => p.includes(`${path.sep}${dstDirName}${path.sep}`)).length === MOVED_PAGE_COUNT,
      'directory rename'
    );

    const fsp = fsMod.promises as unknown as { readFile: ReadFileLike };
    const originalReadFile = fsp.readFile.bind(fsMod.promises);
    const readPaths: string[] = [];
    fsp.readFile = async (...args: unknown[]) => {
      const first = args[0];
      if (typeof first === 'string' && first.endsWith('.md')) readPaths.push(first);
      return originalReadFile(...args);
    };

    let edit: vscode.WorkspaceEdit;
    try {
      const languageFeatures = new WikiLanguageFeatures(null as unknown as WikiBinaryManager);
      edit = await languageFeatures.buildDirectoryMoveEdit(path.join(root, srcDirName), path.join(root, dstDirName));
    } finally {
      (fsMod.promises as unknown as { readFile: ReadFileLike }).readFile = originalReadFile;
    }

    // --- Assert ---
    assert.ok(edit.size > 0, 'directory move should produce link edits');

    const workspaceMdCount = await waitForMarkdown(() => true, 'post-move enumeration');
    const totalReads = readPaths.length;

    console.log(
      `[scan-cost] workspace md files=${workspaceMdCount.length} moved pages=${MOVED_PAGE_COUNT} content reads=${totalReads}`
    );

    assert.ok(
      totalReads > 0,
      'Instrumentation observed zero markdown reads — the readFile wrapper did not intercept extension I/O.'
    );
    assert.ok(
      totalReads <= workspaceMdCount.length,
      `Expected at most ${workspaceMdCount.length} markdown reads (one pass over the workspace) but observed ${totalReads}: moving ${MOVED_PAGE_COUNT} pages re-read every workspace markdown file once per moved page (K×N growth).`
    );
  });
});
