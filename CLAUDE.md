# CLAUDE.md — hello_client

This file governs how you work on this workspace. Read it before every task. For
the `hello_sandbox` sub-crate, also read `hello_sandbox/CLAUDE.md`.

---

## Workspace Layout

```
hello_client/            ← root package (lib + bin) — HTTP client layer
  src/
    lib.rs               public re-exports + module declarations
    main.rs              CLI entry point (thin — delegates to lib)
    http_runner.rs       HttpTestRunner, TestCase, TestResult, CollectionResult,
                         SecurityProfile, interpolate  (F1–F8)
    runner.rs            bridge: .http file parser → TestCase → HttpTestRunner
    client_parser.rs     nom parser for .http file format
    http_request.rs      RequestEntry, HttpRequest, Url, UrlSegment, Script types
    metadata.rs          ### comment-block metadata parser
  sdk-ts/
    src/test.js          sandbox:test assertion library (registered as user module)
    types/test.d.ts      TypeScript declarations for sandbox:test
  tests/
    http_runner_tests.rs integration tests (26 tests, mockito, pool_size=1)
  Cargo.toml

hello_sandbox/           ← sandbox engine (pure V8/deno_core layer)
  — see hello_sandbox/CLAUDE.md
```

---

## Crate Boundaries

| Lives in        | Responsibility                                                                                                     |
| --------------- | ------------------------------------------------------------------------------------------------------------------ |
| `hello_sandbox` | V8 runtime, Sandbox, SandboxBuilder, SDK packs (HttpPack, KvPack, etc.), RunCapabilities, RunMetrics, SandboxError |
| `hello_client`  | HttpTestRunner orchestration, .http file parsing, CLI, sandbox:test JS library                                     |

**Rule**: `hello_client` depends on `hello_sandbox`; never the reverse.
`http_runner.rs` imports `hello_sandbox::` types — it does not
`use crate::sdk::*`.

---

## Module Visibility

- `client_parser`, `http_request`, `metadata` — `pub(crate)`, internal to lib
- `http_runner`, `runner` — `pub`, part of the external library API
- Top-level re-exports from `lib.rs`:
  `interpolate, CollectionResult, HttpRequest, HttpResponse, HttpTestRunner, SecurityProfile, TestCase, TestResult`

The binary (`main.rs`) accesses everything via `use hello_client::*` — it does
**not** declare `mod` blocks.

---

## .http File Format

```
### Optional description line
### @param name value
### #hashtag

GET https://{{base_url}}/users/{{user_id}}
Authorization: Bearer {{token}}

> {%
  // pre-script (inline JS/TS)
%}

> post_script.js   // or file reference
```

- `{{var}}` placeholders: substituted from CLI `--param key=value` or runner env
- Pre-script runs before the fetch; can return `{ url, method, headers, body }`
  to override the request
- Post-script runs after the fetch; reads `sandbox.readInput("_response")`,
  should `return results()`
- Multiple entries in one file are separated by `###` metadata blocks

---

## sandbox:test Module

`sdk-ts/src/test.js` is registered as the `sandbox:test` user module by
`HttpTestRunner::new()`. It must be **7-bit ASCII only** (deno_core
requirement).

Key exports:

- `expect(actual)` — chainable assertion builder with `.toBe`, `.toContain`,
  `.not.*`, etc.
- `wrapResponse(raw)` — wraps `_response` input with `.headers.get(name)`
  (case-insensitive), `.json()`, `.text()`
- `results()` — returns `{ pass, failures }` and **resets** the failure
  accumulator (prevents state leaking across warm pool slots)

Always call `return results();` as the last expression in a post-script.

---

## HttpTestRunner Execution Flow

```
Phase 1 — pre_script (if Some)
  set_input("_request", ...)  →  run_with_caps(pre_script, caps("pre"))
  merge returned { url/method/headers/body } into effective_request

Phase 2 — fetch (always)
  set_input("_request", ...)  →  run_with_caps(FETCH_SCRIPT, caps("fetch"))
  FETCH_SCRIPT calls ops.op_http_fetch directly (no TextDecoder needed)
  body returned as base64 (body_b64), decoded to UTF-8 in Rust via base64 crate

Phase 3 — post_script (if Some)
  set_input("_request", ...)
  set_input("_response", ...)  →  run_with_caps(post_script, caps("post"))
  parse result.value as { pass: bool, failures: string[] }
```

`TestCase::http_allowed_prefixes` overrides the pack-level allowlist per test.
`runner.rs::run_collection_from_str` auto-extracts scheme+host prefixes from all
request URLs and passes them to
`HttpTestRunner::builder().allowed_prefixes(...)`.

---

## Variable Interpolation (F6)

`interpolate(template, env)` replaces `{{key}}` with env values; unknown keys
left unchanged. Applied in `http_runner.rs::interpolate_request` before Phase 1
and again after pre-script merge. Runner-level env set via
`HttpTestRunner::set_env()` or `builder().env(k, v)`. CLI params
(`--param key=value`) are forwarded to both runner env and
`entry_to_test_case()`.

---

## Security Profiles (F8)

`SecurityProfile` static methods return pre-built `RunCapabilities`:

| Method                    | Effect                         |
| ------------------------- | ------------------------------ |
| `public_api(base_url)`    | allowlist + http/kv limits     |
| `auth_flow(base_url)`     | + restrict emit event names    |
| `sensitive(base_url, id)` | + kv_key_prefix `secure:{id}:` |
| `user_script(timeout)`    | http disabled, tight timeout   |

---

## CLI

```
hello_client [OPTIONS] [REQUEST_FILE]

-r, --request <FILE>     .http file (default: requests.http)
-c, --config  <FILE>     config file (timeout, verbose, base_url, param.*)
-p, --param   <KEY=VAL>  variable substitution (repeatable)
-v, --verbose
-t, --timeout <SECS>     default 30s
-f, --format  json|plain|pretty   default pretty
```

Exit code 1 if any test case fails or a parse/runtime error occurs.

---

## Development Rules

### Rust conventions

- `http_runner.rs` must not import from `crate::sdk::*` — use
  `hello_sandbox::sdk::*`
- `main.rs` must not declare `mod` blocks — use `use hello_client::*`
- All async functions that touch `HttpTestRunner` must run on a `LocalSet`
- Integration tests use `pool_size = 1` (V8 single-thread constraint — see
  hello_sandbox/CLAUDE.md)

### JS conventions (sdk-ts/src/test.js)

- 7-bit ASCII only (no Unicode outside ASCII 32–126)
- Access ops via `const ops = globalThis.__sandbox_ops;` (not `Deno.core` —
  deleted by core.js)
- `results()` must reset `_failures = []` before returning to avoid warm-slot
  state pollution

### Adding a new .http file feature

1. Extend `http_request.rs` types if the syntax changes
2. Update `client_parser.rs` (nom parsers)
3. Update `runner.rs::entry_to_test_case()` to map the new field to `TestCase`
4. Add integration tests in `tests/http_runner_tests.rs`

---

## Commands

```bash
# Build everything
cargo build

# Run hello_client integration tests (http_runner_tests)
cargo test --test http_runner_tests

# Run all hello_client tests (unit + integration)
cargo test -p hello_client

# Run hello_sandbox tests
cargo test -p hello-sandbox

# Run the binary
cargo run -- --request requests.http --param base_url=https://api.example.com

# Lint
cargo clippy -- -D warnings
```
