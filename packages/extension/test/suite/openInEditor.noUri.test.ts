/**
 * Reproduction test: wiki.openInEditor without URI argument.
 *
 * Verifies that wiki.openInEditor handles the case where no URI is provided
 * (e.g., when invoked from the Command Palette) without throwing.
 *
 * The current implementation (extension.ts:128) receives `undefined` as its `uri`
 * parameter when the command is invoked with no arguments. It passes that
 * `undefined` directly to vscode.window.showTextDocument(), which rejects.
 *
 * This test asserts that the command resolves gracefully even without a URI
 * argument — it will FAIL against the current unfixed code, serving as a
 * red-phase TDD reproduction test.
 *
 * @summary wiki.openInEditor no-URI guard test.
 * @module test/suite/openInEditor.noUri.test
 */

import * as assert from 'node:assert';
import * as vscode from 'vscode';

describe('wiki.openInEditor', () => {
  it('should not reject when invoked without a URI argument (from Command Palette)', async () => {
    // The Command Palette passes no arguments to executeCommand.
    // Expected: command resolves gracefully (active editor fallback or no-op).
    // Actual (unfixed): showTextDocument(undefined, ...) rejects.
    let rejected = false;
    try {
      await vscode.commands.executeCommand('wiki.openInEditor');
    } catch {
      rejected = true;
    }
    assert.ok(!rejected, 'wiki.openInEditor should not reject when invoked without a URI');
  });
});
