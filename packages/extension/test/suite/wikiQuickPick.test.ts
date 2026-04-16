/**
 * Integration tests for the wiki QuickPick command.
 *
 * Verifies that wiki commands are registered and available via the VS Code
 * extension API after activation.
 *
 * @summary wikiQuickPick command registration tests.
 * @module test/suite/wikiQuickPick.test
 */

import * as assert from 'node:assert';
import * as vscode from 'vscode';

describe('wikiQuickPick', () => {
  it('wiki.search command is registered', async () => {
    const commands = await vscode.commands.getCommands();
    assert.ok(commands.includes('wiki.search'), 'wiki.search not registered');
  });

  it('wiki.openInEditor command is registered', async () => {
    const commands = await vscode.commands.getCommands();
    assert.ok(commands.includes('wiki.openInEditor'), 'wiki.openInEditor not registered');
  });

  it('wiki.retryInstall command is registered', async () => {
    const commands = await vscode.commands.getCommands();
    assert.ok(commands.includes('wiki.retryInstall'), 'wiki.retryInstall not registered');
  });
});
