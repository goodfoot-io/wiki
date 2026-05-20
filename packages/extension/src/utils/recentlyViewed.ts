/**
 * Per-workspace MRU history of wiki articles viewed through
 * [WikiEditorProvider.resolveCustomTextEditor](../providers/WikiEditorProvider.ts).
 *
 * Stored as a string array of absolute paths in
 * [context.workspaceState](../extension.ts) under
 * [RECENTLY_VIEWED_KEY](./recentlyViewed.ts), most-recent first. Entries that
 * no longer resolve to a wiki file (missing, renamed, or stripped of
 * frontmatter) are dropped silently on the next picker open.
 *
 * @summary Per-workspace MRU list of wiki articles for the QuickPick.
 */

import type * as vscode from 'vscode';
import { hasWikiFrontmatter, readFrontmatter } from './frontmatter.js';
import { getWikiLogger } from './logger.js';

/** `workspaceState` key holding the MRU absolute-path array. */
export const RECENTLY_VIEWED_KEY = 'wiki.recentlyViewed';

/** Silent upper bound — older entries fall off as new ones are pushed. */
export const RECENTLY_VIEWED_CAP = 50;

export interface RecentlyViewedItem extends vscode.QuickPickItem {
  file: string;
}

function mruLog() {
  return getWikiLogger().getChildLogger({ label: 'Wiki.MRU' });
}

function readRaw(context: vscode.ExtensionContext): string[] {
  const raw = context.workspaceState.get<unknown>(RECENTLY_VIEWED_KEY);
  if (!Array.isArray(raw)) return [];
  return raw.filter((entry): entry is string => typeof entry === 'string');
}

/**
 * Push `fsPath` to the front of the MRU list. Any prior occurrence is
 * removed; the result is truncated to [RECENTLY_VIEWED_CAP](./recentlyViewed.ts).
 *
 * @param context - The extension's VS Code context (workspace-scoped state).
 * @param fsPath  - Absolute filesystem path, as obtained from `Uri.fsPath`.
 */
export async function recordWikiView(context: vscode.ExtensionContext, fsPath: string): Promise<void> {
  const current = readRaw(context);
  const next = [fsPath, ...current.filter((entry) => entry !== fsPath)].slice(0, RECENTLY_VIEWED_CAP);
  await context.workspaceState.update(RECENTLY_VIEWED_KEY, next);
  mruLog().debug('recorded view (size=%d): %s', next.length, fsPath);
}

/**
 * Read the MRU list, validate each entry in parallel, drop entries that no
 * longer resolve to wiki pages, persist the truncated list back when any
 * entries were dropped, and return [RecentlyViewedItem](./recentlyViewed.ts)s
 * ready to be merged into the QuickPick.
 *
 * @param context - The extension's VS Code context (workspace-scoped state).
 * @returns Validated MRU items in most-recent-first order.
 */
export async function loadValidatedRecentlyViewed(context: vscode.ExtensionContext): Promise<RecentlyViewedItem[]> {
  const raw = readRaw(context);
  if (raw.length === 0) return [];

  const startedAt = Date.now();
  const resolved = await Promise.all(
    raw.map(async (fsPath) => {
      const info = await readFrontmatter(fsPath);
      if (!hasWikiFrontmatter(info)) return null;
      return {
        label: info!.title!,
        detail: info!.summary!,
        file: fsPath
      } satisfies RecentlyViewedItem;
    })
  );

  const items: RecentlyViewedItem[] = [];
  const survivingPaths: string[] = [];
  for (let i = 0; i < resolved.length; i++) {
    const item = resolved[i];
    if (item == null) continue;
    items.push(item);
    survivingPaths.push(raw[i]!);
  }

  const droppedCount = raw.length - survivingPaths.length;
  mruLog().debug(
    'validated MRU: %d kept, %d dropped in %dms',
    survivingPaths.length,
    droppedCount,
    Date.now() - startedAt
  );

  if (droppedCount > 0) {
    await context.workspaceState.update(RECENTLY_VIEWED_KEY, survivingPaths);
  }

  return items;
}
