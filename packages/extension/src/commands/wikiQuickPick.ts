/**
 * QuickPick command that lets the user search and open wiki pages.
 *
 * An empty query lists all pages via `wiki list`; a non-empty query runs
 * `wiki <query>` (search). Queries are scoped to the working directory the
 * CLI is spawned in.
 *
 * @summary QuickPick command that lets the user search and open wiki pages.
 */

import * as path from 'node:path';
import * as vscode from 'vscode';
import { formatLogError, getWikiLogger } from '../utils/logger.js';
import { loadValidatedRecentlyViewed, recordWikiView } from '../utils/recentlyViewed.js';
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

/**
 * Map a [WikiListItem](./wikiQuickPick.ts) from `wiki list --format json`
 * into a [WikiQuickPickItem](./wikiQuickPick.ts) for the QuickPick.
 *
 * @param item - A deserialized list result from the wiki CLI.
 * @returns A QuickPick item with the page title, summary, and repo-relative file path.
 */
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

/**
 * Return the absolute filesystem path of the first workspace folder.
 *
 * @returns Absolute `fsPath` of the first workspace folder, or `undefined`
 *          when no workspace folder is open.
 */
export function workspaceRoot(): string | undefined {
  return vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
}

/**
 * Resolve a possibly repo-relative path to an absolute filesystem path.
 *
 * When [workspaceRoot()](./wikiQuickPick.ts) is available and `file` is
 * not already absolute, resolve it against the workspace root. Otherwise
 * return `file` unchanged (it is already absolute, or there is no
 * workspace folder to resolve against).
 *
 * @param file - A repo-relative or absolute filesystem path.
 * @returns The resolved absolute filesystem path.
 */
function resolveWorkspacePath(file: string): string {
  const root = workspaceRoot();
  if (root == null || path.isAbsolute(file)) return file;
  return path.resolve(root, file);
}

async function loadAllPages(binaryPath: string): Promise<WikiQuickPickItem[]> {
  try {
    const result = await runWikiCommand(binaryPath, ['list', '--format', 'json'], undefined, workspaceRoot());
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

/**
 * Open a wiki file in VS Code's text editor.
 *
 * Resolves repo-relative paths against [workspaceRoot()](./wikiQuickPick.ts)
 * before constructing the URI, so paths emitted by the CLI
 * (`wiki list`, `wiki <query>`) resolve correctly.
 *
 * @param file - A repo-relative or absolute path to a wiki markdown file.
 */
export async function openWikiFile(file: string): Promise<void> {
  const uri = vscode.Uri.file(resolveWorkspacePath(file));
  await vscode.window.showTextDocument(uri, { preview: false });
}

/**
 * Handler returned by {@link createSearchHandler}.
 */
export interface SearchHandler {
  /** Handle a query change event from the QuickPick. */
  onQueryChange: (query: string) => void;
  /** Abort any in-flight search. */
  dispose: () => void;
}

/**
 * Create a handler for QuickPick value-change events that manages abort
 * controllers for in-flight searches.
 *
 * Queries are debounced by 150 ms so rapid keystrokes coalesce into one
 * search invocation. An aborted search skips its completion callbacks by
 * design — a newer query owns the UI state now — so whoever aborts without
 * a successor must clear the busy indicator itself: emptying the query
 * restores the initial page list via {@link onResetToInitial} and clears
 * busy via {@link onBusy}, and disposing the handler does the same for any
 * in-flight search.
 *
 * @param search           - Async search function (e.g., spawns wiki CLI).
 * @param onResults        - Called with search results when they arrive.
 * @param onBusy           - Called to update busy/loading state.
 * @param onResetToInitial - Called when query is cleared.
 * @returns A handler object with `onQueryChange` and `dispose` methods.
 */
export function createSearchHandler(
  search: (query: string, signal: AbortSignal) => Promise<WikiQuickPickItem[]>,
  onResults: (items: WikiQuickPickItem[]) => void,
  onBusy: (busy: boolean) => void,
  onResetToInitial: () => void
): SearchHandler {
  let activeAbort: AbortController | undefined;
  let debounceTimer: ReturnType<typeof setTimeout> | undefined;

  return {
    onQueryChange(query: string): void {
      activeAbort?.abort();
      clearTimeout(debounceTimer);

      if (query.trim() === '') {
        // The aborted search's continuation skips onBusy(false) by design,
        // and no new search runs for an empty query to clear it later.
        if (activeAbort !== undefined) {
          onBusy(false);
          activeAbort = undefined;
        }
        onResetToInitial();
        return;
      }

      debounceTimer = setTimeout(() => {
        const abort = new AbortController();
        activeAbort = abort;
        onBusy(true);

        void (async () => {
          const results = await search(query.trim(), abort.signal);
          if (!abort.signal.aborted) {
            activeAbort = undefined;
            onResults(results);
            onBusy(false);
          }
        })();
      }, 150);
    },

    dispose(): void {
      // Same as the empty-query path: an aborted search cannot clear busy
      // itself.
      if (activeAbort !== undefined) {
        activeAbort.abort();
        onBusy(false);
        activeAbort = undefined;
      }
      clearTimeout(debounceTimer);
    }
  };
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
  qp.show();

  const loadStart = Date.now();
  const [recentItems, listItems] = await Promise.all([loadValidatedRecentlyViewed(context), loadAllPages(binaryPath)]);
  const recentPaths = new Set(recentItems.map((item) => item.file));
  const initialItems: WikiQuickPickItem[] = [
    ...recentItems,
    ...listItems.filter((item) => !recentPaths.has(resolveWorkspacePath(item.file)))
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

  const searchHandler = createSearchHandler(
    (query, signal) => searchPages(binaryPath, query, signal),
    (items) => {
      qp.items = items;
    },
    (busy) => {
      qp.busy = busy;
    },
    () => {
      qp.items = initialItems;
    }
  );

  qp.onDidChangeValue((query) => searchHandler.onQueryChange(query));

  qp.onDidAccept(async () => {
    const selected = qp.selectedItems[0];
    if (selected == null) return;
    qp.hide();
    await recordWikiView(context, resolveWorkspacePath(selected.file));
    await openWikiFile(selected.file);
  });

  qp.onDidHide(() => {
    searchHandler.dispose();
    qp.dispose();
  });
}
