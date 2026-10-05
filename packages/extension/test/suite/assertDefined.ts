/**
 * Assertion helper that narrows away `null`/`undefined` for test code.
 *
 * @summary Fail-closed non-null assertion for test suites.
 */

import * as assert from 'node:assert';

/**
 * Assert that `value` is present and return it with `null`/`undefined`
 * removed from its type, so tests narrow by checking rather than asserting.
 *
 * @param value - The value that must be present.
 * @param message - Failure message naming what was expected.
 * @returns `value`, narrowed to a non-nullable type.
 */
export function assertDefined<T>(value: T, message: string): NonNullable<T> {
  assert.ok(value !== undefined && value !== null, message);
  return value;
}
