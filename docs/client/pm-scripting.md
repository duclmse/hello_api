# sandbox:pm — Shared Scripting Library

Both adapters wrap imported scripts to import from `sandbox:pm`. This module
must be registered with the sandbox before running adapted scripts.

`HttpTestRunner` registers `sandbox:pm` automatically via `PmPack`. When using
the sandbox directly:

```rust
use hello_sandbox::{SandboxBuilder, SandboxConfig, PmPack};

let sandbox = SandboxBuilder::new()
    .config(SandboxConfig::power_user())
    .sdk(PmPack)
    .build()
    .unwrap();
```

**Import statement:**

```js
// Pre-request scripts:
import { pm, bru, req } from "sandbox:pm";

// Post-request scripts:
import { pm, res, bru, test, expect, results } from "sandbox:pm";
```

## 4.1 pm object

```ts
interface Pm {
  test(name: string, fn: () => void): void;
  expect(value: unknown): Assertion;
  readonly response: PmResponse | null;
  readonly request: PmRequest | null;
  environment: Store;
  variables: Store;
  collectionVariables: Store; // alias for variables
  globals: Store;
  get info(): PmInfo; // lazy getter — reads per-run sandbox tags
  sendRequest(request: string | object, callback?: (err: any, res: any) => void): Promise<any>;
  visualizer: { set(template: string, data?: unknown): void };
}
```

**`pm.test(name, fn)`** — Runs `fn()`. If it throws, the test is marked failed.
Result is recorded in `RunMetrics.pm_tests` and also returned by `results()`.

**`pm.info`** — Per-run metadata read lazily from sandbox tags:
`{ eventName: 'test', iterationCount: 1, iteration: 0, requestName: <test name from _test tag>, requestId: '' }`.
The `requestName` is populated from the `_test` tag injected by `HttpTestRunner`
for each run; `iterationCount` and `iteration` are read from `_iteration_count`
and `_iteration` tags respectively.

## 4.2 pm.expect() assertion chain

Chai-compatible assertion chain. Fluent chain words (`to`, `be`, `have`, `that`,
`which`, `and`, `has`, `with`, `at`, `of`, `same`, `but`, `does`, `been`, `is`)
are no-ops that return `this` for readability.

**Property assertions (getters — no `()`):**

```js
pm.expect(value).to.be.ok; // truthy
pm.expect(value).to.be.true; // === true
pm.expect(value).to.be.false; // === false
pm.expect(value).to.be.null; // === null
pm.expect(value).to.be.undefined; // typeof === "undefined"
pm.expect(value).to.be.empty; // "", [], {}, or null/undefined
```

**Method assertions:**

```js
pm.expect(x).to.equal(y); // strict ===
pm.expect(x).to.eql(y); // deep equality (JSON.stringify)
pm.expect(x).to.include(item); // string contains, array contains, object has key
pm.expect(x).to.contain(item); // alias for include
pm.expect(x).to.a("string"); // typeof check (or "array")
pm.expect(x).to.an("object");
pm.expect(x).to.above(n); // x > n
pm.expect(x).to.greaterThan(n);
pm.expect(x).to.below(n); // x < n
pm.expect(x).to.lessThan(n);
pm.expect(x).to.least(n); // x >= n
pm.expect(x).to.most(n); // x <= n
pm.expect(x).to.property("key"); // object has own property
pm.expect(x).to.property("key", val); // ... with specific value
pm.expect(x).to.lengthOf(n); // .length === n
pm.expect(x).to.match(/regex/); // regex test
pm.expect(x).to.startsWith("s");
pm.expect(x).to.endsWith("s");
pm.expect(x).to.status(code); // .status or .code === code
```

**Negation:** Prepend `.not` to invert any assertion:

```js
pm.expect(x).to.not.equal(y);
pm.expect(x).to.not.include(item);
pm.expect(x).to.not.be.null;
```

**Failure behaviour:** A failed assertion throws a `new Error(message)`. When
called inside `pm.test()`, the error is caught and the test is recorded as
failed. When called outside `pm.test()`, the error propagates and terminates the
script — the run returns `SandboxError::Runtime`.

## 4.3 pm.response

Available in post-request scripts. Read lazily on first access from the
`_response` sandbox input (set by `HttpTestRunner`). Returns `null` if no
response input is present.

```ts
interface PmResponse {
  readonly code: number; // HTTP status code
  readonly status: number; // alias for code
  readonly responseTime: number; // milliseconds
  readonly responseSize: number; // body byte length
  readonly redirected: boolean;
  readonly headers: {
    get(name: string): string | null; // case-insensitive
    has(name: string): boolean;
  };
  text(): string;
  json(): unknown; // JSON.parse(body)
  // Postman-style shorthand assertions:
  to: { have: { status(code: number): void }; be: { ok: void } };
}
```

