/**
 * Integration tests for WikiEditorProvider.
 *
 * Verifies extension activation and custom editor command registration using the
 * VS Code extension API. Tests that require a file on disk (e.g., opening a .md
 * file in the custom editor) are not included here — that interaction requires
 * more complex workspace setup and is covered by manual EDH verification.
 *
 * @summary WikiEditorProvider registration and activation tests.
 * @module test/suite/wikiEditorProvider.test
 */

import * as assert from 'node:assert';
import * as vscode from 'vscode';

describe('WikiEditorProvider', () => {
  it('extension activates successfully', async () => {
    const ext = vscode.extensions.getExtension('goodfoot.wiki-extension');
    assert.ok(ext, 'Extension not found — is publisher.name "goodfoot.wiki-extension"?');
    if (!ext.isActive) {
      await ext.activate();
    }
    assert.ok(ext.isActive, 'Extension did not activate');
  });

  it('wiki.openInEditor command is registered', async () => {
    const commands = await vscode.commands.getCommands();
    assert.ok(commands.includes('wiki.openInEditor'), 'wiki.openInEditor not registered');
  });
});
