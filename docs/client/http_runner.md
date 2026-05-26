# HTTP Test Runner

`src/http_runner.rs` implements the core HTTP test orchestration engine. It
wraps a `hello_sandbox::Sandbox` and executes three-phase test cases against
real HTTP endpoints.

## Public Types

### `HttpRequest`

A fully resolved HTTP request (no `{{variables}}`):

```rust
pub struct HttpRequest {
    pub url: String,
    pub method: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
}
```

Convenience builders:

- `HttpRequest::get(url)` — sets method to `GET`
- `HttpRequest::post(url, body)` — sets method to `POST` with a body

---

### `HttpResponse`

The HTTP response captured after Phase 2:

```rust
pub struct HttpResponse {
    pub status: u16,
    pub ok: bool,              // status in 200..=299
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub response_time_ms: u64,
    pub redirected: bool,
}
```

---

### `TestCase`

One test case, wrapping a request with optional scripts and run constraints:

```rust
pub struct TestCase {
    pub name: String,
    pub request: HttpRequest,
    pub pre_script: Option<String>,
    pub post_script: Option<String>,
    pub modules: Vec<(String, String)>,
    pub tags: HashMap<String, String>,
    pub timeout_override: Option<Duration>,
    pub kv_key_prefix: Option<String>,
    pub http_allowed_prefixes: Option<Vec<String>>,
    pub output_file: Option<String>,
    pub response_file: Option<String>,
}
```

- `modules` — additional ES modules registered before running scripts, as
  `(specifier, source)` pairs (e.g. for `import` dependencies)
- `kv_key_prefix` — namespaces all `kv.*` calls inside the test to prevent
  cross-test pollution
- `http_allowed_prefixes` — per-test URL allowlist that overrides the
  runner-level allowlist
- `tags` — forwarded to `RunCapabilities` and appear in `RunMetrics`
- `output_file` — if set, the response body is written to this path after the
  fetch phase (maps from `### @param output <path>` in the `.http` file)
- `response_file` — if set, skip the real HTTP fetch and load the response body
  from this file instead (used with `--response-file` CLI flag)

---

### `TestResult`

Result of running a single `TestCase`:

```rust
pub struct TestResult {
    pub name: String,
    pub passed: bool,
    pub failures: Vec<String>,
    pub response: Option<HttpResponse>,
    pub logs: Vec<String>,
    pub events: Vec<SandboxEvent>,
    pub metrics: RunMetrics,
    pub visualizer_html: Option<String>,
    pub output_written: Option<String>,
}
```

- `visualizer_html` — HTML string produced by `pm.visualizer.set(...)` in the
  post-script, if called. Written to disk by CLI when `--visualize-dir` is set.
- `output_written` — path where the response body was written, if
  `TestCase::output_file` was set.

---

### `CollectionResult`

Result of running all test cases:

```rust
pub struct CollectionResult {
    pub passed: usize,
    pub failed: usize,
    pub results: Vec<TestResult>,
    pub total_duration: Duration,
}
```

---

### `HistorySink`

Trait for persisting test results:

```rust
pub trait HistorySink: Send + Sync {
    fn record(&self, result: &TestResult);
}
```

### `SqliteHistorySink`

File-backed or in-memory SQLite implementation of `HistorySink`. Creates a
`test_history` table with 11 columns on first use.

```rust
let sink = SqliteHistorySink::open("./results.db")?;
let sink = SqliteHistorySink::in_memory()?;
```

Columns: `name`, `status` (pass/fail), `response_time_ms`, `peak_heap_bytes`,
`assertions_passed`, `assertions_failed`, `http_calls`, `kv_ops`, `emit_calls`,
`failures_json`, `timestamp`.

---

## HttpTestRunner

The main runner type.

### Construction

**Builder (recommended):**

```rust
let runner = HttpTestRunner::builder()
    .allowed_prefixes(vec!["https://api.example.com".into()])
    .http_timeout(Duration::from_secs(30))
    .env("base_url", "https://api.example.com")
    .env("token", "secret")
    .history(Arc::new(SqliteHistorySink::open("results.db")?))
    .build()?;
```

