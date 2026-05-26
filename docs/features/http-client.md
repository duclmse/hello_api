# hello_client — Feature Reference

hello_client is an HTTP test runner built on top of hello_sandbox. It provides a
three-phase execution model (pre-script → HTTP fetch → post-script), a `.http`
file parser, a CLI, collection adapters (Postman, Bruno, curl), and a built-in
`sandbox:test` assertion library.

---

## Feature Table

| Feature                       | Description                                     | Status      |
| ----------------------------- | ----------------------------------------------- | ----------- |
| F1 — HttpTestRunner           | Three-phase request orchestrator                | Implemented |
| F2 — KV Environment Store     | Shared KV namespace across a collection run     | Implemented |
| F3 — sandbox:test             | Built-in JS assertion library                   | Implemented |
| F4 — Response Enrichment      | `responseTime`, `headers.get()`, `size`         | Implemented |
| F5 — Collection Runner        | Sequential multi-request runs with chaining     | Implemented |
| F6 — Variable Interpolation   | `{{variable}}` substitution in URLs and headers | Implemented |
| F7 — Request/Response Logging | Per-run logs, events, and `RunMetrics`          | Implemented |
| F8 — Security Profiles        | Per-request `RunCapabilities` presets           | Implemented |
| F9 — sandbox:assert           | Formal assertion tracking in `RunMetrics`       | Implemented |

---

## F1 — HttpTestRunner

`HttpTestRunner` wraps a `Sandbox` (with `HttpPack` and `KvPack`) and
orchestrates three-phase test execution:

```
pre_script (optional) → HTTP fetch → post_script (optional)
```

**API:**

```rust
HttpTestRunner::builder()              // convenience builder, HttpPack + KvPack pre-wired
    .env("base_url", "https://api.example.com")
    .allowed_prefixes(vec!["https://api.example.com"])
    .pool_size(4)
    .build()

runner.run_test(test_case).await       // single test
runner.run_collection(tests).await     // sequential collection
```

The builder registers `sandbox:test` and `sandbox:test.d.ts` automatically. See
[HTTP Runner](../client/http_runner.md) for the full API reference.

---

## F2 — KV Environment Store

Scripts within a collection run share a `KvPack` backend. Values written in one
request (e.g., an extracted auth token) are readable by subsequent requests.

```js
// In a post-script:
import { kv } from "sandbox:kv";
const token = res.json().access_token;
await kv.set("auth_token", token);

// In the next request's pre-script:
const token = await kv.get("auth_token");
```

Collection isolation is enforced via `kv_key_prefix` in `RunCapabilities`, which
prevents cross-collection bleed. Secret masking can be applied at the log-sink
level.

---

## F3 — sandbox:test Assertion Library

A built-in JS module registered as `sandbox:test` on every `HttpTestRunner`.
Scripts import it without any configuration:

```js
import { expect, wrapResponse, results } from "sandbox:test";

const res = wrapResponse(sandbox.readInput("_response"));
expect(res.status).toBe(200);
expect(res.headers.get("content-type")).toContain("application/json");
expect(res.json().id).toBeGreaterThan(0);

return results();
```

**Exports:**

- `expect(actual)` — chainable assertion builder with `.toBe`, `.toEqual`,
  `.toContain`, `.toBeTruthy`, `.toBeFalsy`, `.toBeNull`, `.toBeUndefined`,
  `.toBeGreaterThan`, `.toBeLessThan`, `.not.*`
- `wrapResponse(raw)` — wraps the raw `_response` input with a
  `.headers.get(name)` method (case-insensitive), `.json()`, and `.text()`
- `results()` — returns `{ pass: bool, failures: string[] }` and resets the
  failure accumulator (prevents state leaking across warm pool slots)

The source lives in `sdk-ts/src/test.js` and must be 7-bit ASCII only.

---

## F4 — Response Object Enrichment

The HTTP response available to post-scripts includes:

