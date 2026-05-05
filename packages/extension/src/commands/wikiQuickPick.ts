/**
 * QuickPick command that lets the user search and open wiki pages.
 *
 * Shows all pages on first open (via wiki list), then passes every non-empty
 * query directly to the wiki CLI. Search results are two-line items: the
 * matched snippets appear where the summary sits for list items, and the
 * summary is shown on the second line via `detail`. All search result items
 * carry `alwaysShow: true` so VS Code's own fuzzy filter does not hide CLI
 * results that only match on body text.
 *
 * @summary QuickPick command that lets the user search and open wiki pages.
 */

import * as vscode from 'vscode';
import { runWikiCommand } from '../utils/wikiBinary.js';
import type { WikiBinaryManager } from '../utils/wikiInstaller.js';
import type { NamespaceCache } from '../wiki/namespaceCache.js';

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

/** Item returned by `wiki namespaces --format json`. */
interface NamespaceEntry {
  namespace: string | null;
  path: string;
  abs_path: string;
}

/** A QuickPickItem extended with the resolved file path. */
type WikiQuickPickItem = vscode.QuickPickItem & { file: string };

/**
 * Convert a wiki list item to a single-line QuickPickItem.
 *
 * @param item - A wiki list item returned by `wiki list --format json`.
 * @returns A QuickPickItem with the page title as label and summary as description.
 */
function toListQuickPickItem(item: WikiListItem): WikiQuickPickItem {
  return {
    label: item.title,
    detail: item.summary,
    file: item.file
  };
}

/**
 * Convert a wiki search result to a two-line QuickPickItem.
 *
 * The `description` line (inline, after the title) shows the matched snippets
 * joined by " … ". The `detail` line (below the title) shows the page summary.
 * Bold highlighting of the matched term is not possible in VS Code QuickPick
 * description/detail fields — they are plain text only.
 *
 * `alwaysShow: true` prevents VS Code's own fuzzy filter from hiding items
 * whose match is in body text rather than the title or summary.
 *
 * @param item - A wiki search result returned by `wiki <query> --format json`.
 * @returns A two-line QuickPickItem with snippets as description and summary as detail.
 */
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
 * Convert a namespace entry to a QuickPickItem.
 *
 * The namespace name is the label; the filesystem path is the detail.
 * The file path is included as a sentinel for type compatibility — namespace
 * selection prefills the input rather than opening a file.
 *
 * @param item - A namespace entry from `wiki namespaces --format json`.
 * @returns A QuickPickItem with namespace as label and path as detail.
 */
function toNamespaceQuickPickItem(item: NamespaceEntry): WikiQuickPickItem {
  return {
    label: item.namespace!,
    detail: item.path,
    file: item.abs_path
  };
}

/**
 * Return the filesystem path of the first VS Code workspace folder, or undefined
 * if no folder is open. The wiki CLI requires a cwd inside the git repo to
 * discover repository boundaries.
 *
 * @returns The workspace root path, or undefined if no folder is open.
 */
function workspaceRoot(): string | undefined {
  return vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
}

/**
 * Derive the namespace from the active text editor, falling back to `'*'`
 * (all namespaces) when no document is active or no cache is available.
 *
 * @param cache - Optional NamespaceCache for file-to-namespace resolution.
 * @returns The namespace label to pass via `-n`.
 */
function deriveNamespace(cache?: NamespaceCache): string {
  if (cache != null) {
    const editor = vscode.window.activeTextEditor;
    if (editor != null) {
      return cache.resolveNamespaceForFile(editor.document.uri.fsPath) ?? 'default';
    }
  }
  return '*';
}

/**
 * Parse a quick-pick query to extract an `@namespace` prefix.
 *
 * When the query starts with `@<name>` (e.g. `@mesh something`), returns
 * the namespace portion and the remainder as the clean query. Without a
 * prefix, derives the namespace from the active document (or `'*'`).
 *
 * @param query - Raw query text from the quick-pick input.
 * @param cache - Optional NamespaceCache for file-to-namespace resolution.
 * @returns The resolved namespace and the cleaned query string.
 */
