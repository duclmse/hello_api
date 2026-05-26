// sdk-ts/src/assert.js -- sandbox:assert formal assertion module
// Must be 7-bit ASCII only (no Unicode, no special characters outside ASCII 32-126).
// Assertions are backed by op_assert which records pass/fail counts in RunState
// and surfaces them in RunMetrics.assertions_passed / RunMetrics.assertions_failed.
// Unlike sandbox:test, no results() call is needed -- counts are automatic.

const ops = globalThis.__sandbox_ops;

function _assert(pass, message) {
  ops.op_assert(pass, message || "");
}

let assert = Object.freeze({
  /**
   * Assert that `value` is truthy.
   * @param {unknown} value
   * @param {string=} msg
   */
  ok: function(value, msg) {
    _assert(!!value, msg || `Expected truthy, got ${JSON.stringify(value)}`);
  },

  /**
   * Assert strict equality (`===`).
   * @param {unknown} actual
   * @param {unknown} expected
   * @param {string=} msg
   */
  equal: function(actual, expected, msg) {
    _assert(
      actual === expected,
      msg || `Expected ${JSON.stringify(actual)} to equal ${JSON.stringify(expected)}`
    );
  },

  /**
   * Assert strict inequality (`!==`).
   * @param {unknown} actual
   * @param {unknown} expected
   * @param {string=} msg
   */
  notEqual: function(actual, expected, msg) {
    _assert(
      actual !== expected,
      msg || `Expected ${JSON.stringify(actual)} to not equal ${JSON.stringify(expected)}`
    );
  },

  /**
   * Assert that string `haystack` contains `needle`, or array contains element.
   * @param {string|unknown[]} haystack
   * @param {unknown} needle
   * @param {string=} msg
   */
  contains: function(haystack, needle, msg) {
    let pass;
    if (typeof haystack === "string") {
      pass = haystack.indexOf(String(needle)) !== -1;
    } else if (Array.isArray(haystack)) {
      pass = haystack.indexOf(needle) !== -1;
    } else {
      pass = false;
    }
    _assert(
      pass,
      msg || `Expected ${JSON.stringify(haystack)} to contain ${JSON.stringify(needle)}`
    );
  },

  /**
   * Assert that `a > b`.
   * @param {number} a
   * @param {number} b
   * @param {string=} msg
   */
  greaterThan: function(a, b, msg) {
    _assert(a > b, msg || `Expected ${String(a)} > ${String(b)}`);
  },

  /**
   * Assert that `a < b`.
   * @param {number} a
   * @param {number} b
   * @param {string=} msg
   */
  lessThan: function(a, b, msg) {
    _assert(a < b, msg || `Expected ${String(a)} < ${String(b)}`);
  },

  /**
   * Assert that `value` is strictly `null`.
   * @param {unknown} value
   * @param {string=} msg
   */
  isNull: function(value, msg) {
    _assert(value === null, msg || `Expected null, got ${JSON.stringify(value)}`);
  },

  /**
   * Assert that `value` is strictly `undefined`.
   * @param {unknown} value
   * @param {string=} msg
   */
  isUndefined: function(value, msg) {
    _assert(
      typeof value === "undefined",
      msg || "Expected undefined"
    );
  },
});

export { assert };