**From existing Sandbox:**

```rust
let runner = HttpTestRunner::new(sandbox)?;
```

`HttpTestRunner::new()` automatically registers the `sandbox:test` module.

### Running Tests

```rust
// Single test case
let result: TestResult = runner.run_test(test_case).await?;

// Collection of test cases (sequential, shared KV state)
let collection: CollectionResult = runner.run_collection(test_cases).await?;
```

---

## Three-Phase Execution

Each `TestCase` goes through up to three phases:

### Phase 1 — Pre-script (optional)

```
set_input("_request", { url, method, headers, body })
run_with_caps(pre_script, caps("pre"))
```

The pre-script may return a partial override object:

```js
return { url: "https://other.com/v2", headers: { "X-Override": "yes" } };
```

Any returned fields are merged into the effective request before Phase 2.

### Phase 2 — Fetch (always)

```
set_input("_request", { url, method, headers, body })
run_with_caps(FETCH_SCRIPT, caps("fetch"))
```

`FETCH_SCRIPT` calls `op_http_fetch` directly (bypassing the JS module system).
The response body is returned as `body_b64` (base64), decoded in Rust via the
`base64` crate. The result is stored as
`set_input("_response", { status, ok, headers, body, ... })`.

### Phase 3 — Post-script (optional)

```
set_input("_request", { url, method, headers, body })  // effective request
set_input("_response", { status, ok, headers, body, response_time_ms, redirected })
run_with_caps(post_script, caps("post"))
```

The post-script must return `{ pass: bool, failures: string[] }` via `results()`
from `sandbox:test`. This is parsed by the runner into `TestResult`.

---

## Variable Interpolation

`interpolate(template, env)` replaces `{{key}}` with env values. Unknown keys
are left unchanged.

Applied in `interpolate_request()`:

1. Before Phase 1 (on the raw request)
2. After the pre-script merge (on the effective request)

Set template variables via:

- `HttpTestRunner::builder().env(key, value)` — runner-level
- `TestCase` constructed with params from CLI or metadata

---

## Security Profiles

`SecurityProfile` provides pre-built `RunCapabilities` for common trust
patterns:

| Method                                     | Effect                                      |
| ------------------------------------------ | ------------------------------------------- |
| `SecurityProfile::public_api(base_url)`    | URL allowlist + HTTP/KV call limits         |
| `SecurityProfile::auth_flow(base_url)`     | As above + restricted emit event names      |
| `SecurityProfile::sensitive(base_url, id)` | As above + `kv_key_prefix = "secure:{id}:"` |
| `SecurityProfile::user_script(timeout)`    | HTTP disabled, tight timeout                |

```rust
let caps = SecurityProfile::public_api("https://api.example.com");
sandbox.run_with_caps(script, caps).await?;
```

---

## `sandbox:test` Module

Automatically registered by `HttpTestRunner::new()`. Available in all
post-scripts:

```js
import { expect, wrapResponse, results } from "sandbox:test";

const res = wrapResponse(sandbox.readInput("_response"));

expect(res.status).toBe(200);
expect(res.headers.get("content-type")).toContain("json");

const body = res.json();
expect(body.id).not.toBe(null);

return results(); // required — resets failure state for warm pool slots
```

**Key exports:**

- `expect(actual)` — chainable assertion builder (`.toBe`, `.toContain`,
  `.not.*`)
- `wrapResponse(raw)` — wraps `_response` input with `.headers.get(name)`
  (case-insensitive), `.json()`, `.text()`
- `results()` — returns `{ pass, failures }` and **resets** the accumulator

The JS source lives in `sdk-ts/src/test.js` (7-bit ASCII only).

---

## Source

`src/http_runner.rs` — ~1000 lines. Integration tests in
`tests/http_runner_tests.rs` (16 tests, mockito, `pool_size=1`).
