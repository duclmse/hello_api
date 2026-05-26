// sdk-ts/src/pm.js -- sandbox:pm Postman/Bruno compatibility shim
// Must be 7-bit ASCII only (no Unicode, no special characters outside ASCII 32-126).
// Ops are accessed via globalThis.__sandbox_ops (set by core.js before deleting Deno).
// Module-level mutable state is reset by results() for warm slot correctness.

import { makeChain } from "sandbox:pm-assert";
import { makeStore, headerGet, headerHas, replaceIn } from "sandbox:pm-helpers";

const ops = globalThis.__sandbox_ops;

// -- Module-level mutable state ------------------------------------------------
// These are reset by results() so warm pool slots start each run fresh.

let _pm_tests = [];
let _visualizer_template = null;
let _visualizer_data = null;

// Cannot reassign these objects (closures hold references), so results() clears keys.
let _env = {};      // pm.environment in-run delta
let _vars = {};     // pm.variables (request-scoped, NOT persisted)
let _col_vars = {}; // pm.collectionVariables in-run delta
let _globals = {};  // pm.globals in-run delta

// Lazy caches for env state injected by the runner before each script run (S4).
// The runner reads pm_env/pm_col_vars/pm_globals from results() and re-injects
// them as _pm_env/_pm_col_vars/_pm_globals sandbox inputs so stores persist
// across test cases in a collection run.  Reset to null in results() each run.
let _env_base = null;
let _col_vars_base = null;
let _globals_base = null;

function _loadEnvBase() {
  if (_env_base === null) {
    _env_base = sandbox.readInput("_pm_env") || {};
  }
  return _env_base;
}

function _loadColVarsBase() {
  if (_col_vars_base === null) {
    _col_vars_base = sandbox.readInput("_pm_col_vars") || {};
  }
  return _col_vars_base;
}

function _loadGlobalsBase() {
  if (_globals_base === null) {
    _globals_base = sandbox.readInput("_pm_globals") || {};
  }
  return _globals_base;
}

// Returns all variable stores in Postman scope-resolution order:
// local > collectionVariables > environment > globals.
// Calling the load functions here is safe: they are memoized for the current run.
function _varStores() {
  return [_vars, _col_vars, _loadColVarsBase(), _env, _loadEnvBase(), _globals, _loadGlobalsBase()];
}

function _varStoreGet(key) {
  let stores = _varStores();
  for (let i = 0; i < stores.length; i++) {
    if (Object.prototype.hasOwnProperty.call(stores[i], key)) {
      return stores[i][key];
    }
  }
  return undefined;
}

// Persistent store: reads in-run delta first, falls back to the persisted base.
// clear() resets the base via resetBase() so the snapshot in results() is empty.
function makePersistentStore(delta, loadBase, resetBase) {
  return {
    get: (key) => {
      if (Object.prototype.hasOwnProperty.call(delta, key)) return delta[key];
      let base = loadBase();
      return Object.prototype.hasOwnProperty.call(base, key) ? base[key] : undefined;
    },
    set: (key, val) => { delta[key] = val; },
    has: (key) => Object.prototype.hasOwnProperty.call(delta, key) ||
      Object.prototype.hasOwnProperty.call(loadBase(), key),
    unset: (key) => { delete delta[key]; },
    clear: () => {
      let ks = Object.keys(delta);
      for (let i = 0; i < ks.length; i++) delete delta[ks[i]];
      resetBase();
    },
  };
}

// -- Lazy response/request readers --------------------------------------------
// Must be lazy (getters, not init-time captures) because module is cached across
// warm pool slots and the inputs change each run.

function _getResponseRaw() {
  return sandbox.readInput("_response");
}

function _getRequestRaw() {
  return sandbox.readInput("_request");
}

function _makeHeaders(raw) {
  let headers = Array.isArray(raw.headers) ? raw.headers : [];
  return {
    get: name => headerGet(headers, name),
    has: name => headerHas(headers, name),
  };
}

// -- pm object -----------------------------------------------------------------

