/**
 * OpenCode's module-plugin contract requires a default-exported object with a
 * function `server` and optional string `id`. Pinning the entry shape prevents
 * the host from falling back to scanning helper exports as plugin functions.
 */

import { describe, expect, it } from 'vitest';
import { default as moduleDefault, wikiOpencode } from '../../src/opencode/index.js';

describe('opencode plugin module shape', () => {
  it('default-exports an object with the server initializer and string id', () => {
    expect(moduleDefault).toBeTypeOf('object');
    expect(moduleDefault).not.toBeNull();
    expect(moduleDefault.id).toBe('@goodfoot/opencode-wiki');
    expect(moduleDefault.server).toBe(wikiOpencode);
  });
});
