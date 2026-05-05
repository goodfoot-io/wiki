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
import { isNamespaceMode, toNamespaceQuickPickItem } from '../../src/commands/wikiQuickPick.js';

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

  describe('wiki namespaces', () => {
    it('isNamespaceMode detects @-prefix without space', () => {
      assert.strictEqual(isNamespaceMode('@'), true);
      assert.strictEqual(isNamespaceMode('@marketing'), true);
      assert.strictEqual(isNamespaceMode('@eng'), true);
      assert.strictEqual(isNamespaceMode(''), false);
      assert.strictEqual(isNamespaceMode('marketing'), false);
      assert.strictEqual(isNamespaceMode('@marketing '), false);
      assert.strictEqual(isNamespaceMode('@eng design'), false);
      assert.strictEqual(isNamespaceMode('@ '), false);
    });

    it('converts namespace entry to quick pick item', () => {
      const entry = { namespace: 'engineering', path: 'wiki/engineering', abs_path: '/repo/wiki/engineering' };
      const item = toNamespaceQuickPickItem(entry);
      assert.strictEqual(item.label, 'engineering');
      assert.strictEqual(item.detail, 'wiki/engineering');
      assert.strictEqual(item.file, '/repo/wiki/engineering');
      assert.strictEqual(item.alwaysShow, true);
    });

    it('filters namespace items case-insensitively by substring', () => {
      const items = [
        { label: 'Engineering', detail: '/e', file: '/e' },
        { label: 'Marketing', detail: '/m', file: '/m' },
        { label: 'Design', detail: '/d', file: '/d' }
      ];
      const filtered = items.filter((item) => item.label.toLowerCase().includes('eng'.toLowerCase()));
      assert.strictEqual(filtered.length, 1);
      assert.strictEqual(filtered[0]!.label, 'Engineering');
    });

    it('empty filter shows all namespace items', () => {
      const items = [
        { label: 'Engineering', detail: '/e', file: '/e' },
        { label: 'Marketing', detail: '/m', file: '/m' }
      ];
      const filtered = items.filter((item) => item.label.toLowerCase().includes(''));
      assert.strictEqual(filtered.length, 2);
    });
  });
});
