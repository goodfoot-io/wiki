/**
 * Regression test for workspace-root-absolute (`/`-rooted) link resolution.
 *
 * ## Bug
 * The authoritative Rust resolver `resolve_link_path`
 * (packages/cli/src/commands/mod.rs:123) treats a `/`-prefixed link as
 * workspace/repo-root-relative: it strips the leading `/` and resolves the
 * remainder against the repo root. The two TypeScript resolvers instead
 * treated a leading `/` as a filesystem-absolute path
 * (`path.isAbsolute(rawPath)` is true on POSIX), so a link like
 * `/packages/cli/src/commands/mod.rs#L123` resolved to the literal
 * filesystem path `/packages/...` — which does not exist.
 *
 * ## What this test covers
 * The language-feature resolver must mirror the Rust convention: strip the
 * leading `/` and resolve against the workspace root while leaving relative
 * and external links unchanged.
 *
 * @summary Regression test for `/`-rooted link resolution in both TS resolvers.
 * @module test/suite/linkResolution.slashRooted.test
 */

import * as assert from 'node:assert';
import * as path from 'node:path';
import { resolveLinkTarget } from '../../src/providers/WikiLanguageFeatures.js';

const WS = path.sep === '\\' ? 'C:\\ws' : '/ws';
const SRC_FILE = path.join(WS, 'wiki', 'page.md');

describe('WikiLanguageFeatures.resolveLinkTarget — /-rooted links', () => {
  it('resolves a /-rooted link against the workspace root', () => {
    const resolved = resolveLinkTarget('/packages/cli/src/commands/mod.rs#L123', SRC_FILE, WS);
    assert.strictEqual(resolved, path.normalize(path.join(WS, 'packages', 'cli', 'src', 'commands', 'mod.rs')));
  });

  it('still resolves a relative link against the linking file directory', () => {
    const resolved = resolveLinkTarget('../other/x.md', SRC_FILE, WS);
    assert.strictEqual(resolved, path.normalize(path.join(WS, 'other', 'x.md')));
  });

  it('returns null for external links', () => {
    assert.strictEqual(resolveLinkTarget('https://example.com', SRC_FILE, WS), null);
  });
});
