/**
 * Tests for NamespaceCache.
 *
 * Uses the real NamespaceCache implementation with a test fixture binary for
 * the `wiki namespaces` command.
 *
 * @summary NamespaceCache unit tests.
 * @module test/suite/wiki/namespaceCache.test
 */

import * as assert from 'node:assert';

describe('NamespaceCache', () => {
  it.skip('refreshes from wiki namespaces --format json', async () => {
    assert.fail('Not Implemented');
  });

  it.skip('maps null namespace to "default"', async () => {
    assert.fail('Not Implemented');
  });

  it.skip('returns WikiInfo by namespace name', () => {
    assert.fail('Not Implemented');
  });

  it.skip('returns all discovered namespaces', () => {
    assert.fail('Not Implemented');
  });

  it.skip('resolves namespace for a file path via longest-prefix match', () => {
    assert.fail('Not Implemented');
  });

  it.skip('surfaces non-zero exit as a workspace diagnostic', async () => {
    assert.fail('Not Implemented');
  });
});
