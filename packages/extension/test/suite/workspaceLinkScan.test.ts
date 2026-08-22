/**
 * Characterization tests for the workspace link scanner shared by Find
 * References (`_findIncomingLinks`) and single-file rename
 * (`buildFileMoveEdit`) in {@link WikiLanguageFeatures}.
 *
 * These pin CURRENT behavior — sequential reads, per-line regex construction,
 * exact output ordering and ranges — so the batched-concurrency rewrite can be
 * verified as output-equivalent. Assertions inspect the returned
 * `WorkspaceEdit` / `Location[]` values directly, so no editor buffers need to
 * be opened.
 *
 * @summary Pins current output of incoming-link scanning and file-move editing.
 * @module test/suite/workspaceLinkScan.test
 */

import * as assert from 'node:assert';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import * as vscode from 'vscode';
import { WikiLanguageFeatures } from '../../src/providers/WikiLanguageFeatures.js';
import type { WikiBinaryManager } from '../../src/utils/wikiInstaller.js';

/** Minimal shape of the private incoming-link scanner used by these tests. */
type IncomingLinksAccessor = {
  _findIncomingLinks(targetAbsPath: string, token?: vscode.CancellationToken): Promise<vscode.Location[]>;
};

function wsRoot(): string {
  const folder = vscode.workspace.workspaceFolders?.[0];
  assert.ok(folder, 'Expected a workspace folder');
  return folder.uri.fsPath;
}

/**
 * Sort key so location comparisons are independent of scan order.
 *
 * @param loc - The location to key.
 * @returns A deterministic string key for the location.
 */
function locationKey(loc: vscode.Location): string {
  return `${loc.uri.fsPath}#${loc.range.start.line}:${loc.range.start.character}`;
}

