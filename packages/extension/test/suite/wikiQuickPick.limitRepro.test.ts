/**
 * Reproduction test: loadAllPages should list all pages without `--limit`.
 *
 * loadAllPages (wikiQuickPick.ts L94-L116) hardcodes `--limit 10` when calling
 * `wiki list`, which restricts the QuickPick to at most 10 pages. The module
 * JSDoc states an empty query "lists all pages" via `wiki list`, implying no
 * limit should be applied.
 *
 * This test reads the source file and asserts that `--limit` is not used in the
 * loadAllPages function. It will FAIL against the current unfixed code.
 *
 * @summary loadAllPages should not use --limit for listing all pages.
 * @module test/suite/wikiQuickPick.limitRepro.test
 */

import * as assert from 'node:assert';
import * as fs from 'node:fs';
import * as path from 'node:path';

describe('wikiQuickPick loadAllPages', () => {
  it('should not restrict results with --limit when listing all pages', () => {
    // __dirname is `{EXTENSION_ROOT}/dist-test-{pid}/test/suite/` at runtime.
    // From there, ../../../src/commands/wikiQuickPick.ts resolves to the
    // source file under test.
    const sourcePath = path.resolve(__dirname, '../../../src/commands/wikiQuickPick.ts');
    const source = fs.readFileSync(sourcePath, 'utf8');

    // Locate the loadAllPages function in the source
    const funcStart = source.indexOf('async function loadAllPages');
    assert.ok(funcStart >= 0, 'Could not find loadAllPages function in source');

    // Extract a window large enough to cover the full function body
    const searchRegion = source.slice(funcStart, funcStart + 500);

    // ASSERTION: --limit should not appear in loadAllPages.
    // This asserts the CORRECT behavior (list all pages without restriction).
    // It WILL FAIL on the current code because --limit 10 is hardcoded.
    assert.ok(
      !searchRegion.includes('--limit'),
      'loadAllPages should list all pages without a --limit restriction. ' +
        'Remove the `--limit` argument from the runWikiCommand call in loadAllPages.'
    );
  });
});
