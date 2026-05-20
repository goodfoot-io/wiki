/**
 * Unit tests for the wiki recently-viewed MRU module.
 *
 * Runs inside the Extension Host but uses a stub Memento for
 * `workspaceState` to exercise the push/dedupe/cap logic and the
 * filesystem-validated load in isolation.
 *
 * @summary recentlyViewed push/dedupe/cap + validation unit tests.
 * @module test/suite/recentlyViewed.test
 */

import * as assert from 'node:assert';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import * as path from 'node:path';
import type * as vscode from 'vscode';
import {
  loadValidatedRecentlyViewed,
  RECENTLY_VIEWED_CAP,
  RECENTLY_VIEWED_KEY,
  recordWikiView
} from '../../src/utils/recentlyViewed.js';

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

function stubContext(): vscode.ExtensionContext {
  return { workspaceState: new StubMemento() } as unknown as vscode.ExtensionContext;
}

function wikiPage(title: string, summary: string): string {
  return `---\ntitle: ${title}\nsummary: ${summary}\n---\n\nbody\n`;
}

describe('recentlyViewed — recordWikiView', () => {
  it('pushes a new entry to the front', async () => {
    const ctx = stubContext();
    await recordWikiView(ctx, '/a.md');
    await recordWikiView(ctx, '/b.md');
    assert.deepStrictEqual(ctx.workspaceState.get(RECENTLY_VIEWED_KEY), ['/b.md', '/a.md']);
  });

  it('dedupes by absolute path, moving prior entry to front', async () => {
    const ctx = stubContext();
    await recordWikiView(ctx, '/a.md');
    await recordWikiView(ctx, '/b.md');
    await recordWikiView(ctx, '/a.md');
    assert.deepStrictEqual(ctx.workspaceState.get(RECENTLY_VIEWED_KEY), ['/a.md', '/b.md']);
  });

  it('truncates to RECENTLY_VIEWED_CAP', async () => {
    const ctx = stubContext();
    for (let i = 0; i < RECENTLY_VIEWED_CAP + 5; i++) {
      await recordWikiView(ctx, `/p${i}.md`);
    }
    const stored = ctx.workspaceState.get<string[]>(RECENTLY_VIEWED_KEY)!;
    assert.strictEqual(stored.length, RECENTLY_VIEWED_CAP);
    assert.strictEqual(stored[0], `/p${RECENTLY_VIEWED_CAP + 4}.md`);
  });
});

describe('recentlyViewed — loadValidatedRecentlyViewed', () => {
  let dir: string;

  beforeEach(async () => {
    dir = await mkdtemp(path.join(tmpdir(), 'wiki-mru-'));
  });

  afterEach(async () => {
    await rm(dir, { recursive: true, force: true });
  });

  it('returns empty when nothing is stored', async () => {
    const ctx = stubContext();
    assert.deepStrictEqual(await loadValidatedRecentlyViewed(ctx), []);
  });

  it('keeps entries that resolve to wiki pages, in stored order', async () => {
    const a = path.join(dir, 'a.md');
    const b = path.join(dir, 'b.md');
    await writeFile(a, wikiPage('Alpha', 'aaa'));
    await writeFile(b, wikiPage('Beta', 'bbb'));
    const ctx = stubContext();
    await ctx.workspaceState.update(RECENTLY_VIEWED_KEY, [b, a]);
    const items = await loadValidatedRecentlyViewed(ctx);
    assert.deepStrictEqual(
      items.map((i) => i.label),
      ['Beta', 'Alpha']
    );
    assert.deepStrictEqual(
      items.map((i) => i.file),
      [b, a]
    );
    assert.strictEqual(items[0]!.detail, 'bbb');
  });

  it('drops missing files and persists the truncated list', async () => {
    const a = path.join(dir, 'a.md');
    await writeFile(a, wikiPage('Alpha', 'aaa'));
    const ctx = stubContext();
    await ctx.workspaceState.update(RECENTLY_VIEWED_KEY, [path.join(dir, 'missing.md'), a]);
    const items = await loadValidatedRecentlyViewed(ctx);
    assert.deepStrictEqual(
      items.map((i) => i.file),
      [a]
    );
    assert.deepStrictEqual(ctx.workspaceState.get(RECENTLY_VIEWED_KEY), [a]);
  });

  it('drops files that no longer have wiki frontmatter', async () => {
    const a = path.join(dir, 'a.md');
    const b = path.join(dir, 'b.md');
    await writeFile(a, wikiPage('Alpha', 'aaa'));
    await writeFile(b, '# Plain markdown — no frontmatter\n');
    const ctx = stubContext();
    await ctx.workspaceState.update(RECENTLY_VIEWED_KEY, [a, b]);
    const items = await loadValidatedRecentlyViewed(ctx);
    assert.deepStrictEqual(
      items.map((i) => i.file),
      [a]
    );
    assert.deepStrictEqual(ctx.workspaceState.get(RECENTLY_VIEWED_KEY), [a]);
  });

  it('does not rewrite storage when nothing is dropped', async () => {
    const a = path.join(dir, 'a.md');
    await writeFile(a, wikiPage('Alpha', 'aaa'));
    const ctx = stubContext();
    await ctx.workspaceState.update(RECENTLY_VIEWED_KEY, [a]);
    const before = ctx.workspaceState.get<string[]>(RECENTLY_VIEWED_KEY);
    await loadValidatedRecentlyViewed(ctx);
    assert.strictEqual(ctx.workspaceState.get<string[]>(RECENTLY_VIEWED_KEY), before);
  });
});
