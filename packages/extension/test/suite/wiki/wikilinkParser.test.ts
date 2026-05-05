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
import { parseQualifiedWikilink } from '../../../src/wiki/wikilinkParser.js';

describe('parseQualifiedWikilink', () => {
  it('parses [[ns:Title]] into namespace and title', () => {
    const result = parseQualifiedWikilink('ns:Title');
    assert.strictEqual(result.namespace, 'ns');
    assert.strictEqual(result.title, 'Title');
    assert.strictEqual(result.fragment, null);
  });

  it('parses [[ns:Title#fragment]] into namespace, title, and fragment', () => {
    const result = parseQualifiedWikilink('ns:Title#fragment');
    assert.strictEqual(result.namespace, 'ns');
    assert.strictEqual(result.title, 'Title');
    assert.strictEqual(result.fragment, 'fragment');
  });

  it('parses bare [[Title]] with null namespace', () => {
    const result = parseQualifiedWikilink('Title');
    assert.strictEqual(result.namespace, null);
    assert.strictEqual(result.title, 'Title');
    assert.strictEqual(result.fragment, null);
  });

  it('parses [[Title|Display]] pipe syntax', () => {
    const result = parseQualifiedWikilink('Title|Display');
    assert.strictEqual(result.namespace, null);
    assert.strictEqual(result.title, 'Title');
    assert.strictEqual(result.fragment, null);
  });

  it('parses [[ns:Title|Display]] with namespace and pipe', () => {
    const result = parseQualifiedWikilink('ns:Title|Display');
    assert.strictEqual(result.namespace, 'ns');
    assert.strictEqual(result.title, 'Title');
    assert.strictEqual(result.fragment, null);
  });

  it('parses [[Title#fragment|Display]] with fragment and pipe', () => {
    const result = parseQualifiedWikilink('Title#fragment|Display');
    assert.strictEqual(result.namespace, null);
    assert.strictEqual(result.title, 'Title');
    assert.strictEqual(result.fragment, 'fragment');
  });
});
