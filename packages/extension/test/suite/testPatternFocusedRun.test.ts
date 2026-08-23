/**
 * Regression tests for TEST_PATTERN focused runs of the Mocha bootstrap.
 *
 * Pins the contract of {@link run}: a TEST_PATTERN that matches zero compiled
 * suites must reject (surfacing as a non-zero exit instead of a silent green
 * build), while a bare suite name — without the `suite/` directory prefix —
 * must still resolve because the runner normalizes it against the compiled
 * layout (`{dist}/test/suite/*.test.cjs`).
 *
 * The second half of the contract self-references this file's compiled name
 * (`testPatternFocusedRun.test.cjs`), which guarantees the probe target exists
 * without coupling to any other suite.
 *
 * @summary Pins reject-on-zero-match and bare-name normalization for TEST_PATTERN.
 * @module test/suite/testPatternFocusedRun.test
 */

import * as assert from 'node:assert';
import { run } from './index.js';

describe('TEST_PATTERN focused runs', () => {
  const originalPattern = process.env['TEST_PATTERN'];

  afterEach(() => {
    if (originalPattern === undefined) {
      delete process.env['TEST_PATTERN'];
    } else {
      process.env['TEST_PATTERN'] = originalPattern;
    }
  });

  it('rejects zero-match patterns and accepts valid bare suite names', async () => {
    // A pattern that cannot match any compiled suite must fail loudly rather
    // than report "0 passing" and exit 0.
    process.env['TEST_PATTERN'] = 'zebraNoMatchingSuite';
    await assert.rejects(() => run(), /TEST_PATTERN/i);

    // A bare file name (no `suite/` prefix) must be normalized so it matches
    // this very suite under the compiled test root.
    process.env['TEST_PATTERN'] = 'testPatternFocusedRun';
    await assert.doesNotReject(() => run());
  });
});
