/**
 * Reproduction test: the webview CSP must not allow 'unsafe-inline' scripts.
 *
 * ## Bug
 * [[WikiEditorProvider.ts#L402](./packages/extension/src/providers/WikiEditorProvider.ts#L402)]
 * includes `'unsafe-inline'` in the `script-src` CSP directive, but the shell
 * HTML contains no inline script — only an external module
 * (`<script type="module" src="...">`). The `'unsafe-inline'` keyword discards
 * defense-in-depth for free.
 *
 * ## What this test covers
 * `_buildShellHtml()` generates the webview shell HTML including the CSP meta
 * tag. This test extracts the `script-src` value from the generated HTML and
 * asserts that `'unsafe-inline'` is absent.
 *
 * @summary Reproduction test: CSP script-src must not allow unsafe-inline.
 * @module test/suite/wikiEditorProvider.cspScriptSrc.test
 */

import * as assert from 'node:assert';
import * as vscode from 'vscode';
import { WikiEditorProvider } from '../../src/providers/WikiEditorProvider.js';
import { WikiBinaryManager } from '../../src/utils/wikiInstaller.js';

describe('WikiEditorProvider — CSP script-src', () => {
  afterEach(async () => {
    await vscode.commands.executeCommand('workbench.action.closeAllEditors');
  });

  it('script-src does not allow unsafe-inline', async function () {
    this.timeout(15000);

    const ext = vscode.extensions.getExtension('goodfoot.wiki-extension');
    assert.ok(ext, 'Expected wiki extension to be discoverable');
    if (!ext.isActive) await ext.activate();

    // Minimal context: only extensionUri is needed for localResourceRoots.
    const store = new Map<string, unknown>();
    const context: vscode.ExtensionContext = {
      extensionUri: ext.extensionUri,
      workspaceState: {
        keys: () => [...store.keys()],
        get: <T>(key: string, defaultValue?: T): T | undefined =>
          store.has(key) ? (store.get(key) as T) : defaultValue,
        update: (key: string, value: unknown) => {
          store.set(key, value);
          return Promise.resolve();
        }
      }
    } as unknown as vscode.ExtensionContext;

    const provider = new WikiEditorProvider(ext.extensionUri, new WikiBinaryManager(context), context);

    const panel = vscode.window.createWebviewPanel('wiki.viewer.test-csp', 'CSP Test', vscode.ViewColumn.One, {
      enableScripts: true
    });

    try {
      const html: string = (provider as unknown as { _buildShellHtml: (w: vscode.Webview) => string })[
        '_buildShellHtml'
      ](panel.webview);

      // Extract the Content-Security-Policy meta tag's content attribute.
      const cspMatch = /http-equiv="Content-Security-Policy"\s+content="([^"]+)"/.exec(html);
      assert.ok(cspMatch, 'Expected CSP meta tag in shell HTML');

      const cspContent = cspMatch[1]!;

      // Extract the script-src directive value (text between "script-src " and the next ";").
      const scriptSrcMatch = /script-src\s+([^;]+)/.exec(cspContent);
      assert.ok(scriptSrcMatch, 'Expected script-src directive in CSP');

      const scriptSrcValue = scriptSrcMatch[1]!.trim();

      assert.ok(
        !scriptSrcValue.includes("'unsafe-inline'"),
        `script-src must not allow 'unsafe-inline', got: ${scriptSrcValue}`
      );
    } finally {
      panel.dispose();
    }
  });
});