**Headers are case-insensitive:**

```js
pm.response.headers.get("Content-Type"); // same as
pm.response.headers.get("content-type");
```

**Shorthand assertion syntax:**

```js
pm.response.to.have.status(200); // throws if status !== 200
pm.response.to.be.ok; // throws if status not ok (2xx implied by ok flag)
```

## 4.4 pm.request

Available in both pre- and post-request scripts. Reads lazily from the
`_request` sandbox input.

```ts
interface PmRequest {
  readonly url: string;
  readonly method: string;
  readonly headers: {
    get(name: string): string | null;
    has(name: string): boolean;
  };
  readonly body: string | null;
}
```

## 4.5 Store API

Three in-memory stores: `environment`, `variables` (aliased as
`collectionVariables`), and `globals`. All share the same interface:

```ts
interface Store {
  get(key: string): unknown; // undefined if not set
  set(key: string, value: unknown): void;
  has(key: string): boolean;
  unset(key: string): void;
  clear(): void;
}
```

**Scope semantics:**

| Store                | Within a `run_collection` call | Across `run_collection` calls |
| -------------------- | ------------------------------ | ----------------------------- |
| `environment`        | persists across tests          | reset at each collection start |
| `collectionVariables`| persists across tests          | reset at each collection start |
| `variables`          | request-scoped (not persisted) | not persisted                  |
| `globals`            | persists across tests          | persists across collections    |

`HttpTestRunner` captures the store snapshots returned by `results()` and
re-injects them as sandbox inputs before each subsequent script in the same
collection run. `pm.globals` is never reset — it accumulates across all
`run_collection` calls on the same runner instance.

## 4.6 Bruno compat exports

`sandbox:pm` also exports Bruno-compatible objects for scripts written in Bruno
style:

**`res`** — Response accessor:

```ts
interface BruResponse {
  readonly status: number | null;
  readonly body: string;
  readonly headers: [string, string][];
  getBody(): string;
  getStatus(): number | null;
  getResponseTime(): number;
  getSize(): number;
  getHeader(name: string): string | null; // case-insensitive
}
```

**`req`** — Request accessor:

```ts
interface BruRequest {
  readonly url: string;
  readonly method: string;
  readonly headers: [string, string][];
  readonly body: string | null;
}
```

**`bru`** — Environment utility:

```ts
interface BruStore {
  getEnvVar(key: string): unknown;
  setEnvVar(key: string, value: unknown): void;
  deleteEnvVar(key: string): void;
  getVar(key: string): unknown;
  setVar(key: string, value: unknown): void;
  getEnvName(): string; // returns the _env tag value set by the runner, or "" if unset
}
```

Note: `bru.getEnvVar` / `bru.setEnvVar` share the same backing store as
`pm.environment`.

`bru.getEnvName()` returns the `_env` tag value set by the runner (e.g. via
`import_dir_with_env`), or `""` if unset.

**`test(name, fn)`** — Global alias for `pm.test()`.

**`expect(value)`** — Global alias for `pm.expect()`.

## 4.7 results()

```ts
function results(): {
  pass: boolean;
  failures: string[];
  pm_env: Record<string, unknown>;
  pm_col_vars: Record<string, unknown>;
  pm_globals: Record<string, unknown>;
};
```

**Must be called as the last `return` statement in every post-request script.**

- Collects all `pm.test()` results recorded during the run.
- `pass: true` if zero tests failed (also true when zero tests ran).
- `failures`: names of failed tests.
- `pm_env`, `pm_col_vars`, `pm_globals`: merged snapshots of the three
  persistent stores (base + in-run deltas). `HttpTestRunner` captures these and
  re-injects them before the next script via `set_input("_pm_env", ...)` etc.
- **Resets** all module-level state (tests, deltas, and lazy base caches) so
  the warm pool slot starts the next run clean.

The result object is frozen (`Object.freeze`).

```js
// Canonical post-script footer:
return results();
```

## 4.8 Warm-slot safety

The sandbox reuses V8 isolates across multiple `run()` calls on the same pool
slot. ESM module top-level code runs **once** and is cached. This has three
implications for `sandbox:pm`:

1. **`pm.response` and `pm.request` use lazy getters** — they call
   `sandbox.readInput()` at access time, not at module load time. Each run
   provides fresh input via `set_input("_response", ...)`.

2. **Persistent store bases use a null-sentinel lazy cache** — `_env_base`,
   `_col_vars_base`, and `_globals_base` are initialized to `null` and loaded
   from `sandbox.readInput("_pm_env" / "_pm_col_vars" / "_pm_globals")` on
   first access each run. `results()` resets them back to `null` so the next
   run reads fresh injected state.

