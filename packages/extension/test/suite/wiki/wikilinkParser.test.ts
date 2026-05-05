/**
 * Tests for parseQualifiedWikilink.
 *
 * Verifies parsing of qualified `[[ns:Title]]`, `[[ns:Title#fragment]]`,
 * and bare `[[Title]]` wikilinks.
 *
 * @summary Qualified wikilink parser tests.
 * @module test/suite/wiki/wikilinkParser.test
 */

import * as assert from 'node:assert';

describe('parseQualifiedWikilink', () => {
  it.skip('parses [[ns:Title]] into namespace and title', () => {
    assert.fail('Not Implemented');
  });

  it.skip('parses [[ns:Title#fragment]] into namespace, title, and fragment', () => {
    assert.fail('Not Implemented');
  });

  it.skip('parses bare [[Title]] with null namespace', () => {
    assert.fail('Not Implemented');
  });

  it.skip('parses [[Title|Display]] pipe syntax', () => {
    assert.fail('Not Implemented');
  });
});
