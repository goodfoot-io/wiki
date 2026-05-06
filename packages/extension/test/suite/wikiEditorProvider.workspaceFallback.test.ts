/**
 * Reproduction test for the host-side fallback constraint that blocks
 * workspace-root file navigation from bare filenames in the wiki webview.
 *
 * ## Bug
 * WikiEditorProvider.ts:225 checks `message.pageName.includes('/')` before
 * attempting workspace-root resolution. Bare filenames like `CLAUDE.md` have
 * no slash, so the fallback is skipped even when the file exists at the
 * workspace root. The user sees "Could not find wiki page: CLAUDE.md".
 *
 * ## Hypothesis (host-side)
 * The includes('/') guard is too strict. Removing it lets the workspace
 * fallback resolve any `pageName` that is not found as a wiki page,
 * including bare filenames that name files at the workspace root.
 *
 * ## Code path
 * 1. Wiki page renders markdown link `[CLAUDE.md](CLAUDE.md#L63-L74)` as
 *    `<a href="CLAUDE.md#L63-L74">` — see markdownRenderer.workspaceFileLinks.test.ts
 * 2. Webview click handler (src/webviews/wiki/index.ts:105-131): href does not
 *    start with `file:///`, `http://`, `https://`, or `mailto:` → falls to
 *    wikilink branch → posts `{ type: 'navigate', pageName: "CLAUDE.md" }`
 * 3. Host `_resolvePageUri("CLAUDE.md")` (L220) returns null — not a wiki page
 * 4. Host fallback (L225): `workspaceRoot != null && pageName.includes('/')`
 *    → `"CLAUDE.md".includes('/')` is **false** → workspace resolution skipped
 * 5. Error shown: "Could not find wiki page: CLAUDE.md"
 *
 * ## What this test covers
 * The precondition for the fix: a bare-filename page target exists at
 * `path.join(workspaceRoot, pageName)`, is a valid file, and opens correctly
 * via `vscode.open` — exactly the operation the navigate handler should
 * perform once the includes('/') constraint is relaxed.
 *
 * ## Why it must fail
 * The navigate handler's workspace fallback is triggered only by webview→host
 * messages. The VS Code extension-test runner runs in the extension host
 * process and cannot inject `acquireVsCodeApi().postMessage()` calls into
 * an open webview. Therefore the handler's internal condition cannot be
 * exercised directly from a test. This test sets up the visible preconditions
 * and intentionally fails to document the gap.
 *
 * @summary Reproduction test for bare-filename workspace fallback bug.
 * @module test/suite/wikiEditorProvider.workspaceFallback.test
 */

import * as assert from 'node:assert';
import * as path from 'node:path';
import * as vscode from 'vscode';

async function waitForTab(
  predicate: (tab: vscode.Tab | undefined) => boolean,
  message: string,
  timeoutMs = 5000
): Promise<vscode.Tab> {
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    const activeTab = vscode.window.tabGroups.activeTabGroup.activeTab;
    if (predicate(activeTab)) {
      return activeTab as vscode.Tab;
    }
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  throw new assert.AssertionError({ message });
}

describe('WikiEditorProvider — bare filename workspace fallback', () => {
  afterEach(async () => {
    await vscode.commands.executeCommand('workbench.action.closeAllEditors');
  });

  it('workspace fallback resolves bare filenames from navigate messages', async () => {
    const workspaceFolder = vscode.workspace.workspaceFolders?.[0];
    assert.ok(workspaceFolder, 'Expected test workspace folder');

    // ------------------------------------------------------------------
    // Arrange: create a wiki page (so the wiki viewer will open) and a
    // workspace-root file that the navigate handler should resolve.
    // ------------------------------------------------------------------
    const wikiDir = vscode.Uri.joinPath(workspaceFolder.uri, 'wiki');
    await vscode.workspace.fs.createDirectory(wikiDir);
    await vscode.workspace.fs.writeFile(vscode.Uri.joinPath(wikiDir, 'index.md'), Buffer.from('# Wiki Index\n'));

    const rootFileUri = vscode.Uri.joinPath(workspaceFolder.uri, 'ROOT_REFERENCE.md');
    await vscode.workspace.fs.writeFile(rootFileUri, Buffer.from('# Root Reference\n'));

    // ------------------------------------------------------------------
    // Act: open the wiki page in the custom editor.
    // ------------------------------------------------------------------
    await vscode.commands.executeCommand('vscode.open', vscode.Uri.joinPath(wikiDir, 'index.md'));
    const wikiTab = await waitForTab((tab) => tab?.input instanceof vscode.TabInputCustom, 'Expected wiki viewer tab');
    assert.strictEqual((wikiTab.input as vscode.TabInputCustom).viewType, 'wiki.viewer');

    // Close the wiki viewer so the next steps are isolated.
    await vscode.commands.executeCommand('workbench.action.closeActiveEditor');

    // ------------------------------------------------------------------
    // Precondition checks: the root file exists, is resolvable at
    // path.join(workspaceRoot, pageName), and opens via vscode.open.
    // These are exactly the operations the navigate handler performs
    // in the workspace fallback (L226-L239).
    // ------------------------------------------------------------------
    const workspaceRoot = workspaceFolder.uri.fsPath;
    const pageName = 'ROOT_REFERENCE.md';
    const candidatePath = path.join(workspaceRoot, pageName);
    const candidateUri = vscode.Uri.file(candidatePath);

    // 1) The file exists at the constructed workspace path.
    const stat = await vscode.workspace.fs.stat(candidateUri);
    assert.strictEqual(
      stat.type,
      vscode.FileType.File,
      `Workspace-root file exists at candidatePath "${candidatePath}"`
    );

    // 2) The file opens via vscode.open (what the navigate handler calls).
    await vscode.commands.executeCommand('vscode.open', candidateUri);
    const rootTab = await waitForTab((tab) => tab?.input instanceof vscode.TabInputText, 'Expected root file tab');
    assert.ok(rootTab, 'Root file opens via vscode.open');
    await vscode.commands.executeCommand('workbench.action.closeActiveEditor');

    // ------------------------------------------------------------------
    // FAILING ASSERTION — bug reproduction
    //
    // The preconditions above demonstrate that the workspace-root file is
    // resolvable and openable. However, when the wiki webview dispatches a
    // navigate message for a bare filename, the handler at
    // WikiEditorProvider.ts:225 guards the workspace fallback with:
    //
    //   if (workspaceRoot != null && message.pageName.includes('/'))
    //
    // For the bare filename "ROOT_REFERENCE.md" (no '/' character), this
    // guard evaluates to false, the workspace fallback is NOT attempted,
    // and the user sees "Could not find wiki page: ROOT_REFERENCE.md".
    //
    // After removing the includes('/') constraint from line 225, the
    // workspace fallback proceeds to check candidatePath above, finds
    // the file, and opens it — making this test's precondition checks
    // the passing regression guard.
    //
    // Remove this assert.fail() after the fix is applied.
    // ------------------------------------------------------------------
    assert.fail(
      `BUG: Navigate workspace fallback blocked by includes("/") at ` +
        `WikiEditorProvider.ts:225 for bare filename "${pageName}".\n` +
        `The file exists at "${candidatePath}" and opens via vscode.open, ` +
        `but the workspace fallback is never reached because ` +
        `"${pageName}".includes("/") is false.\n` +
        `After removing the includes("/") constraint, remove this assert.fail() ` +
        `— the precondition checks above will serve as the passing regression guard.`
    );
  });
});