describe('workspace link scan characterization', () => {
  const fxDirName = `scanfix-${Date.now()}`;
  let features: WikiLanguageFeatures;

  beforeEach(() => {
    features = new WikiLanguageFeatures(null as unknown as WikiBinaryManager);
  });

  afterEach(async () => {
    await vscode.commands.executeCommand('workbench.action.closeAllEditors');
    // Restore permissions in case the unreadable-file test left a 0o000 file.
    const locked = path.join(wsRoot(), fxDirName, 'unreadable', 'locked.md');
    try {
      fs.chmodSync(locked, 0o644);
    } catch (_err) {
      void _err;
    }
    try {
      await vscode.workspace.fs.delete(vscode.Uri.file(path.join(wsRoot(), fxDirName)), {
        recursive: true,
        useTrash: false
      });
    } catch (_err) {
      void _err;
    }
  });

  /**
   * Fixture layout:
   *
   * - `target.md` — the reference target; also links to itself.
   * - `linker-a.md` — one line carrying a plain link, an image (must be
   *   skipped by the `(?<!!)` lookbehind), a fragment-carrying link to the
   *   same target, and an external https link (unresolvable).
   * - `sub/linker-b.md` — parent-relative link with a `"title"` suffix.
   *
   * @returns The fixture directory and the absolute target path.
   */
  async function writeFixtures(): Promise<{ fxDir: string; targetAbs: string }> {
    const root = wsRoot();
    const fxDir = path.join(root, fxDirName);
    const subDir = path.join(fxDir, 'sub');

    fs.mkdirSync(subDir, { recursive: true });

    const targetAbs = path.join(fxDir, 'target.md');
    fs.writeFileSync(targetAbs, ['# Target', '', 'Self ref: [here](./target.md)', ''].join('\n'), 'utf8');

    const linkerALine = '- [plain](target.md) ![pic](target.md) [frag](target.md#sec) [ext](https://example.com/x.md)';
    fs.writeFileSync(path.join(fxDir, 'linker-a.md'), ['# A', '', linkerALine, ''].join('\n'), 'utf8');

    fs.writeFileSync(
      path.join(subDir, 'linker-b.md'),
      ['# B', '', 'Up: [t](../target.md "doc")', ''].join('\n'),
      'utf8'
    );

    // Give VS Code's file watcher a moment to notice the new files.
    await new Promise((resolve) => setTimeout(resolve, 300));
    return { fxDir, targetAbs };
  }

  it('_findIncomingLinks reports exact locations for every incoming link', async () => {
    const { fxDir, targetAbs } = await writeFixtures();

    const accessor = features as unknown as IncomingLinksAccessor;
    const locations = await accessor._findIncomingLinks(targetAbs);
    assert.ok(locations.length > 0, 'expected at least one incoming link');

    const linkerAPath = path.join(fxDir, 'linker-a.md');
    const linkerADocText = fs.readFileSync(linkerAPath, 'utf8');
    const linkerALineIdx = 2;
    const linkerALine = linkerADocText.split('\n')[linkerALineIdx]!;

    const plainStart = linkerALine.indexOf('target.md');
    const fragStart = linkerALine.indexOf('target.md#sec');

    const linkerBPath = path.join(fxDir, 'sub', 'linker-b.md');
    const linkerBLine = fs.readFileSync(linkerBPath, 'utf8').split('\n')[2]!;
    const upStart = linkerBLine.indexOf('../target.md');

    const targetDocText = fs.readFileSync(targetAbs, 'utf8');
    const targetLine = targetDocText.split('\n')[2]!;
    const selfStart = targetLine.indexOf('./target.md');

    const expected: Array<[string, number, number, number, number]> = [
      // Self link inside the target file.
      [targetAbs, 2, selfStart, 2, selfStart + './target.md'.length],
      // Plain link — image occurrence is skipped by the lookbehind.
      [linkerAPath, linkerALineIdx, plainStart, linkerALineIdx, plainStart + 'target.md'.length],
      // Fragment-carrying link resolves to the same target; range spans the
      // full href including "#sec".
      [linkerAPath, linkerALineIdx, fragStart, linkerALineIdx, fragStart + 'target.md#sec'.length],
      // Parent-relative link with title suffix.
      [linkerBPath, 2, upStart, 2, upStart + '../target.md'.length]
    ];

    const byPosition = (
      a: [string, number, number, number, number],
      b: [string, number, number, number, number]
    ): number => a[0].localeCompare(b[0]) || a[1] - b[1] || a[2] - b[2];

    const actual = locations.map((loc): [string, number, number, number, number] => [
      loc.uri.fsPath,
      loc.range.start.line,
      loc.range.start.character,
      loc.range.end.line,
      loc.range.end.character
    ]);
    const expectedSorted = [...expected].sort(byPosition);
    actual.sort(byPosition);

    assert.strictEqual(locations.length, expected.length, `raw count mismatch: ${JSON.stringify(actual)}`);
    assert.deepStrictEqual(actual, expectedSorted, 'location sets differ');
    assert.ok(new Set(locations.map(locationKey)).size === locations.length, 'duplicate locations emitted');
    assert.ok(linkerALine.includes('![pic](target.md)'), 'fixture sanity: image link present on scanned line');
  });

  it('buildFileMoveEdit rewrites exactly the incoming links and preserves fragments', async () => {
    const { fxDir, targetAbs } = await writeFixtures();
    const renamedAbs = path.join(fxDir, 'renamed.md');

    const edit = await features.buildFileMoveEdit(targetAbs, renamedAbs);

    const entries = edit.entries();
    const byUri = new Map(entries.map(([uri, edits]) => [uri.fsPath, edits]));
    assert.strictEqual(entries.length, 3, `expected edits in 3 files, got ${entries.length}`);

    // Apply the edits to the fixture text ourselves and compare exactly.
    const applyEdits = (text: string, edits: readonly vscode.TextEdit[]): string => {
      const sorted = [...edits].sort((x, y) => y.range.start.character - x.range.start.character);
      const lines = text.split('\n');
      for (const e of sorted) {
        const line = lines[e.range.start.line]!;
        lines[e.range.start.line] =
          line.slice(0, e.range.start.character) + e.newText + line.slice(e.range.end.character);
      }
      return lines.join('\n');
    };

    const linkerAPath = path.join(fxDir, 'linker-a.md');
    const linkerAResult = applyEdits(fs.readFileSync(linkerAPath, 'utf8'), byUri.get(linkerAPath)!);
    assert.strictEqual(
      linkerAResult.split('\n')[2],
      '- [plain](renamed.md) ![pic](target.md) [frag](renamed.md#sec) [ext](https://example.com/x.md)',
      'linker-a: plain + fragment rewritten, image + external untouched'
    );

    const linkerBPath = path.join(fxDir, 'sub', 'linker-b.md');
    const linkerBResult = applyEdits(fs.readFileSync(linkerBPath, 'utf8'), byUri.get(linkerBPath)!);
    assert.strictEqual(
      linkerBResult.split('\n')[2],
      'Up: [t](../renamed.md "doc")',
      'linker-b: parent-relative href recomputed, title suffix kept outside href'
    );

    const targetResult = applyEdits(fs.readFileSync(targetAbs, 'utf8'), byUri.get(targetAbs)!);
    assert.strictEqual(
      targetResult.split('\n')[2],
      'Self ref: [here](renamed.md)',
      'target self link rewritten without "./" prefix'
    );
  });

  it('buildFileMoveEdit keeps the href text when the new path equals the linker directory', async () => {
    const root = wsRoot();
    const fxDir = path.join(root, fxDirName, 'fallback');
    fs.mkdirSync(fxDir, { recursive: true });

    const linkerPath = path.join(fxDir, 'fall.md');
    fs.writeFileSync(linkerPath, '# F\n\n[x](somefile.md)\n', 'utf8');
    await new Promise((resolve) => setTimeout(resolve, 300));

    // New path is the linker's own directory: path.relative computes "" and
    // the `|| rawPath` fallback keeps the original href text.
    const edit = await features.buildFileMoveEdit(path.join(fxDir, 'somefile.md'), fxDir);

    const entries = edit.entries();
    assert.strictEqual(entries.length, 1, `expected one edited file, got ${entries.length}`);
    const [, edits] = entries[0]!;
    assert.strictEqual(edits.length, 1);
    assert.strictEqual(edits[0]!.newText, 'somefile.md', 'fallback must preserve the raw path text');
  });

  const canTestPermissions = os.platform() === 'linux' && process.getuid?.() !== 0;
  (canTestPermissions ? it : it.skip)('skips files that cannot be read', async () => {
    const { fxDir, targetAbs } = await writeFixtures();

    const unreadableDir = path.join(fxDir, 'unreadable');
    fs.mkdirSync(unreadableDir, { recursive: true });
    const lockedPath = path.join(unreadableDir, 'locked.md');
    fs.writeFileSync(lockedPath, '[broken](./target.md)\n', 'utf8');
    fs.chmodSync(lockedPath, 0o000);

    try {
      await new Promise((resolve) => setTimeout(resolve, 300));

      const accessor = features as unknown as IncomingLinksAccessor;
      const locations = await accessor._findIncomingLinks(targetAbs);
      for (const loc of locations) {
        assert.notStrictEqual(loc.uri.fsPath, lockedPath, 'unreadable file contributed results');
      }

      const renamedAbs = path.join(fxDir, 'renamed.md');
      const edit = await features.buildFileMoveEdit(targetAbs, renamedAbs);
      for (const [uri] of edit.entries()) {
        assert.notStrictEqual(uri.fsPath, lockedPath, 'unreadable file produced edits');
      }
    } finally {
      fs.chmodSync(lockedPath, 0o644);
    }
  });

  describe('cancellation', () => {
    beforeEach(() => {
      features = new WikiLanguageFeatures(null as unknown as WikiBinaryManager);
    });

    afterEach(async () => {
      await vscode.commands.executeCommand('workbench.action.closeAllEditors');
      try {
        await vscode.workspace.fs.delete(vscode.Uri.file(path.join(wsRoot(), fxDirName)), {
          recursive: true,
          useTrash: false
        });
      } catch (_err) {
        void _err;
      }
    });

    function writeCancellationFixture(): string {
      const root = wsRoot();
      const fxDir = path.join(root, fxDirName);
      fs.mkdirSync(fxDir, { recursive: true });
      fs.writeFileSync(path.join(fxDir, 'target.md'), '# T\n\n', 'utf8');
      fs.writeFileSync(path.join(fxDir, 'linker.md'), '[a](./target.md)\n[b](./target.md#f)\n', 'utf8');
      return path.join(fxDir, 'target.md');
    }

    it('find references returns nothing when the token is already cancelled', async () => {
      const targetAbs = writeCancellationFixture();
      await new Promise((resolve) => setTimeout(resolve, 300));

      const source = new vscode.CancellationTokenSource();
      source.cancel();

      const accessor = features as unknown as IncomingLinksAccessor;
      // The fixture has two incoming links; a cancelled scan must not report them.
      const locations = await accessor._findIncomingLinks(targetAbs, source.token);
      assert.deepStrictEqual(locations, []);
    });

    it('file-move rename rejects with CancellationError instead of returning a partial edit', async () => {
      const fxDir = path.join(wsRoot(), fxDirName);
      fs.mkdirSync(fxDir, { recursive: true });
      fs.writeFileSync(path.join(fxDir, 'target.md'), '# T\n\n', 'utf8');
      fs.writeFileSync(path.join(fxDir, 'linker.md'), '[a](./target.md)\n', 'utf8');
      await new Promise((resolve) => setTimeout(resolve, 300));

      const source = new vscode.CancellationTokenSource();
      source.cancel();

      await assert.rejects(
        features.buildFileMoveEdit(path.join(fxDir, 'target.md'), path.join(fxDir, 'renamed.md'), source.token),
        vscode.CancellationError
      );
    });

    // Sized from the recorded scanner benchmark (10 MB ≈ 170–270 ms): 40
    // files × 2 MB of link-dense text keeps a warm scan well above one
    // second, so a cancel fired a few milliseconds into the scan lands
    // mid-scan while batches are still resolving. Links are packed many per
    // line so the byte/link throughput matches the benchmark while keeping
    // split('\n') arrays (and therefore batch-read memory churn) small.
    const BULK_FILE_COUNT = 40;
    const BULK_TARGET_BYTES_PER_FILE = 2 * 1024 * 1024;
    /** One target-linking line every N filler lines keeps the partial array light but non-empty. */
    const BULK_TARGET_EVERY = 512;
    /** Decoy links packed onto each filler line. */
    const BULK_LINKS_PER_LINE = 30;

    /**
     * Build a ~2 MB link-dense corpus file: most lines carry decoy links
     * (`o.md`); every {@link BULK_TARGET_EVERY}-th line adds one link to
     * `target.md`.
     *
     * @returns A ~2 MB string shared by all bulk fixture files.
     */
    function bulkFileText(): string {
      const fillerJoin = Array<string>(BULK_LINKS_PER_LINE).fill('[n](o.md)').join(' ');
      const fillerLine = `${fillerJoin}\n`;
      const targetLine = `- [t](target.md) ${Array<string>(BULK_LINKS_PER_LINE - 1)
        .fill('[n](o.md)')
        .join(' ')}\n`;
      const cycle = fillerLine.repeat(BULK_TARGET_EVERY - 1) + targetLine;
      const cycles = Math.ceil(BULK_TARGET_BYTES_PER_FILE / cycle.length);
      return `# Bulk\n\n${cycle.repeat(cycles)}`;
    }

    it('mid-scan cancellation discards partial references instead of returning them', async function () {
      // Fixture write + warm scan + cancelled scan + teardown of ~80 MB.
      this.timeout(120_000);

      const fxDir = path.join(wsRoot(), fxDirName, 'bulk');
      fs.mkdirSync(fxDir, { recursive: true });
      fs.writeFileSync(path.join(fxDir, 'target.md'), '# Target\n', 'utf8');
      const bulkText = bulkFileText();
      for (let i = 0; i < BULK_FILE_COUNT; i++) {
        fs.writeFileSync(path.join(fxDir, `bulk-${i}.md`), bulkText, 'utf8');
      }
      const targetAbs = path.join(fxDir, 'target.md');
      // Give VS Code's file watcher a moment to notice the new files.
      await new Promise((resolve) => setTimeout(resolve, 300));

      const accessor = features as unknown as IncomingLinksAccessor;

      // Warm full scan first: proves the corpus produces incoming links when
      // uncancelled and measures the scan-duration baseline for the margin
      // guards below.
      const warmStart = Date.now();
      const fullLocations = await accessor._findIncomingLinks(targetAbs);
      const warmMs = Date.now() - warmStart;
      assert.ok(fullLocations.length > 0, 'fixture sanity: bulk corpus produced incoming links');
      // Corpus guard — well under the recorded ~1.4 s floor for this corpus
      // size; if a machine ever gets near it, the corpus no longer guarantees
      // a mid-scan cancel and must be enlarged.
      assert.ok(warmMs >= 500, `bulk corpus scanned too fast (${warmMs}ms) to guarantee a mid-scan cancel`);

      // Warm the search service and confirm enumeration sees the corpus; its
      // measured latency sets the cancel-delay floor below.
      const enumStart = Date.now();
      const enumerated = await vscode.workspace.findFiles('**/*.md', '**/node_modules/**');
      const enumerateMs = Date.now() - enumStart;
      assert.ok(
        enumerated.length >= BULK_FILE_COUNT + 1,
        `fixture sanity: enumeration found only ${enumerated.length} markdown files`
      );

      // Explicit margins, tightened on fast machines rather than weakening
      // the assertion below: the delay must outlast enumeration (a cancel
      // landing before the first batch would collect nothing and mask a
      // fail-open regression as a legitimate empty result) while staying a
      // small fraction of the warm scan duration.
      const cancelDelayMs = Math.max(25, Math.min(150, Math.ceil(enumerateMs * 3)));

      const source = new vscode.CancellationTokenSource();
      try {
        const pending = accessor._findIncomingLinks(targetAbs, source.token);
        setTimeout(() => source.cancel(), cancelDelayMs);

        // A cancelled scan must resolve to [] — never a partial subset.
        // assert.fail first keeps the leak count readable: deepStrictEqual
        // attaches both Location arrays to the error, which overflows the
        // extension host console.
        const locations = await pending;
        if (locations.length > 0) {
          assert.fail(`cancelled scan leaked ${locations.length} partial references`);
        }
        assert.deepStrictEqual(locations, []);
      } finally {
        source.dispose();
      }
    });
  });
});

