/**
 * Reproduction test: the `onDidRenameFiles` async callback does not wrap its
 * awaited operations in try/catch, so rejections from `buildFileMoveEdit`,
 * `buildDirectoryMoveEdit`, or `applyEdit` become unhandled rejections.
 *
 * ## What this test covers
 * The handler at extension.ts L42-L83 is registered as an async callback to
 * `vscode.workspace.onDidRenameFiles`. VS Code does not await the return value
 * of event listeners. When an awaited operation inside the callback rejects,
 * the rejection propagates out of the async callback and becomes an unhandled
 * promise rejection.
 *
 * ## Why a structural test
 * The handler is an anonymous closure inside `activate()`. The event cannot be
 * triggered programmatically (`vscode.workspace.fs.rename` does not fire
 * `onDidRenameFiles` — confirmed by the directory rename test). The callback
 * function is not exported and has no accessible reference after registration.
 *
 * ## How this test reproduces the bug
 * 1. Read the extension.ts source file.
 * 2. Locate the `onDidRenameFiles(async (event) => {` handler.
 * 3. Count balanced braces to extract the handler body.
 * 4. Trim leading whitespace and extract the first word in the body.
 *
 * Against the UNFIXED code the body starts with `const aggregate = …`, so the
 * first word is `const`. The assertion `firstWord === 'try'` FAILS — proving
 * the handler lacks an outer try/catch.
 *
 * After the fix (wrapping the entire body in `try { … } catch (error) { … }`),
 * the first word is `try` and the assertion PASSES.
 *
 * @summary Reproduction: onDidRenameFiles handler lacks try/catch — unhandled rejections escape.
 * @module test/suite/renameHandler.unhandledRejection.test
 */

import * as assert from 'node:assert';
import * as fs from 'node:fs';
import * as path from 'node:path';

describe('onDidRenameFiles handler error handling', () => {
  it('wraps the async callback body in try/catch to prevent unhandled rejections', () => {
    // __dirname in the compiled CJS output resolves to:
    //   {EXTENSION_ROOT}/dist-test-{PID}/test/suite/
    // The source TypeScript file lives at:
    //   {EXTENSION_ROOT}/src/extension.ts
    const extPath = path.resolve(__dirname, '../../../src/extension.ts');
    const source = fs.readFileSync(extPath, 'utf8');

    // Locate the onDidRenameFiles handler via its function expression head.
    const handlerRe = /onDidRenameFiles\(async\s*\([^)]*\)\s*=>\s*\{/;
    const match = source.match(handlerRe);
    assert.ok(
      match != null,
      'Could not locate onDidRenameFiles(async (event) => { handler in extension.ts.\n' +
        'The handler signature may have changed — update this test.'
    );

    // Position of the `{` that opens the handler body.
    const braceStart = match.index! + match[0].length - 1;

    // Count balanced braces to find the matching closing `}`.
    let depth = 0;
    let braceEnd = -1;
    for (let i = braceStart; i < source.length; i++) {
      if (source[i] === '{') depth++;
      if (source[i] === '}') {
        depth--;
        if (depth === 0) {
          braceEnd = i;
          break;
        }
      }
    }
    assert.notStrictEqual(
      braceEnd,
      -1,
      'Could not find matching closing brace for onDidRenameFiles handler body.\n' +
        'The handler structure may have changed — update this test.'
    );

    // Extract the handler body (content between outer `{` and `}`).
    const handlerBody = source.slice(braceStart + 1, braceEnd);

    // The first meaningful word in the handler body.
    // Against the UNFIXED code the handler starts with:
    //   const aggregate = new vscode.WorkspaceEdit();
    // so `firstWord` is `const`.  The assertion `firstWord === 'try'` FAILS.
    //
    // After the fix the body starts with:
    //   try {
    //     const aggregate = new vscode.WorkspaceEdit();
    //     ...
    //   } catch (error) {
    //     ...
    //   }
    // so `firstWord` is `try`.  The assertion PASSES.
    const firstWord = handlerBody.trim().match(/^\w+/)?.[0];

    assert.strictEqual(
      firstWord,
      'try',
      'BUG REPRODUCED: onDidRenameFiles async callback at extension.ts L42-L83 does not ' +
        'wrap its body in try/catch.\n\n' +
        'The handler awaits buildFileMoveEdit (line 64), buildDirectoryMoveEdit (line 76), ' +
        'and applyEdit (line 81) without a surrounding try/catch. When any of these rejects, ' +
        'the rejection propagates out of the async callback and becomes an unhandled ' +
        'rejection because VS Code does not await the return value of event listeners.\n\n' +
        'Fix: wrap the handler body in try/catch and log the error via ' +
        'getWikiLogger().error() / formatLogError().\n\n' +
        `Expected first word in handler body to be 'try', got '${firstWord}'.`
    );
  });
});