let pm = {
  test: (name, fn) => {
    let passed = true;
    try {
      fn();
    } catch (e) {
      passed = false;
    }
    ops.op_pm_test(passed, String(name));
    _pm_tests.push({ name: String(name), passed: passed });
  },

  expect: value => makeChain(value, false),

  get response() {
    let raw = _getResponseRaw();
    if (!raw) return null;
    return {
      get code() {
        return raw.status;
      },
      get status() {
        return raw.status;
      },
      get responseTime() {
        return raw.response_time_ms || 0;
      },
      get responseSize() {
        return typeof raw.body === "string" ? raw.body.length : 0;
      },
      get redirected() {
        return raw.redirected || false;
      },
      headers: _makeHeaders(raw),
      text: () => typeof raw.body === "string" ? raw.body : "",
      json: () => JSON.parse(raw.body || "null"),
      to: {
        have: {
          status: code => {
            if (raw.status !== code) throw new Error(`Expected status ${String(code)} but got ${String(raw.status)}`);
          },
        },
        be: {
          get ok() {
            if (!raw.ok) throw new Error(`Expected response to be ok but status was ${String(raw.status)}`);
          },
        },
      },
    };
  },

  get request() {
    let raw = _getRequestRaw();
    if (!raw) return null;
    return {
      get url() {
        return raw.url || "";
      },
      get method() {
        return raw.method || "GET";
      },
      headers: _makeHeaders(raw),
      get body() {
        return raw.body || null;
      },
    };
  },

  environment: makePersistentStore(_env, _loadEnvBase, () => { _env_base = {}; }),
  variables: {
    get: (key) => _varStoreGet(key),
    set: (key, val) => { _vars[key] = val; },
    has: (key) => _varStoresGet(key) !== undefined,
    unset: (key) => { delete _vars[key]; },
    replaceIn: (s) => replaceIn(s, _vars),
  },
  collectionVariables: Object.assign(
    makePersistentStore(_col_vars, _loadColVarsBase, () => { _col_vars_base = {}; }),
    { replaceIn: (s) => replaceIn(s, Object.assign({}, _loadColVarsBase(), _col_vars)) }
  ),
  globals: makePersistentStore(_globals, _loadGlobalsBase, () => { _globals_base = {}; }),

  // S2: info is a lazy getter so requestName reflects the _test tag for each run.
  get info() {
    let tags = sandbox.tags ? sandbox.tags() : {};
    return Object.freeze({
      eventName: "test",
      iterationCount: parseInt(tags["_iteration_count"] || "1", 10),
      iteration: parseInt(tags["_iteration"] || "0", 10),
      requestName: tags["_test"] || "",
      requestId: "",
    });
  },

  sendRequest: (request, callback) => {
    let opts;
    if (typeof request === "string") {
      opts = { url: request, method: "GET", headers: null, body: null };
    } else {
      let hdrs = null;
      if (Array.isArray(request.header)) {
        hdrs = request.header.map(h => [String(h.key), String(h.value)]);
      }
      opts = {
        url: String(request.url || ""),
        method: String(request.method || "GET"),
        headers: hdrs,
        body: request.body && request.body.mode === "raw" ? request.body.raw : null,
      };
    }
    return ops
      .op_pm_send_request(opts)
      .then(raw => {
        let hdrs = Array.isArray(raw.headers) ? raw.headers : [];
        let wrapped = {
          get code() {
            return raw.status;
          },
          get status() {
            return raw.status;
          },
          get ok() {
            return raw.ok;
          },
          get responseTime() {
            return raw.response_time_ms || 0;
          },
          headers: {
            get: n => headerGet(hdrs, n),
            has: n => headerHas(hdrs, n),
          },
          text: () => raw.body || "",
          json: () => JSON.parse(raw.body || "null"),
        };
        if (typeof callback === "function") callback(null, wrapped);
        return wrapped;
      })
      .catch(err => {
        if (typeof callback === "function") callback(err, null);
        throw err;
      });
  },

  visualizer: {
    set: (template, data) => {
      _visualizer_template = typeof template === "string" ? template : String(template);
      _visualizer_data = data !== undefined ? data : null;
    },
  },
};

