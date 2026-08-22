/**
 * Reproduction test: clearing the query mid-search leaves the QuickPick busy
 * spinner stuck on.
 *
 * When the user empties the input while a debounced search is in flight,
 * createSearchHandler aborts the run and resets items to the initial list,
 * but neither the empty-query branch nor the aborted search continuation
 * ever reports onBusy(false) — the spinner keeps spinning until some later
 * search completes.
 *
 * Expected: clearing the query fully resets the picker — initial items are
 * restored AND busy is reported false. The final busy assertion MUST FAIL
 * against the current unfixed code, confirming the bug.
 *
 * @summary Reproduction test for stuck busy indicator after a mid-search clear.
 * @module test/suite/wikiQuickPick.busyReset.test
 */

import * as assert from 'node:assert';
import type { WikiQuickPickItem } from '../../src/commands/wikiQuickPick.js';
import { createSearchHandler } from '../../src/commands/wikiQuickPick.js';

describe('wikiQuickPick — busy indicator stuck after mid-search clear (bug reproduction)', () => {
  it('clearing the query while a search is in flight clears busy and restores initial items', async function () {
    this.timeout(10000);

    const busyEvents: boolean[] = [];
    let resetCount = 0;
    let searchStarted = false;
    let searchSettled = false;

    // Slow search simulating a wiki CLI round-trip; resolves rather than
    // rejects when aborted, mirroring searchPages' abort handling.
    const mockSearch = async (query: string, signal: AbortSignal): Promise<WikiQuickPickItem[]> => {
      searchStarted = true;
      await new Promise((resolve) => setTimeout(resolve, 100));
      searchSettled = true;
      return signal.aborted ? [] : [{ label: query, file: `${query}.md` }];
    };

    const handler = createSearchHandler(
      mockSearch,
      () => {},
      (busy) => busyEvents.push(busy),
      () => {
        resetCount += 1;
      }
    );

    // Type a query and wait past the 150ms debounce so the search spawns and
    // flips busy on, then clear while it is still in flight (~20ms into its
    // 100ms latency).
    handler.onQueryChange('auth');
    await new Promise((resolve) => setTimeout(resolve, 170));

    assert.ok(searchStarted, 'expected the debounced search to spawn');
    assert.ok(busyEvents.includes(true), 'expected the spawned search to report busy=true');
    assert.ok(!searchSettled, 'search must still be in flight when the query is cleared');

    handler.onQueryChange('');

    // Let the aborted search's promise settle before judging the outcome.
    await new Promise((resolve) => setTimeout(resolve, 200));

    handler.dispose();

    // Items reset already works today; busy clearing is the bug under test.
    assert.strictEqual(resetCount, 1, 'clearing the query must restore the initial item list');
    assert.strictEqual(
      busyEvents[busyEvents.length - 1],
      false,
      `expected busy=false after clearing the query mid-search, got [${busyEvents.join(', ')}] — ` +
        'the spinner is stranded on because neither the empty-query branch nor the aborted ' +
        'search continuation ever reports onBusy(false)'
    );
  });
});