describe('workspace link scanner source structure', () => {
  // __dirname in the compiled CJS output resolves to
  // {EXTENSION_ROOT}/dist-test-{PID}/test/suite/, so three levels up is the
  // extension package root — same layout assumption as the renameHandler
  // precedent test.
  const srcPath = path.resolve(__dirname, '../../../src/providers/WikiLanguageFeatures.ts');

  /**
   * Extract a method body by balanced-brace counting from its declaration.
   *
   * Assumes the declaration head contains no `{` before the body opener and
   * that the body itself contains no brace-carrying template literals — both
   * hold for the scanner methods this suite inspects. Same technique as the
   * `onDidRenameFiles` precedent in renameHandler.unhandledRejection.test.ts.
   *
   * @param source - Full file text to search.
   * @param headRe - Regex matching the method declaration head.
   * @returns The text between the body's opening and closing braces.
   */
  function extractBody(source: string, headRe: RegExp): string {
    const head = source.match(headRe);
    assert.ok(head != null, `could not locate ${headRe} in WikiLanguageFeatures.ts — update this test`);
    let openIdx = -1;
    for (let i = head.index! + head[0].length; i < source.length; i++) {
      if (source[i] === '{') {
        openIdx = i;
        break;
      }
    }
    assert.notStrictEqual(openIdx, -1, 'no opening brace found after declaration head');
    let depth = 0;
    for (let i = openIdx; i < source.length; i++) {
      if (source[i] === '{') depth++;
      if (source[i] === '}') {
        depth--;
        if (depth === 0) return source.slice(openIdx + 1, i);
      }
    }
    assert.fail('unbalanced braces: could not find method body end');
  }

  /**
   * Extract a full call expression by paren balancing.
   *
   * @param source - Text to search within.
   * @param nameRe - Regex matching the callee name including its opening
   *                 parenthesis (e.g. `/findFiles\(/`).
   * @returns The complete call expression including arguments, or null.
   */
  function extractCall(source: string, nameRe: RegExp): string | null {
    const m = source.match(nameRe);
    if (m == null) return null;
    // The match already consumed the opening paren, so balancing starts one
    // level deep and closes when depth returns to zero.
    let depth = 1;
    for (let i = m.index! + m[0].length; i < source.length; i++) {
      const ch = source[i];
      if (ch === '(') depth++;
      if (ch === ')') {
        depth--;
        if (depth === 0) return source.slice(m.index!, i + 1);
      }
    }
    return null;
  }

  /**
   * Split a call's argument list on top-level commas only (quote- and
   * nesting-aware), so multi-line argument formatting does not matter.
   *
   * @param callText - A full call expression, e.g. `findFiles('a', token)`.
   * @returns Trimmed top-level argument texts.
   */
  function topLevelArgs(callText: string): string[] {
    const inner = callText.slice(callText.indexOf('(') + 1, callText.lastIndexOf(')'));
    const args: string[] = [];
    let depth = 0;
    let quote: string | null = null;
    let current = '';
    for (const ch of inner) {
      if (quote != null) {
        current += ch;
        if (ch === quote) quote = null;
        continue;
      }
      if (ch === "'" || ch === '"' || ch === '`') {
        quote = ch;
        current += ch;
        continue;
      }
      if ('([{'.includes(ch)) depth++;
      if (')]}'.includes(ch)) depth--;
      if (ch === ',' && depth === 0) {
        args.push(current.trim());
        current = '';
        continue;
      }
      current += ch;
    }
    if (current.trim().length > 0) args.push(current.trim());
    return args;
  }

  it('wires cancellation through enumeration into findFiles and bails before any read', () => {
    const source = fs.readFileSync(srcPath, 'utf8');

    // --- _allMarkdownFiles must forward its token into findFiles ---
    const enumHead = source.match(/private async _allMarkdownFiles\(\s*(\w+)\?\s*:\s*vscode\.CancellationToken/);
    assert.ok(enumHead != null, '_allMarkdownFiles must declare an optional CancellationToken parameter');
    const enumTokenName = enumHead[1]!;

    const enumBody = extractBody(source, /private async _allMarkdownFiles\(/);
    const findCall = extractCall(enumBody, /findFiles\(/);
    assert.ok(findCall != null, '_allMarkdownFiles must call vscode.workspace.findFiles');
    const findArgs = topLevelArgs(findCall);
    assert.ok(
      findArgs.length >= 4,
      `findFiles must pass include, exclude, maxResults and a token — got ${findArgs.length} arguments`
    );
    assert.strictEqual(
      findArgs[findArgs.length - 1],
      enumTokenName,
      `the last findFiles argument must be the ${enumTokenName} parameter`
    );

    // --- _scanWorkspaceMarkdown must pass its token and bail right after ---
    const scanHead = source.match(/private async _scanWorkspaceMarkdown\(\s*(\w+)\?\s*:\s*vscode\.CancellationToken/);
    assert.ok(scanHead != null, '_scanWorkspaceMarkdown must declare an optional CancellationToken parameter');
    const scanTokenName = scanHead[1]!;

    const scanBody = extractBody(source, /private async _scanWorkspaceMarkdown\(/);
    const mdCall = extractCall(scanBody, /this\._allMarkdownFiles\(/);
    assert.ok(mdCall != null, '_scanWorkspaceMarkdown must call _allMarkdownFiles');
    const mdArgs = topLevelArgs(mdCall);
    assert.ok(
      mdArgs.includes(scanTokenName),
      `_scanWorkspaceMarkdown must forward its ${scanTokenName} into _allMarkdownFiles`
    );

    // The first cancellation check after enumeration must sit between the
    // awaited enumeration and the first readFile call, so no file is read
    // once cancellation has landed during enumeration.
    const callStart = scanBody.indexOf(mdCall!);
    const readStart = scanBody.indexOf('readFile(');
    assert.notStrictEqual(readStart, -1, 'readFile call not found in _scanWorkspaceMarkdown');
    const betweenEnumAndRead = scanBody.slice(callStart + mdCall!.length, readStart);
    assert.match(
      betweenEnumAndRead,
      new RegExp(`${scanTokenName}\\??\\.isCancellationRequested`),
      'cancellation must be checked directly after awaiting _allMarkdownFiles, before any file read'
    );

    // --- Consumers must fail closed on a cancelled corpus ---
    const refsBody = extractBody(source, /private async _findIncomingLinks\(/);
    assert.match(
      refsBody,
      /if\s*\(\s*scans\s*===\s*null\s*\)\s*return\s*\[\s*\]\s*;/,
      '_findIncomingLinks must return [] rather than partial locations when cancelled'
    );
    const moveBody = extractBody(source, /async buildFileMoveEdit\(/);
    assert.match(
      moveBody,
      /if\s*\(\s*scans\s*===\s*null\s*\)\s*throw new vscode\.CancellationError\(\)/,
      'buildFileMoveEdit must reject with CancellationError rather than yield a partial edit'
    );
  });
});