3. **`results()` must be called to reset state** — it clears `_pm_tests`, all
   in-run deltas, and the lazy base caches so accumulated state from a previous
   run does not bleed into the next run.

If a post-script throws before reaching `return results()`, the state will
**not** be reset. The slot is marked `Stale` on error and recycled, so this does
not cause cross-run contamination in practice.

## 4.9 pm.sendRequest()

Post-scripts and pre-scripts can make asynchronous outbound HTTP requests using `pm.sendRequest`. This is useful for retrieving auth tokens, doing pre-request setup, or sending notifications/telemetry.

**Signature:**
```js
pm.sendRequest(request: string | object, callback?: (error: any, response: any) => void): Promise<any>
```

- If `request` is a string, it is treated as a `GET` request to that URL.
- If `request` is an object, it can specify:
  - `url`: target URL string
  - `method`: HTTP method (e.g., `GET`, `POST`)
  - `header`: array of `{ key: string, value: string }` objects
  - `body`: object with `{ mode: "raw", raw: string }`
- The `callback` is called with `(error, response)` upon completion.
- It also returns a `Promise` that resolves to the wrapped response, allowing `await` usage in `async` contexts.

**Example:**
```js
pm.sendRequest({
  url: "https://api.example.com/oauth/token",
  method: "POST",
  header: [{ key: "Content-Type", value: "application/json" }],
  body: {
    mode: "raw",
    raw: JSON.stringify({ client_id: "foo", client_secret: "bar" })
  }
}, function (err, res) {
  if (err) {
    console.error(err);
  } else {
    const token = res.json().access_token;
    pm.environment.set("auth_token", token);
  }
});
```

## 4.10 pm.visualizer

Post-scripts can generate interactive HTML reports using `pm.visualizer.set(template, data)`. The test runner captures this output and writes it to files when `--visualize-dir` is specified.

**Signature:**
```js
pm.visualizer.set(template: string, data?: unknown): void
```

- `template`: Handlebars-compatible HTML template string.
- `data`: Optional data object passed to the template.

Inside the template, you can load external assets (like Chart.js or Tailwind CSS) via CDN inside `<script>` or `<style>` tags to build rich UI dashboards.

**Example:**
```js
const template = `
  <html>
    <body>
      <h1>Status Report for {{name}}</h1>
      <p>Response Status: {{status}}</p>
    </body>
  </html>
`;

pm.visualizer.set(template, {
  name: pm.info.requestName,
  status: pm.response.code
});
```

---

## 5. Execution Flow

When a `TestCase` derived from an imported Postman or Bruno collection is run
through `HttpTestRunner`, the three-phase execution is:

```
+---------------------------------------------------------------------------+
| Phase 1 -- Pre-request script (if pre_script is Some)                     |
|                                                                           |
|   sandbox input:  _request = { url, method, headers, body }               |
|   script import:  import { pm, bru, req } from "sandbox:pm";              |
|   return value:   optional { url?, method?, headers?, body? }             |
|   effect:         merged into effective_request for Phase 2               |
+---------------------------------------------------------------------------+
                              |
                              v
+---------------------------------------------------------------------------+
| Phase 2 -- HTTP fetch (always)                                            |
|                                                                           |
|   sandbox input:  _request = effective_request                            |
|   internal script: FETCH_SCRIPT (ops.op_http_fetch)                       |
|   return value:   { status, ok, headers, body, response_time_ms }         |
+---------------------------------------------------------------------------+
                              |
                              v
+---------------------------------------------------------------------------+
| Phase 3 -- Post-request script (if post_script is Some)                   |
|                                                                           |
|   sandbox input:  _request = effective_request                            |
|                   _response = HttpResponse from Phase 2                   |
|   script import:  import { pm, res, bru, test, expect, results }          |
|                       from "sandbox:pm";                                  |
|   return value:   results() -> { pass: bool, failures: string[] }         |
|   effect:         TestResult.passed / TestResult.failures                 |
+---------------------------------------------------------------------------+
```

Tags injected per phase for observability:

```json
{ "_phase": "pre" | "fetch" | "post", "_test": "<test name>" }
```

These are visible inside scripts via `sandbox.tags()` and forwarded to
`RunMetrics.tags`.

---

## 6. Error Types

**`PostmanError`:**

```rust
pub enum PostmanError {
    Json(serde_json::Error),     // JSON parse failed
    Invalid(String),             // structural validation error
}
```

**`BrunoError`:**

```rust
pub enum BrunoError {
    Parse(String),               // .bru syntax or semantic error
    Io(std::io::Error),          // file read error (import_dir only)
}
```

Both implement `std::error::Error` and `Display` via `thiserror`.