// -- Bruno res / req / bru exports --------------------------------------------

let res = {
  get status() {
    let r = _getResponseRaw();
    return r ? r.status : null;
  },
  get body() {
    let r = _getResponseRaw();
    return r ? (typeof r.body === "string" ? r.body : "") : "";
  },
  get headers() {
    let r = _getResponseRaw();
    return r && Array.isArray(r.headers) ? r.headers : [];
  },
  getBody: () => {
    let r = _getResponseRaw();
    return r ? (typeof r.body === "string" ? r.body : "") : "";
  },
  getStatus: () => {
    let r = _getResponseRaw();
    return r ? r.status : null;
  },
  getResponseTime: () => {
    let r = _getResponseRaw();
    return r ? r.response_time_ms || 0 : 0;
  },
  getSize: () => {
    let r = _getResponseRaw();
    let b = r ? r.body : null;
    return typeof b === "string" ? b.length : 0;
  },
  getHeader: name => {
    let r = _getResponseRaw();
    return r ? headerGet(r.headers, name) : null;
  },
};

let req = {
  get url() {
    let r = _getRequestRaw();
    return r ? r.url || "" : "";
  },
  get method() {
    let r = _getRequestRaw();
    return r ? r.method || "GET" : "GET";
  },
  get headers() {
    let r = _getRequestRaw();
    return r && Array.isArray(r.headers) ? r.headers : [];
  },
  get body() {
    let r = _getRequestRaw();
    return r ? r.body || null : null;
  },
};

let bru = {
  getEnvVar: (key) => pm.environment.get(key),
  setEnvVar: (key, val) => pm.environment.set(key, val),
  deleteEnvVar: (key) => pm.environment.unset(key),
  getVar: (key) => makeStore(_vars).get(key),
  setVar: (key, val) => {
    _vars[key] = val;
  },
  // S3: reads the _env tag set by the runner (e.g. from import_dir_with_env)
  getEnvName: () => (sandbox.tags ? sandbox.tags()["_env"] : "") || "",
};

// -- Convenience exports -------------------------------------------------------

let test = (name, fn) => pm.test(name, fn);
let expect = value => pm.expect(value);

// -- results() -- resets all state for warm slot safety ------------------------

function results() {
  let tests = _pm_tests.slice();
  let failures = [];
  for (let i = 0; i < tests.length; i++) {
    if (!tests[i].passed) failures.push(tests[i].name);
  }
  let viz = _visualizer_template !== null
    ? { template: _visualizer_template, data: _visualizer_data }
    : null;

  // Capture merged env snapshots (persisted base + in-run changes) so the runner
  // can inject them before the next test case.  This is the S4 persistence mechanism:
  // the runner reads pm_env/pm_col_vars/pm_globals here and sets _pm_env etc. inputs
  // before every subsequent script run.
  let pm_env = Object.assign({}, _loadEnvBase(), _env);
  let pm_col_vars = Object.assign({}, _loadColVarsBase(), _col_vars);
  let pm_globals = Object.assign({}, _loadGlobalsBase(), _globals);

  // Reset all mutable state for warm-slot correctness
  _pm_tests = [];
  _visualizer_template = null;
  _visualizer_data = null;
  clearAllKeys(_env);
  clearAllKeys(_vars);
  clearAllKeys(_col_vars);
  clearAllKeys(_globals);
  _env_base = null;
  _col_vars_base = null;
  _globals_base = null;

  return Object.freeze({
    pass: failures.length === 0,
    failures: failures,
    visualizer: viz,
    pm_env: pm_env,
    pm_col_vars: pm_col_vars,
    pm_globals: pm_globals,
  });
}

function clearAllKeys(obj) {
  let keys = Object.keys(obj);
  for (let i = 0; i < keys.length; i++) {
    delete obj[keys[i]];
  }
}

export { pm, test, expect, res, req, bru, results };
