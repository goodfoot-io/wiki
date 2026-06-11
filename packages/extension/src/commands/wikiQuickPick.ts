/**
 * QuickPick command that lets the user search and open wiki pages.
 *
 * An empty query lists all pages via `wiki list`; a non-empty query runs
 * `wiki <query>` (search). Queries are scoped to the working directory the
 * CLI is spawned in.
 *
 * @summary QuickPick command that lets the user search and open wiki pages.
 */

import * as vscode from 'vscode';
import { formatLogError, getWikiLogger } from '../utils/logger.js';
import { loadValidatedRecentlyViewed } from '../utils/recentlyViewed.js';
import { runWikiCommand } from '../utils/wikiBinary.js';
import type { WikiBinaryManager } from '../utils/wikiInstaller.js';

function qpLog() {
  return getWikiLogger().getChildLogger({ label: 'QuickPick' });
}

/** Item returned by `wiki list --format json`. */
export interface WikiListItem {
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

export type WikiQuickPickItem = vscode.QuickPickItem & { file: string };

export function toListQuickPickItem(item: WikiListItem): WikiQuickPickItem {
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

export function workspaceRoot(): string | undefined {
  return vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
}

async function loadAllPages(binaryPath: string): Promise<WikiQuickPickItem[]> {
  try {
    const result = await runWikiCommand(
      binaryPath,
      ['list', '--limit', '10', '--format', 'json'],
      undefined,
      workspaceRoot()
    );
    if (result.exitCode !== 0) {
      const message = result.stderr.trim() || `wiki list exited with code ${result.exitCode}`;
      qpLog().warn('wiki list failed: %s', message);
      void vscode.window.showErrorMessage(`Wiki: ${message}`);
      return [];
    }
    const items = JSON.parse(result.stdout) as WikiListItem[];
    return items.map(toListQuickPickItem);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    qpLog().error('Failed to load wiki pages: %s', formatLogError(err));
    void vscode.window.showErrorMessage(`Wiki: ${message}`);
    return [];
  }
}

async function searchPages(binaryPath: string, query: string, signal: AbortSignal): Promise<WikiQuickPickItem[]> {
  try {
    const result = await runWikiCommand(binaryPath, [query, '--format', 'json'], signal, workspaceRoot());
    if (signal.aborted) return [];
    if (result.exitCode !== 0) {
      const message = result.stderr.trim() || `wiki search exited with code ${result.exitCode}`;
      qpLog().warn('wiki search failed: %s', message);
      void vscode.window.showErrorMessage(`Wiki: ${message}`);
      return [];
    }
    const items = JSON.parse(result.stdout) as WikiSearchItem[];
    return items.map(toSearchQuickPickItem);
  } catch (err) {
    if (signal.aborted) return [];
    const message = err instanceof Error ? err.message : String(err);
    qpLog().error('Failed to search wiki "%s": %s', query, formatLogError(err));
    void vscode.window.showErrorMessage(`Wiki: ${message}`);
    return [];
  }
}

export async function openWikiFile(file: string): Promise<void> {
  const uri = vscode.Uri.file(file);
  await vscode.commands.executeCommand('vscode.openWith', uri, 'wiki.viewer');
}

/**
 * Show a QuickPick that lets the user browse and search wiki pages.
 *
 * @param binaryManager - Service that resolves or installs the wiki CLI.
 * @param context       - Extension context, used to read and update the per-workspace recently-viewed list.
 */
export async function wikiQuickPick(binaryManager: WikiBinaryManager, context: vscode.ExtensionContext): Promise<void> {
  const log = qpLog();
  const invokedAt = Date.now();
  log.info('wikiQuickPick invoked');
  let binaryPath: string;
  try {
    const readyStart = Date.now();
    binaryPath = (
      await vscode.window.withProgress(
        { location: vscode.ProgressLocation.Notification, title: 'Preparing wiki CLI…' },
        () => binaryManager.ready()
      )
    ).path;
    log.debug('binaryManager.ready resolved in %dms -> %s', Date.now() - readyStart, binaryPath);
  } catch (error) {
    log.error('binaryManager.ready failed: %s', formatLogError(error));
    void vscode.window.showErrorMessage(`Wiki: ${binaryManager.formatFailure(error)}`);
    return;
  }

  const qp = vscode.window.createQuickPick<WikiQuickPickItem>();
  qp.placeholder = 'Search wiki pages…';
  qp.matchOnDetail = true;
  qp.busy = true;

  const loadStart = Date.now();
  const [recentItems, listItems] = await Promise.all([loadValidatedRecentlyViewed(context), loadAllPages(binaryPath)]);
  const recentPaths = new Set(recentItems.map((item) => item.file));
  const initialItems: WikiQuickPickItem[] = [
    ...recentItems,
    ...listItems.filter((item) => !recentPaths.has(item.file))
  ];
  log.info(
    'Initial list loaded: %d recent + %d list (%d total) in %dms (total since invoke %dms)',
    recentItems.length,
    listItems.length,
    initialItems.length,
    Date.now() - loadStart,
    Date.now() - invokedAt
  );
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
