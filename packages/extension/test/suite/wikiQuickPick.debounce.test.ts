/**
 * Reproduction test: QuickPick's onDidChangeValue handler has no debounce,
 * spawning a wiki CLI process on every keystroke.
 *
 * When the handler fires via onDidChangeValue, rapid keystrokes within any
 * short window each immediately invoke searchPages, which spawns a separate
 * wiki CLI process. There is no debounce timer to batch them.
 *
 * A fix would add a 150ms debounce to `createSearchHandler` so that only a
 * single search is triggered after the user stops typing. This test asserts
 * the desired behavior (1 call) and MUST FAIL against the current unfixed
 * code, confirming the bug.
 *
 * @summary Reproduction test for missing debounce in wikiQuickPick search.
 * @module test/suite/wikiQuickPick.debounce.test
 */

import * as assert from 'node:assert';
import type { WikiQuickPickItem } from '../../src/commands/wikiQuickPick.js';
import { createSearchHandler } from '../../src/commands/wikiQuickPick.js';

describe('wikiQuickPick — missing debounce (bug reproduction)', () => {
  it('rapid keystrokes trigger one search call per key (should be debounced to 1)', async function () {
    this.timeout(10000);

    const searchCalls: string[] = [];
    const mockSearch = async (query: string, signal: AbortSignal): Promise<WikiQuickPickItem[]> => {
      searchCalls.push(query);
      // Simulate async search latency (e.g., spawn + wiki process running time)
      await new Promise((resolve) => setTimeout(resolve, 20));
      return signal.aborted ? [] : [{ label: query, file: `${query}.md` }];
    };

    const handler = createSearchHandler(
      mockSearch,
      () => {},
      () => {},
      () => {}
    );

    // Simulate rapid keystrokes — all fire synchronously, as onDidChangeValue
    // does per the VS Code API. With no debounce, each call immediately spawns
    // a search. A debounce timer would reset on each call and fire only once
    // after a ~150ms idle window.
    handler.onQueryChange('w');
    handler.onQueryChange('wi');
    handler.onQueryChange('wik');
    handler.onQueryChange('wiki');

    // Wait for all async operations to settle
    await new Promise((resolve) => setTimeout(resolve, 200));

    handler.dispose();

    // The desired behavior with debounce: only 1 search call.
    // The actual behavior without debounce: N calls for N keystrokes.
    // This assertion FAILS on the current code because no debounce exists.
    assert.strictEqual(
      searchCalls.length,
      1,
      `Expected 1 search call with debounce but got ${searchCalls.length}. ` +
        'Rapid keystrokes are not debounced, so each one immediately spawns a wiki process.'
    );
  });
});