function parseNamespaceQuery(query: string, cache?: NamespaceCache): { ns: string; cleanQuery: string } {
  // @namespace prefix overrides any active-document namespace.
  const atMatch = query.match(/^@(\S+)\s+(.*)/);
  if (atMatch != null) {
    return { ns: atMatch[1]!, cleanQuery: atMatch[2]! };
  }
  // No prefix — use the active document's namespace.
  if (cache != null) {
    const editor = vscode.window.activeTextEditor;
    if (editor != null) {
      const ns = cache.resolveNamespaceForFile(editor.document.uri.fsPath) ?? 'default';
      return { ns, cleanQuery: query };
    }
  }
  return { ns: '*', cleanQuery: query };
}

/**
 * Load all wiki pages for the initial (empty-query) state.
 * Shows a VS Code error notification and returns an empty array on failure.
 *
 * @param binaryPath - Absolute path to the resolved wiki CLI binary.
 * @param cache - Optional NamespaceCache to scope the query to a namespace.
 * @returns All wiki pages as QuickPickItems, or an empty array on error.
 */
async function loadAllPages(binaryPath: string, cache?: NamespaceCache): Promise<WikiQuickPickItem[]> {
  const ns = deriveNamespace(cache);
  try {
    const result = await runWikiCommand(binaryPath, ['-n', ns, 'list', '--format', 'json'], undefined, workspaceRoot());
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

/**
 * Load all available wiki namespaces for the @-prefix namespace selector.
 * Filters out the null-namespace (default wiki) entry.
 * Shows a VS Code error notification and returns an empty array on failure.
 *
 * @param binaryPath - Absolute path to the resolved wiki CLI binary.
 * @returns All named namespaces as QuickPickItems, or an empty array on error.
 */
async function loadNamespaces(binaryPath: string): Promise<WikiQuickPickItem[]> {
  try {
    const result = await runWikiCommand(binaryPath, ['namespaces', '--format', 'json'], undefined, workspaceRoot());
    if (result.exitCode !== 0) {
      const message = result.stderr.trim() || `wiki namespaces exited with code ${result.exitCode}`;
      console.warn('[wiki-extension] wiki namespaces failed:', message);
      void vscode.window.showErrorMessage(`Wiki: ${message}`);
      return [];
    }
    const items = JSON.parse(result.stdout) as NamespaceEntry[];
    return items.filter((item) => item.namespace != null).map(toNamespaceQuickPickItem);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    console.error('[wiki-extension] Failed to load wiki namespaces:', err);
    void vscode.window.showErrorMessage(`Wiki: ${message}`);
    return [];
  }
}

/**
 * Search wiki pages for the given query.
 * Shows a VS Code error notification and returns an empty array on failure.
 *
 * Supports an `@namespace` prefix: when the query starts with `@<name>`,
 * the search is scoped to that namespace. Without a prefix, the active
 * document's namespace is used (or all namespaces if no document is open).
 *
 * @param binaryPath - Absolute path to the resolved wiki CLI binary.
 * @param query - The search query to pass to the wiki CLI.
 * @param signal - AbortSignal used to cancel the underlying wiki process.
 * @param cache - Optional NamespaceCache for namespace derivation.
 * @returns Matching wiki pages as QuickPickItems, or an empty array on error.
 */
async function searchPages(
  binaryPath: string,
  query: string,
  signal: AbortSignal,
  cache?: NamespaceCache
): Promise<WikiQuickPickItem[]> {
  const { ns, cleanQuery } = parseNamespaceQuery(query, cache);
  try {
    const result = await runWikiCommand(
      binaryPath,
      ['-n', ns, cleanQuery, '--format', 'json'],
      signal,
      workspaceRoot()
    );
    // If the signal was aborted, the process was killed intentionally — not an error.
    if (signal.aborted) {
      return [];
    }
    if (result.exitCode !== 0) {
      const message = result.stderr.trim() || `wiki search exited with code ${result.exitCode}`;
      console.warn('[wiki-extension] wiki search failed:', message);
      void vscode.window.showErrorMessage(`Wiki: ${message}`);
      return [];
    }
    const items = JSON.parse(result.stdout) as WikiSearchItem[];
    return items.map(toSearchQuickPickItem);
  } catch (err) {
    // If the signal was aborted, the spawn error (EPIPE, etc.) is expected.
    if (signal.aborted) {
      return [];
    }
    const message = err instanceof Error ? err.message : String(err);
    console.error('[wiki-extension] Failed to search wiki pages:', err);
    void vscode.window.showErrorMessage(`Wiki: ${message}`);
    return [];
  }
}

/**
 * Open a wiki file in the custom wiki viewer.
 *
 * @param file - Absolute path to the wiki file to open.
 */
async function openWikiFile(file: string): Promise<void> {
  const uri = vscode.Uri.file(file);
  await vscode.commands.executeCommand('vscode.openWith', uri, 'wiki.viewer');
}

/**
 * Returns true when the input value represents a namespace-list query,
 * i.e. starts with `@` and does not yet contain a space (the user is
 * still typing or selecting a namespace name).
 *
 * @param value - The current QuickPick input value.
 * @returns True when the picker should show namespace items.
 */
export function isNamespaceMode(value: string): boolean {
  return value.startsWith('@') && !value.includes(' ');
}

/**
 * Show a QuickPick that lets the user browse and search wiki pages.
 * An empty query lists all pages; a non-empty query performs a ranked search.
 *
 * @param binaryManager - Service that resolves or installs the wiki CLI.
 * @param cache - Optional NamespaceCache to scope queries to a namespace.
 */
export async function wikiQuickPick(binaryManager: WikiBinaryManager, cache?: NamespaceCache): Promise<void> {
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

  // Load all pages immediately for the initial empty state.
  const initialItems = await loadAllPages(binaryPath, cache);
  qp.items = initialItems;
  qp.busy = false;

  // Namespace items loaded lazily on first @-prefix input.
  let namespaceItems: WikiQuickPickItem[] = [];
  let namespacesLoaded = false;

  let activeAbort: AbortController | undefined;

  qp.onDidChangeValue((query) => {
    activeAbort?.abort();

    if (query.trim() === '') {
      qp.items = initialItems;
      return;
    }

    // @-prefix without space: namespace list mode — filter namespaces client-side.
    if (isNamespaceMode(query)) {
      void (async () => {
        if (!namespacesLoaded) {
          qp.busy = true;
          namespacesLoaded = true;
          namespaceItems = await loadNamespaces(binaryPath);
          qp.busy = false;
        }
        if (!isNamespaceMode(qp.value)) return;
        const filter = qp.value.slice(1).toLowerCase();
        qp.items = namespaceItems.filter((item) => item.label.toLowerCase().includes(filter));
      })();
      return;
    }

    const abort = new AbortController();
    activeAbort = abort;
    qp.busy = true;

    void (async () => {
      const results = await searchPages(binaryPath, query.trim(), abort.signal, cache);
      if (!abort.signal.aborted) {
        qp.items = results;
        qp.busy = false;
      }
    })();
  });

  qp.onDidAccept(async () => {
    const selected = qp.selectedItems[0];
    if (selected == null) return;

    // NamespaceList mode: prefill input and keep picker open.
    if (isNamespaceMode(qp.value) && namespaceItems.length > 0) {
      qp.value = `@${selected.label} `;
      return;
    }

    qp.hide();
    await openWikiFile(selected.file);
  });

  qp.onDidHide(() => {
    activeAbort?.abort();
    qp.dispose();
  });

  qp.show();
}
