/**
 * Reproduction test: onDidReceiveMessage async handler lacks try/catch wrapper.
 *
 * ## What this test covers
 * The onDidReceiveMessage callback at WikiEditorProvider.ts:139-219 is an async
 * function whose switch-case body is NOT wrapped in try/catch. Message types
 * `ready` (line 145), `navigate` (line 168), `requestFileInfo` (line 173),
 * `openInEditor` (line 179), and `openSearch` (line 205) all `await` operations
 * that can reject. VS Code does not await webview message listener return
 * values, so rejections become unhandled promise rejections.
 *
 * ## How this test reproduces the bug
 * 1. Reads the source file WikiEditorProvider.ts.
 * 2. Locates the onDidReceiveMessage handler registration line.
 * 3. Examines the first statement of the handler body (the next line).
 * 4. Asserts the body begins with `try {` to catch rejections from all
 *    awaited switch-case operations.
 *
 * Against the UNFIXED code, the body starts with `switch (message.type) {`,
 * so this assertion FAILS — proving the structural bug exists.
 *
 * After the fix wraps the handler body in try/catch, the assertion PASSES.
 *
 * @summary Structural reproduction: onDidReceiveMessage handler lacks try/catch.
 * @module test/suite/wikiEditorProvider.unhandledRejection.test
 */

import * as assert from 'node:assert';
import * as fs from 'node:fs';
import * as path from 'node:path';

describe('WikiEditorProvider — async message handler unhandled rejections', () => {
  it('onDidReceiveMessage handler body should be wrapped in try/catch to prevent unhandled rejections', () => {
    // __dirname in the compiled CJS output resolves to:
    //   {EXTENSION_ROOT}/dist-test-{PID}/test/suite/
    // The source TypeScript file lives at:
    //   {EXTENSION_ROOT}/src/providers/WikiEditorProvider.ts
    const sourcePath = path.resolve(__dirname, '../../../src/providers/WikiEditorProvider.ts');

    assert.ok(fs.existsSync(sourcePath), `Source file not found at: ${sourcePath}`);

    const source = fs.readFileSync(sourcePath, 'utf-8');
    const lines = source.split('\n');

    // Locate the onDidReceiveMessage handler registration line.
    // The exact line is:
    //   const messageDisposable = webviewPanel.webview.onDidReceiveMessage(async (message: WebviewMessage) => {
    const targetSignature = '.onDidReceiveMessage(async (message: WebviewMessage) => {';
    let handlerLineIndex = -1;
    for (let i = 0; i < lines.length; i++) {
      if (lines[i]?.includes(targetSignature)) {
        handlerLineIndex = i;
        break;
      }
    }

    assert.ok(
      handlerLineIndex >= 0,
      `Could not locate onDidReceiveMessage handler in ${sourcePath}. ` +
        `Searched for line containing: "${targetSignature}"`
    );

    // The next line (handlerLineIndex + 1) is the first statement of the
    // arrow function body. Against the unfixed code, it begins with
    // `switch (message.type) {`. After the fix, it should begin with
    // `try {`.
    const bodyStartLine = lines[handlerLineIndex + 1];
    assert.ok(bodyStartLine != null, `Expected handler body at line ${handlerLineIndex + 2} but line is undefined`);

    const trimmed = bodyStartLine.trimStart();
    assert.ok(
      trimmed.startsWith('try {'),
      `BUG REPRODUCED: onDidReceiveMessage handler at ` +
        `WikiEditorProvider.ts:${handlerLineIndex + 2} lacks a try/catch wrapper.\n` +
        `First line of handler body: "${trimmed}"\n` +
        `Expected the body to start with \`try {\` to catch rejections from ` +
        `awaited operations in the 'ready', 'navigate', 'requestFileInfo', ` +
        `'openInEditor', and 'openSearch' cases.\n` +
        `VS Code does not await webview message listener return values, so ` +
        `these rejections propagate as unhandled promise rejections.`
    );
  });
});
