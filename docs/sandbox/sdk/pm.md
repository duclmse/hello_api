# PmPack — Postman/Bruno Compatibility

`src/sdk/pm_sdk.rs` + `sdk-ts/src/pm.js`

PmPack provides a `sandbox:pm` module that implements the Postman scripting API and Bruno test helpers. Scripts imported from Postman or Bruno collections can use the familiar `pm.test()`, `pm.expect()`, and `pm.response.*` patterns without modification.

---

## Registration

```rust
use hello_sandbox::sdk::pm_sdk::PmPack;

let sandbox = SandboxBuilder::new()
    .sdk(PmPack)
    .build()?;
```

`HttpTestRunner` registers `PmPack` automatically when Postman/Bruno scripts are detected.

---

## JavaScript API

```js
import { pm, res, req, bru, test, expect, results } from "sandbox:pm";

// Postman-style pm.test()
pm.test("status is 200", function() {
    pm.expect(pm.response.code).to.equal(200);
    pm.expect(pm.response.responseTime).to.be.below(500);
});

// Bruno-style test() / expect()
test("body has id", function() {
    const body = pm.response.json();
    expect(body.id).to.not.equal(null);
});

// Access request/response
const status = pm.response.code;      // HTTP status
const body = pm.response.json();       // parsed JSON body
const text = pm.response.text();       // raw text body
const header = pm.response.headers.get("content-type");

// Environment variables
pm.environment.set("token", "abc123");
const token = pm.environment.get("token");

// Global variables
pm.globals.set("userId", "42");
const id = pm.globals.get("userId");

// Send asynchronous requests
pm.sendRequest("https://api.example.com/status", (err, res) => {
    if (!err) console.log("Status: " + res.code);
});

// HTML visualizer output
pm.visualizer.set("<h1>{{title}}</h1>", { title: "Hello World" });

// Must be the last expression
return results();
```

---

## Exports

| Export | Description |
|--------|-------------|
| `pm` | Main Postman `pm` object |
| `pm.test(name, fn)` | Run a named test block |
| `pm.expect(value)` | Chai-like assertion builder |
| `pm.response` | Response wrapper (code, status, json(), text(), headers, responseTime) |
| `pm.request` | Request wrapper (method, url, headers, body) |
| `pm.environment` | Per-run environment variables (get/set/has/unset) |
| `pm.globals` | Global variables (get/set/has/unset) |
| `pm.collectionVariables` | Collection-level variables |
| `pm.sendRequest(req, cb)` | Send an outbound HTTP request (async) |
| `pm.visualizer.set(tpl, data)` | Set interactive HTML visualizer output |
| `res` | Alias for `pm.response` |
| `req` | Alias for `pm.request` |
| `bru` | Bruno compatibility object (same as pm) |
| `test` | Alias for `pm.test` |
| `expect` | Alias for `pm.expect` |
| `results()` | Collect test outcomes and reset state |

---

## Data Sources

Response and request data are read lazily via `sandbox.readInput()` inside getters — not captured at module initialization time. This is required for warm pool slot correctness: module-level variables persist across runs, but `sandbox.readInput()` always returns the current run's data.

`results()` resets all mutable module state (`_pm_tests`, `_env`, `_vars`, `_globals`) before returning, ensuring warm slots start fresh.

---

## `PmTestResult` in RunMetrics

Each `pm.test()` call records its outcome in `RunState::pm_tests` via `op_pm_test`. After the run, these are forwarded to `RunMetrics::pm_tests`:

```rust
let result = sandbox.run(script).await?;
for test in &result.metrics.pm_tests {
    println!("{}: {}", test.name, if test.passed { "PASS" } else { "FAIL" });
}
```

---

## Script Preamble Convention

Scripts imported from Postman/Bruno collections are automatically wrapped with import preambles by the adapters:

**Pre-script:**
```js
import { pm, bru, req } from "sandbox:pm";
// ... original script ...
```

**Post-script:**
```js
import { pm, res, bru, test, expect, results } from "sandbox:pm";
// ... original script ...
return results();
```

---

## Op

| Op | Description |
|----|-------------|
| `op_pm_test(pass: bool, name: String)` | Record a `pm.test()` outcome in `RunState::pm_tests` |

---

## TypeScript Declarations

```typescript
// sdk-ts/types/pm.d.ts
export declare const pm: { ... };
export declare const res: typeof pm.response;
export declare const req: typeof pm.request;
export declare const bru: typeof pm;
export declare function test(name: string, fn: () => void): void;
export declare function expect(value: unknown): ChaiAssertion;
export declare function results(): { pass: boolean; failures: string[] };
```

---

## Source

`src/sdk/pm_sdk.rs`
`sdk-ts/src/pm.js`
`sdk-ts/types/pm.d.ts`
