/**
 * Reproduction test: the QuickPick busy flag sticks when the query is
 * cleared mid-search.
 *
 * In `createSearchHandler`, an in-flight search whose signal is aborted
 * skips both of its completion callbacks. Clearing the input aborts the
 * active search and takes the empty-query/reset branch, which never calls
 * `onBusy(false)` — and no new search fires for an empty query, so `busy`
 * stays true until some later non-empty query completes. The same gap
 * exists in `dispose()`.
 *
 * This test drives the handler directly with a search that resolves only
 * when aborted, and asserts busy is cleared after both the reset path and
 * dispose. It MUST FAIL against the current unfixed code, where neither
 * path clears busy.
 *
 * @summary Reproduction test for the stuck QuickPick busy flag.
 * @module test/suite/wikiQuickPick.busy.test
 */

import * as assert from 'node:assert';
import type { WikiQuickPickItem } from '../../src/commands/wikiQuickPick.js';
import { createSearchHandler } from '../../src/commands/wikiQuickPick.js';

describe('wikiQuickPick — busy clears on abort (bug reproduction)', () => {
  it('clearing the query while a search is in flight clears busy', async function () {
    this.timeout(10000);

    const { handler, busyValues, resets } = startStuckSearch();

    // Type a query; after the 150ms debounce the search starts and flips
    // the busy flag on.
    handler.onQueryChange('wiki');
    await waitForCondition(() => busyValues.includes(true), 'search to start');

    // Clearing the input aborts the active search and resets to the
    // initial list; no new search fires for an empty query.
    handler.onQueryChange('');
    await settle();

    assert.strictEqual(resets.length, 1, 'Expected the reset-to-initial callback to run once');
    assert.strictEqual(
      busyValues[busyValues.length - 1],
      false,
      `Expected busy to be cleared after the query was emptied but it stayed ${String(
        busyValues[busyValues.length - 1]
      )} — the aborted search skipped its completion callbacks.`
    );

    handler.dispose();
  });

  it('dispose while a search is in flight clears busy', async function () {
    this.timeout(10000);

    const { handler, busyValues } = startStuckSearch();
    handler.onQueryChange('wiki');
    await waitForCondition(() => busyValues.includes(true), 'search to start');

    handler.dispose();
    await settle();

    assert.strictEqual(
      busyValues[busyValues.length - 1],
      false,
      'Expected busy to be cleared on dispose but the aborted search left it set.'
    );
  });
});

/**
 * Start a handler whose mock search resolves only when its abort signal
 * fires — mirroring a wiki CLI process killed mid-run.
 *
 * @returns The handler plus the arrays recording busy flips and resets.
 */
function startStuckSearch(): {
  handler: ReturnType<typeof createSearchHandler>;
  busyValues: boolean[];
  resets: number[];
} {
  const busyValues: boolean[] = [];
  const resets: number[] = [];
  const search = (_query: string, signal: AbortSignal): Promise<WikiQuickPickItem[]> =>
    new Promise((resolve) => {
      if (signal.aborted) {
        resolve([]);
        return;
      }
      signal.addEventListener('abort', () => resolve([]), { once: true });
    });

  const handler = createSearchHandler(
    search,
    () => {},
    (busy) => busyValues.push(busy),
    () => resets.push(1)
  );
  return { handler, busyValues, resets };
}

async function waitForCondition(predicate: () => boolean, what: string): Promise<void> {
  const deadline = Date.now() + 2000;
  while (!predicate()) {
    if (Date.now() > deadline) {
      throw new Error(`Timed out waiting for ${what}`);
    }
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}

async function settle(): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, 50));
}
