/**
 * Reproduction test for the QuickPick-show-timing bug.
 *
 * In the current code, `wikiQuickPick` calls `qp.show()` at L237, which runs
 * AFTER the `await Promise.all([loadValidatedRecentlyViewed(context),
 * loadAllPages(binaryPath)])` at L185. The QuickPick is created with
 * `qp.busy = true` at L182, but `show()` is never called until all async
 * initialization completes. The user sees nothing — no busy spinner, no
 * picker — during the entire data-load window.
 *
 * This test spies on `vscode.window.createQuickPick` to record the order of
 * `show()` vs `items=` operations, then asserts that `show()` is called
 * BEFORE the initial items are set. Against the current buggy code this
 * assertion MUST FAIL because `show()` (L237) runs after `items =` (L199).
 *
 * @summary Reproduction test: qp.show() called too late, after initial data load.
 * @module test/suite/wikiQuickPick.showTiming.test
 */

import * as assert from 'node:assert';
import * as vscode from 'vscode';
import { wikiQuickPick } from '../../src/commands/wikiQuickPick.js';
import type { WikiBinaryManager } from '../../src/utils/wikiInstaller.js';

// ── Stubs ──────────────────────────────────────────────────────────────────────

/** Minimal mock of WikiBinaryManager that resolves binary quickly. */
const mockBinaryManager = {
  ready: async () => ({ path: '/tmp/__wiki_show_timing_test__', source: 'path' as const }),
  formatFailure: (error: unknown) => String(error)
} satisfies Partial<WikiBinaryManager> as unknown as WikiBinaryManager;

class StubMemento implements vscode.Memento {
  private readonly _store = new Map<string, unknown>();

  keys(): readonly string[] {
    return Array.from(this._store.keys());
  }
  get<T>(key: string): T | undefined;
  get<T>(key: string, defaultValue: T): T;
  get<T>(key: string, defaultValue?: T): T | undefined {
    return this._store.has(key) ? (this._store.get(key) as T) : defaultValue;
  }
  async update(key: string, value: unknown): Promise<void> {
    if (value === undefined) this._store.delete(key);
    else this._store.set(key, value);
  }
  setKeysForSync(): void {
    /* no-op */
  }
}

function stubExtensionContext(): vscode.ExtensionContext {
  return { workspaceState: new StubMemento() } as unknown as vscode.ExtensionContext;
}

// ── Tests ──────────────────────────────────────────────────────────────────────

describe('wikiQuickPick — show timing (bug reproduction)', () => {
  let origCreateQuickPick: typeof vscode.window.createQuickPick;
  let origWithProgress: typeof vscode.window.withProgress;
  let currentQuickPick: vscode.QuickPick<vscode.QuickPickItem> | undefined;
  const callOrder: string[] = [];

  beforeEach(() => {
    callOrder.length = 0;
    currentQuickPick = undefined;

    // Bypass progress UI: just call the task function directly.
    origWithProgress = vscode.window.withProgress.bind(vscode.window);
    vscode.window.withProgress = ((_options: vscode.ProgressOptions, task: (...args: unknown[]) => Thenable<unknown>) =>
      Promise.resolve(task())) as unknown as typeof vscode.window.withProgress;

    // Spy on createQuickPick: intercept `show()` and the `items` setter to
    // record their relative call order.
    origCreateQuickPick = vscode.window.createQuickPick.bind(vscode.window);
    vscode.window.createQuickPick = (<T extends vscode.QuickPickItem>(_options?: vscode.InputBoxOptions) => {
      const qp = origCreateQuickPick<T>();

      // Save for cleanup.
      currentQuickPick = qp;

      // Intercept show().
      const origShow = qp.show.bind(qp);
      qp.show = () => {
        callOrder.push('show');
        return origShow();
      };

      // Intercept property sets for `items` and `busy` so the call-order
      // trace captures item assignment relative to show().
      return new Proxy(qp, {
        set(target, prop, value) {
          if (prop === 'items' || prop === 'busy') {
            callOrder.push(String(prop));
          }
          return Reflect.set(target, prop, value);
        }
      });
    }) as typeof vscode.window.createQuickPick;
  });

  afterEach(() => {
    vscode.window.createQuickPick = origCreateQuickPick;
    vscode.window.withProgress = origWithProgress;
    currentQuickPick?.dispose();
    currentQuickPick = undefined;
  });

  it('calls show() before setting initial items', async () => {
    await wikiQuickPick(mockBinaryManager, stubExtensionContext());

    const showIdx = callOrder.indexOf('show');
    const itemsIdx = callOrder.indexOf('items');

    assert.notStrictEqual(showIdx, -1, 'qp.show() was never called');
    assert.notStrictEqual(itemsIdx, -1, 'qp.items was never set');

    // The correct behaviour: show() must be called before items are set so
    // the user sees a QuickPick (with busy=true spinner) during loading.
    //
    // Current bug: callOrder is [..., 'items', ..., 'show'] because show()
    // at L237 runs after the await at L185. This assertion FAILS.
    const ok = showIdx < itemsIdx;
    assert.ok(
      ok,
      `Expected show() before items, but recorded order was: [${callOrder.join(', ')}]. ` +
        `show() index=${showIdx}, items index=${itemsIdx}. ` +
        'This confirms the bug: qp.show() at L237 runs after the await for initial data load at L185.'
    );
  });
});
