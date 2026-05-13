/**
 * QuickPick command that lets the user search and open wiki pages.
 *
 * An empty query lists all pages via `wiki list`; a non-empty query runs
 * `wiki <query>` (search). All queries are scoped to the single wiki root
 * configured via the CLI's `--root` flag.
 *
 * @summary QuickPick command that lets the user search and open wiki pages.
 */

import * as vscode from 'vscode';
import { getSourceArgs } from '../utils/sourceMode.js';
import { runWikiCommand } from '../utils/wikiBinary.js';
import type { WikiBinaryManager } from '../utils/wikiInstaller.js';

/** Item returned by `wiki list --format json`. */
interface WikiListItem {
  title: string;
  aliases: string[];
  tags: string[];
  summary: string;
  file: string;
}

/** Item returned by `wiki <query> --format json` (search). */
interface WikiSearchItem {
  title: string;
  file: string;
  summary: string;
  snippets: Array<{ line: number; text: string }>;
}

type WikiQuickPickItem = vscode.QuickPickItem & { file: string };

function toListQuickPickItem(item: WikiListItem): WikiQuickPickItem {
  return {
    label: item.title,
    detail: item.summary,
    file: item.file
  };
}

function toSearchQuickPickItem(item: WikiSearchItem): WikiQuickPickItem {
  const snippetText = item.snippets.map((s) => s.text.trim()).join(' … ');
  return {
    label: item.title,
    description: snippetText.length > 0 ? snippetText : item.summary,
    detail: snippetText.length > 0 ? item.summary : undefined,
    alwaysShow: true,
    file: item.file
  };
}

function workspaceRoot(): string | undefined {
  return vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
}

async function loadAllPages(binaryPath: string): Promise<WikiQuickPickItem[]> {
  const sourceArgs = getSourceArgs();
  try {
    const result = await runWikiCommand(
      binaryPath,
      [...sourceArgs, 'list', '--format', 'json'],
      undefined,
      workspaceRoot()
    );
    if (result.exitCode !== 0) {
      const message = result.stderr.trim() || `wiki list exited with code ${result.exitCode}`;
      console.warn('[wiki-extension] wiki list failed:', message);
      void vscode.window.showErrorMessage(`Wiki: ${message}`);
      return [];
    }
    const items = JSON.parse(result.stdout) as WikiListItem[];
    return items.map(toListQuickPickItem);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    console.error('[wiki-extension] Failed to load wiki pages:', err);
    void vscode.window.showErrorMessage(`Wiki: ${message}`);
    return [];
  }
}

async function searchPages(binaryPath: string, query: string, signal: AbortSignal): Promise<WikiQuickPickItem[]> {
  const sourceArgs = getSourceArgs();
  try {
    const result = await runWikiCommand(
      binaryPath,
      [...sourceArgs, query, '--format', 'json'],
      signal,
      workspaceRoot()
    );
    if (signal.aborted) return [];
    if (result.exitCode !== 0) {
      const message = result.stderr.trim() || `wiki search exited with code ${result.exitCode}`;
      console.warn('[wiki-extension] wiki search failed:', message);
      void vscode.window.showErrorMessage(`Wiki: ${message}`);
      return [];
    }
    const items = JSON.parse(result.stdout) as WikiSearchItem[];
    return items.map(toSearchQuickPickItem);
  } catch (err) {
    if (signal.aborted) return [];
    const message = err instanceof Error ? err.message : String(err);
    console.error('[wiki-extension] Failed to search wiki pages:', err);
    void vscode.window.showErrorMessage(`Wiki: ${message}`);
    return [];
  }
}

async function openWikiFile(file: string): Promise<void> {
  const uri = vscode.Uri.file(file);
  await vscode.commands.executeCommand('vscode.openWith', uri, 'wiki.viewer');
}

/**
 * Show a QuickPick that lets the user browse and search wiki pages.
 *
 * @param binaryManager - Service that resolves or installs the wiki CLI.
 */
export async function wikiQuickPick(binaryManager: WikiBinaryManager): Promise<void> {
  let binaryPath: string;
  try {
    binaryPath = (
      await vscode.window.withProgress(
        { location: vscode.ProgressLocation.Notification, title: 'Preparing wiki CLI…' },
        () => binaryManager.ready()
      )
    ).path;
  } catch (error) {
    void vscode.window.showErrorMessage(`Wiki: ${binaryManager.formatFailure(error)}`);
    return;
  }

  const qp = vscode.window.createQuickPick<WikiQuickPickItem>();
  qp.placeholder = 'Search wiki pages…';
  qp.matchOnDetail = true;
  qp.busy = true;

  const initialItems = await loadAllPages(binaryPath);
  qp.items = initialItems;
  qp.busy = false;

  let activeAbort: AbortController | undefined;

  qp.onDidChangeValue((query) => {
    activeAbort?.abort();

    if (query.trim() === '') {
      qp.items = initialItems;
      return;
    }

    const abort = new AbortController();
    activeAbort = abort;
    qp.busy = true;

    void (async () => {
      const results = await searchPages(binaryPath, query.trim(), abort.signal);
      if (!abort.signal.aborted) {
        qp.items = results;
        qp.busy = false;
      }
    })();
  });

  qp.onDidAccept(async () => {
    const selected = qp.selectedItems[0];
    if (selected == null) return;
    qp.hide();
    await openWikiFile(selected.file);
  });

  qp.onDidHide(() => {
    activeAbort?.abort();
    qp.dispose();
  });

  qp.show();
}
