// sdk-ts/src/pm-assert.js -- Chai-like assertion chain for sandbox:pm
// Must be 7-bit ASCII only (no Unicode, no special characters outside ASCII 32-126).

function deepEq(a, b) {
  if (a === b) return true;
  if (a === null || b === null || typeof a !== typeof b) return false;
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  if (Array.isArray(a)) {
    if (a.length !== b.length) return false;
    for (let i = 0; i < a.length; i++) {
      if (!deepEq(a[i], b[i])) return false;
    }
    return true;
  }
  if (typeof a === "object") {
    let ka = Object.keys(a).sort();
    let kb = Object.keys(b).sort();
    if (ka.length !== kb.length) return false;
    for (let j = 0; j < ka.length; j++) {
      if (ka[j] !== kb[j] || !deepEq(a[ka[j]], b[kb[j]])) return false;
    }
    return true;
  }
  return false;
}

function makeChain(value, negated) {
  let chain = {};

  let fluent = [
    "to",
    "be",
    "been",
    "is",
    "have",
    "that",
    "which",
    "and",
    "has",
    "with",
    "at",
    "of",
    "same",
    "but",
    "does",
  ];
  for (let _i = 0; _i < fluent.length; _i++) {
    (word => {
      Object.defineProperty(chain, word, { get: () => chain, enumerable: true });
    })(fluent[_i]);
  }

  Object.defineProperty(chain, "not", {
    get: () => makeChain(value, !negated),
    enumerable: true,
  });

  function _assert(pass, failMsg) {
    let ok = negated ? !pass : pass;
    if (!ok) throw new Error(negated ? "not: " + failMsg : failMsg);
  }

  Object.defineProperty(chain, "ok", {
    get: () => {
      _assert(!!value, `Expected truthy value, got ${JSON.stringify(value)}`);
      return chain;
    },
    enumerable: true,
  });

  Object.defineProperty(chain, "true", {
    get: () => {
      _assert(value === true, `Expected true, got ${JSON.stringify(value)}`);
      return chain;
    },
    enumerable: true,
  });

  Object.defineProperty(chain, "false", {
    get: () => {
      _assert(value === false, `Expected false, got ${JSON.stringify(value)}`);
      return chain;
    },
    enumerable: true,
  });

  Object.defineProperty(chain, "null", {
    get: () => {
      _assert(value === null, `Expected null, got ${JSON.stringify(value)}`);
      return chain;
    },
    enumerable: true,
  });

  Object.defineProperty(chain, "undefined", {
    get: () => {
      _assert(typeof value === "undefined", `Expected undefined, got ${JSON.stringify(value)}`);
      return chain;
    },
    enumerable: true,
  });

  Object.defineProperty(chain, "empty", {
    get: () => {
      let isEmpty =
        value == null ||
        value === "" ||
        (Array.isArray(value) && value.length === 0) ||
        (typeof value === "object" && !Array.isArray(value) && Object.keys(value).length === 0);
      _assert(isEmpty, `Expected empty, got ${JSON.stringify(value)}`);
      return chain;
    },
    enumerable: true,
  });

  chain.equal = expected => {
    _assert(value === expected, `Expected ${JSON.stringify(value)} to equal ${JSON.stringify(expected)}`);
    return chain;
  };

  chain.eql = expected => {
    _assert(deepEq(value, expected), `Expected ${JSON.stringify(value)} to deeply equal ${JSON.stringify(expected)}`);
    return chain;
  };

  chain.include = item => {
    let pass;
    if (typeof value === "string") {
      pass = value.indexOf(String(item)) !== -1;
    } else if (Array.isArray(value)) {
      pass = value.some(el => deepEq(el, item));
    } else if (value !== null && typeof value === "object") {
      pass = Object.prototype.hasOwnProperty.call(value, item);
    } else {
      pass = false;
    }
    _assert(pass, `Expected ${JSON.stringify(value)} to include ${JSON.stringify(item)}`);
    return chain;
  };

  chain.contain = chain.include;

  chain.a = type => {
    let actual = Array.isArray(value) ? "array" : typeof value;
    _assert(actual === type, `Expected ${JSON.stringify(value)} to be a ${type} but got ${actual}`);
    return chain;
  };
  chain.an = chain.a;

  chain.above = n => {
    _assert(value > n, `Expected ${String(value)} to be above ${String(n)}`);
    return chain;
  };
  chain.greaterThan = chain.above;

  chain.below = n => {
    _assert(value < n, `Expected ${String(value)} to be below ${String(n)}`);
    return chain;
  };
  chain.lessThan = chain.below;

  chain.least = n => {
    _assert(value >= n, `Expected ${String(value)} to be at least ${String(n)}`);
    return chain;
  };

  chain.most = n => {
    _assert(value <= n, `Expected ${String(value)} to be at most ${String(n)}`);
    return chain;
  };

  chain.property = function (key, val) {
    let hasProp = value !== null && typeof value === "object" && Object.prototype.hasOwnProperty.call(value, key);
    _assert(hasProp, `Expected object to have property ${JSON.stringify(key)}`);
    if (arguments.length >= 2) {
      _assert(
        deepEq(value[key], val),
        `Expected property ${JSON.stringify(key)} to equal ${JSON.stringify(val)} but got ${JSON.stringify(value[key])}`
      );
    }
    return chain;
  };

  chain.lengthOf = n => {
    let len = value != null ? value.length : undefined;
    _assert(len === n, `Expected length ${String(n)} but got ${String(len)}`);
    return chain;
  };

  chain.match = re => {
    _assert(re.test(String(value)), `Expected ${JSON.stringify(value)} to match ${String(re)}`);
    return chain;
  };

  chain.startsWith = s => {
    _assert(String(value).indexOf(s) === 0, `Expected ${JSON.stringify(value)} to start with ${JSON.stringify(s)}`);
    return chain;
  };

  chain.endsWith = s => {
    let str = String(value);
    _assert(
      str.lastIndexOf(s) === str.length - s.length,
      `Expected ${JSON.stringify(value)} to end with ${JSON.stringify(s)}`
    );
    return chain;
  };

  chain.status = code => {
    let actual = value !== null && typeof value === "object" ? value.code || value.status : value;
    _assert(actual === code, `Expected status ${String(code)} but got ${String(actual)}`);
    return chain;
  };

  return chain;
}

export { makeChain };
