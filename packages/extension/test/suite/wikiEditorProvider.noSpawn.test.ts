/**
 * Regression test: the wiki page render path performs no `wiki`-CLI
 * subprocess spawn, and derives the tab title from in-process frontmatter.
 *
 * ## What this test covers
 * `_renderPage()` previously spawned `wiki summary` on every page open and
 * live-reload to derive the tab title. That spawn is removed; the title now
 * comes from `parseFrontmatter(text).title`.
 *
 * The test drives the real `WikiEditorProvider` against a real
 * `vscode.WebviewPanel` and a real on-disk wiki fixture. It patches
 * `child_process.spawn` — the exact primitive `runWikiCommand` uses
 * (wikiBinary.ts:268) — to record every spawned argument vector. After
 * resolving the custom editor it triggers a live-reload by editing the
 * open document (the `onDidChangeTextDocument` path that calls
 * `_renderPage` directly), then asserts (a) no `summary` wiki
 * subcommand was ever spawned and (b) `webviewPanel.title` equals the
 * fixture frontmatter `title`.
 *
 * `webviewPanel.title` (not `Tab.label`) is the observable: VS Code's
 * custom-editor `Tab.label` is bound to the resource name, while the
 * title the provider sets is `webviewPanel.title`.
 *
 * @summary No-spawn render path + in-process title regression test.
 * @module test/suite/wikiEditorProvider.noSpawn.test
 */

import * as assert from 'node:assert';
import { createRequire } from 'node:module';
import * as vscode from 'vscode';
import { WikiEditorProvider } from '../../src/providers/WikiEditorProvider.js';
import { WikiBinaryManager } from '../../src/utils/wikiInstaller.js';

// esbuild treats ESM namespace imports as immutable, so the spawn spy cannot
// reassign `child_process.spawn` via an `import * as` binding. A CommonJS
// require returns the same live, mutable module object that `runWikiCommand`
// resolves at runtime, so patching its `spawn` is observed by production code.
const require = createRequire(__filename);
const childProcess = require('node:child_process') as typeof import('node:child_process');

async function waitFor(predicate: () => boolean, message: string, timeoutMs = 10000): Promise<void> {
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    if (predicate()) return;
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  throw new assert.AssertionError({ message });
}

/**
 * Minimal real ExtensionContext: only the members the render/live-reload
 * path touches are populated (extensionUri + a Map-backed workspaceState).
 * The wiki-binary install path is never exercised by this test, so no
 * stubbing of process/network behaviour is required.
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

describe('WikiEditorProvider — no-spawn render path', () => {
  afterEach(async () => {
    await vscode.commands.executeCommand('workbench.action.closeAllEditors');
  });

  it('renders + live-reloads with no wiki summary/refs spawn and an in-process title', async function () {
    this.timeout(45000);

    const workspaceFolder = vscode.workspace.workspaceFolders?.[0];
    assert.ok(workspaceFolder, 'Expected test workspace folder');

    const ext = vscode.extensions.getExtension('goodfoot.wiki-extension');
    assert.ok(ext, 'Expected wiki extension to be discoverable');
    if (!ext.isActive) await ext.activate();

    const wikiDir = vscode.Uri.joinPath(workspaceFolder.uri, 'wiki');
    const wikiFile = vscode.Uri.joinPath(wikiDir, 'no-spawn.md');
    const frontmatterTitle = 'No Spawn Page';
    const fixture =
      `---\ntitle: ${frontmatterTitle}\n` +
      `summary: A fixture wiki page for the no-spawn regression test.\n---\n\n# Body Heading\n`;

    await vscode.workspace.fs.createDirectory(wikiDir);
    await vscode.workspace.fs.writeFile(wikiFile, Buffer.from(fixture));

    // ------------------------------------------------------------------
    // Spy on the real spawn primitive runWikiCommand uses. Record every
    // argument vector so we can assert no `summary`/`refs` wiki subcommand
    // is ever spawned by the render path.
    // ------------------------------------------------------------------
    const spawnedArgVectors: string[][] = [];
    const originalSpawn = childProcess.spawn;
    (childProcess as { spawn: typeof childProcess.spawn }).spawn = function patchedSpawn(
      this: unknown,
      ...args: Parameters<typeof childProcess.spawn>
    ): ReturnType<typeof childProcess.spawn> {
      spawnedArgVectors.push(Array.isArray(args[1]) ? (args[1] as string[]) : []);
      return (originalSpawn as (...a: unknown[]) => unknown).apply(this, args) as ReturnType<typeof childProcess.spawn>;
    } as typeof childProcess.spawn;

    const wikiSubcommandSpawned = (): boolean =>
      spawnedArgVectors.some((argv) => argv.includes('summary') || argv.includes('refs'));

    const context = makeContext(ext.extensionUri);
    const provider = new WikiEditorProvider(ext.extensionUri, new WikiBinaryManager(context), context);

    const document = await vscode.workspace.openTextDocument(wikiFile);
    const panel = vscode.window.createWebviewPanel('wiki.viewer.test', 'initial', vscode.ViewColumn.One, {
      enableScripts: true
    });

    try {
      const tokenSource = new vscode.CancellationTokenSource();
      await provider.resolveCustomTextEditor(document, panel, tokenSource.token);

      // ----------------------------------------------------------------
      // Act: edit the open document to trigger the live-reload path
      // (onDidChangeTextDocument -> _renderPage), which derives the title
      // in-process and sets panel.title without any subprocess.
      // ----------------------------------------------------------------
      const edit = new vscode.WorkspaceEdit();
      edit.insert(wikiFile, new vscode.Position(document.lineCount, 0), '\nAppended on live reload.\n');
      assert.ok(await vscode.workspace.applyEdit(edit), 'Expected the live-reload edit to apply');

      await waitFor(
        () => panel.title === frontmatterTitle,
        `Expected webviewPanel.title to equal frontmatter title "${frontmatterTitle}", got "${panel.title}"`
      );

      assert.strictEqual(
        wikiSubcommandSpawned(),
        false,
        `Expected no wiki summary/refs spawn on render + live-reload; spawned: ${JSON.stringify(spawnedArgVectors)}`
      );
    } finally {
      (childProcess as { spawn: typeof childProcess.spawn }).spawn = originalSpawn;
      panel.dispose();
    }
  });
});