| Property            | Type           | Description                             |
| ------------------- | -------------- | --------------------------------------- |
| `status`            | number         | HTTP status code                        |
| `ok`                | boolean        | `true` if `status` is 200–299           |
| `headers.get(name)` | string or null | Case-insensitive header lookup          |
| `headers.has(name)` | boolean        | Case-insensitive header existence check |
| `text()`            | string         | Response body as text (sync)            |
| `json()`            | any            | Parsed JSON body (sync)                 |
| `responseTime`      | number         | Elapsed ms from request start           |
| `size`              | number         | Body byte length                        |

`responseTime` is injected from `RunMetrics.elapsed` before the post-script
runs. `headers.get()` is a pure JS wrapper over the array-of-tuples returned by
`HttpPack`.

---

## F5 — Collection Runner with Chaining

`run_collection` runs test cases sequentially, sharing the KV backend across all
cases:

```rust
let result: CollectionResult = runner.run_collection(vec![
    TestCase { name: "Login".into(), request: login_req, post_script: Some(LOGIN_POST), .. },
    TestCase { name: "GetUser".into(), request: get_user_req, post_script: Some(GET_POST), .. },
]).await?;

println!("{}/{} passed", result.passed, result.passed + result.failed);
```

`CollectionResult` aggregates `passed`, `failed`, individual `TestResult`s, and
`total_duration`. Each test emits `test_pass` / `test_fail` events for live
reporting.

---

## F6 — Variable Interpolation

`{{variable}}` placeholders in request URLs, headers, and bodies are substituted
with runner environment values before the request is constructed:

```http
GET https://{{base_url}}/users/{{user_id}}
Authorization: Bearer {{auth_token}}
```

- `interpolate(template, env)` in `src/http_runner.rs` performs substitution
- Unknown keys are left unchanged (no error)
- Applied before the pre-script and again after a pre-script merge
- Set via `HttpTestRunner::set_env(key, value)`, `builder().env(k, v)`, or CLI
  `--param key=value`

---

## F7 — Request/Response Logging

Each `TestResult` captures:

- `logs` — `console.*` output from pre and post scripts
- `events` — `sandbox.emit(...)` events (test pass/fail, custom)
- `metrics` — `RunMetrics` from the last phase (heap, elapsed, op counts, tags)
- `response` — full `HttpResponse` (status, headers, body, timing)

Tags carry `_phase` (`"pre"`, `"fetch"`, `"post"`) and `_test` (test name) into
`RunMetrics` for the metrics sink. Optional SQLite-backed history persistence is
available via `SqlitePack`.

---

## F8 — Per-Request Security Profiles

`SecurityProfile` provides static constructors for pre-built `RunCapabilities`:

| Profile              | Method                                     | Effect                             |
| -------------------- | ------------------------------------------ | ---------------------------------- |
| Public API test      | `SecurityProfile::public_api(base_url)`    | HTTP allowlist + KV/HTTP op limits |
| Auth flow            | `SecurityProfile::auth_flow(base_url)`     | + restricted `emit` event names    |
| Sensitive endpoint   | `SecurityProfile::sensitive(base_url, id)` | + `kv_key_prefix = "secure:{id}:"` |
| User-provided script | `SecurityProfile::user_script(timeout)`    | HTTP disabled, tight timeout       |

These enforce the principle of least privilege per test without requiring
separate sandbox instances.

---

## F9 — sandbox:assert

`AssertPack` provides formal assertion ops tracked in `RunMetrics`:

```js
import { assert } from "sandbox:assert";
assert.equal(res.status, 200, "expect 200 OK");
assert.contains(res.text(), "success", "body should say success");
// collects all failures, not just the first
```

- Pass/fail counts recorded in `RunState` (new fields per run)
- Surfaced as `RunMetrics.assertions: { passed, failed }` after the run
- No JS exception thrown on failure — allows collecting all assertion failures
- Available via `AssertPack` in the `hello_sandbox` SDK pack registry
