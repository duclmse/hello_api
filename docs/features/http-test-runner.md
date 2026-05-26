# HTTP Test Runner — Implementation Reference

This is a design note for `HttpTestRunner` and the `sandbox:test` module. For
the full public API reference, see [HTTP Runner](../client/http_runner.md).

---

## Overview

`HttpTestRunner` (in `src/http_runner.rs`) wraps a `hello_sandbox` `Sandbox` and
orchestrates three-phase test execution: pre-script, HTTP fetch, and
post-script. It was originally planned as a Phase 19 addition to hello_sandbox
but is implemented in `hello_client` to respect the crate boundary (hello_client
depends on hello_sandbox, not the reverse).

---

## Three-Phase Execution

### Phase 1 — Pre-Script (optional)

```
set_input("_request", serialize(test.request))
run_with_caps(pre_script, caps(test, "pre"))
if result.value has {url, method, headers, body} → merge into effective_request
```

The pre-script can read and write KV (environment store), override any request
field by returning an object, and access `sandbox.tags()` for metadata.

### Phase 2 — HTTP Fetch (always)

```
set_input("_request", serialize(effective_request))
run_with_caps(FETCH_SCRIPT, caps(test, "fetch"))
effective_response = deserialize(result.value) as HttpResponse
```

`FETCH_SCRIPT` is an internal constant that calls `op_http_fetch` directly via
the `sandbox:http` pack. The response body is returned as base64 (`body_b64`)
and decoded to UTF-8 in Rust via the `base64` crate. No `TextDecoder` is used in
the JS layer.

### Phase 3 — Post-Script (optional)

```
set_input("_request", serialize(effective_request))
set_input("_response", serialize(http_response))
run_with_caps(post_script, caps(test, "post"))
parse result.value as { pass: bool, failures: string[] }
```

A `{ pass: false, failures: [...] }` result is **not** a Rust error — it becomes
`TestResult { passed: false, failures: [...] }`. Script syntax errors and
runtime errors propagate as `SandboxError::Runtime`.

### The `caps()` Helper

Builds `RunCapabilities` from a `TestCase` + phase name:

- `tags`: `test.tags` merged with `{"_phase": phase, "_test": test.name}`
- `timeout_override`, `kv_key_prefix`, `http_allowed_prefixes` from `TestCase`

---

## sandbox:test Module Design

`sdk-ts/src/test.js` is registered as `sandbox:test` by `HttpTestRunner::new()`.

**Key design constraints:**

- Must be 7-bit ASCII only (deno_core ESM entry point requirement)
- Accesses ops via `const ops = globalThis.__sandbox_ops;` (not `Deno.core` —
  deleted by `core.js`)
- `results()` resets `_failures = []` before returning to avoid warm-slot state
  pollution

**Why `results()` must reset state:** The `RuntimePool` reuses warm
`SharedRuntime` slots across multiple runs. Module-level variables in a
`sandbox:*` module persist in the warm slot between runs. Without the reset, a
failure from run N would appear in run N+1's results.

---

## Additions Beyond the Original Plan

The original plan's `TestCase` and `TestResult` types were minimal. The
implemented versions include additional fields:

**`TestCase` additions:**

- `modules: Vec<(String, String)>` — extra `sandbox:*` modules registered
  per-test
- `output_file: Option<PathBuf>` — write response body to file
- `response_file: Option<PathBuf>` — write full response JSON to file

**`TestResult` additions:**

- `visualizer_html: Option<String>` — HTML snippet for the TUI visualizer
- `output_written: bool` — whether `output_file` was successfully written

The `.http` file runner bridge (`src/runner.rs`) uses `run_collection_from_str`
to auto-extract scheme+host prefixes from all request URLs and forwards them as
the HTTP allowlist to `HttpTestRunner::builder().allowed_prefixes(...)`.
