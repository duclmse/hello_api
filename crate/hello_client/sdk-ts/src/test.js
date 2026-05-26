// sdk-ts/src/test.js -- sandbox:test assertion library
// Must be 7-bit ASCII only (no Unicode, no special characters outside ASCII 32-126).
// This module is registered as a user module (not an extension ESM), so it runs
// AFTER core.js has deleted globalThis.Deno. Ops are accessed via __sandbox_ops.

const ops = globalThis.__sandbox_ops;

let _failures = [];

function _push(msg) {
  _failures.push(msg);
  try {
    ops.op_emit("test_failure", JSON.stringify({ message: msg }));
  } catch (_e) {
    // emit may be disabled or rate-limited; failure is still recorded in _failures
  }
}

function _deepEq(a, b) {
  return JSON.stringify(a) === JSON.stringify(b);
}

class Assertion {
  constructor(actual, negated) {
    this._actual = actual;
    this._negated = !!negated;
    if (!negated) {
      this.not = new Assertion(actual, true);
    }
  }

  _check(pass, failMsg) {
    const ok = this._negated ? !pass : pass;
    if (!ok) {
      _push(this._negated ? "not: " + failMsg : failMsg);
    }
  }

  toBe(expected, msg) {
    this._check(
      this._actual === expected,
      msg || `Expected ${JSON.stringify(this._actual)} to be ${JSON.stringify(expected)}`
    );
    return this;
  }

  toEqual(expected, msg) {
    this._check(
      _deepEq(this._actual, expected),
      msg || `Expected ${JSON.stringify(this._actual)} to equal ${JSON.stringify(expected)}`
    );
    return this;
  }

  toContain(item, msg) {
    let pass;
    if (typeof this._actual === "string") {
      pass = this._actual.includes(String(item));
    } else if (Array.isArray(this._actual)) {
      pass = this._actual.some(function (el) {
        return _deepEq(el, item);
      });
    } else {
      pass = false;
    }
    this._check(pass, msg || `Expected ${JSON.stringify(this._actual)} to contain ${JSON.stringify(item)}`);
    return this;
  }

  toBeTruthy(msg) {
    this._check(!!this._actual, msg || `Expected ${JSON.stringify(this._actual)} to be truthy`);
    return this;
  }

  toBeFalsy(msg) {
    this._check(!this._actual, msg || `Expected ${JSON.stringify(this._actual)} to be falsy`);
    return this;
  }

  toBeNull(msg) {
    this._check(this._actual === null, msg || `Expected ${JSON.stringify(this._actual)} to be null`);
    return this;
  }

  toBeUndefined(msg) {
    this._check(typeof this._actual === "undefined", msg || `Expected ${JSON.stringify(this._actual)} to be undefined`);
    return this;
  }

  toBeGreaterThan(n, msg) {
    this._check(
      this._actual > n,
      msg || `Expected ${JSON.stringify(this._actual)} to be greater than ${JSON.stringify(n)}`
    );
    return this;
  }

  toBeLessThan(n, msg) {
    this._check(
      this._actual < n,
      msg || `Expected ${JSON.stringify(this._actual)} to be less than ${JSON.stringify(n)}`
    );
    return this;
  }
}

/**
 * Create a chainable assertion for the given actual value.
 * @param {unknown} actual
 * @returns {Assertion}
 */
function expect(actual) {
  return new Assertion(actual, false);
}

/**
 * Wrap a raw HTTP response object (as set in the _response sandbox input)
 * for easy assertion in post-scripts.
 *
 * @param {{ status: number, ok: boolean, headers: [string,string][], body: string, response_time_ms: number }} raw
 */
function wrapResponse(raw) {
  const headers = Array.isArray(raw.headers) ? raw.headers : [];

  const hdrs = {
    get: function (name) {
      const lower = String(name).toLowerCase();
      for (let i = 0; i < headers.length; i++) {
        if (String(headers[i][0]).toLowerCase() === lower) {
          return headers[i][1];
        }
      }
      return null;
    },
    has: function (name) {
      const lower = String(name).toLowerCase();
      for (let i = 0; i < headers.length; i++) {
        if (String(headers[i][0]).toLowerCase() === lower) {
          return true;
        }
      }
      return false;
    },
  };

  return {
    status: raw.status,
    ok: raw.ok,
    responseTime: raw.response_time_ms,
    size: typeof raw.body === "string" ? raw.body.length : 0,
    redirected: raw.redirected || false,
    headers: hdrs,
    text: function () {
      return typeof raw.body === "string" ? raw.body : "";
    },
    json: function () {
      return JSON.parse(raw.body || "null");
    },
  };
}

/**
 * Return the current test results and reset the failure accumulator for the
 * next run. Always call this as the final `return` in a post-script.
 *
 * @returns {{ pass: boolean, failures: string[] }}
 */
function results() {
  const current = _failures.slice();
  _failures = [];
  return Object.freeze({ pass: current.length === 0, failures: current });
}

export { expect, wrapResponse, results };
