# Development Guide

This guide covers workspace layout, crate boundaries, build commands, and
conventions for adding features to hello_client and hello_sandbox.

---

## Workspace Layout

```
hello_client/               root workspace
  src/
    lib.rs                  public re-exports + module declarations
    main.rs                 CLI entry point (thin — delegates to lib)
    http_runner.rs          HttpTestRunner, TestCase, TestResult, CollectionResult,
                            SecurityProfile, interpolate
    runner.rs               bridge: .http file parser -> TestCase -> HttpTestRunner
    client_parser.rs        nom parser for the .http file format
    http_request.rs         RequestEntry, HttpRequest, Url, UrlSegment, Script types
    metadata.rs             ### comment-block metadata parser
    flow.rs                 Flow type and flow-level abstractions
    flow_parser.rs          Flow file parser
    flow_runner.rs          Flow execution orchestrator
    adapters/
      postman.rs            Postman v2 collection import/export
      bruno.rs              Bruno .bru file import/export
      curl.rs               curl command import/export
      opencollection.rs     OpenCollection v1.0.0 JSON import/export
      openapi.rs            OpenAPI 3.x / Swagger 2.0 import/export
  sdk-ts/
    src/test.js             sandbox:test assertion library (7-bit ASCII only)
    types/test.d.ts         TypeScript declarations for sandbox:test
  tests/
    http_runner_tests.rs    integration tests (mockito, pool_size=1)
  Cargo.toml

hello_tui/                  TUI frontend (separate crate)
  src/
    main.rs
    app.rs
    ui.rs
    debug.rs

hello_sandbox/              V8/deno_core sandbox engine (separate crate)
  src/
    lib.rs, sandbox.rs, runtime.rs, pool.rs, config.rs, error.rs, event.rs
    transpile.rs, loader.rs, snapshot.rs
    sdk/
      mod.rs, core_sdk.rs, kv_sdk.rs, http_sdk.rs, crypto_sdk.rs
      sqlite_sdk.rs, timer_sdk.rs, assert_sdk.rs, pm_sdk.rs
  sdk-ts/src/               JS shims for each pack
  sdk-ts/types/             TypeScript declarations
```

---

## Crate Boundaries

**Rule: hello_client depends on hello_sandbox. Never the reverse.**

| Lives in        | Responsibility                                                                                                     |
| --------------- | ------------------------------------------------------------------------------------------------------------------ |
| `hello_sandbox` | V8 runtime, Sandbox, SandboxBuilder, SDK packs (HttpPack, KvPack, etc.), RunCapabilities, RunMetrics, SandboxError |
| `hello_client`  | HttpTestRunner orchestration, .http file parsing, CLI, sandbox:test JS library, adapters                           |
| `hello_tui`     | Terminal UI (depends on hello_client)                                                                              |

`http_runner.rs` imports `hello_sandbox::` types. It does not
`use crate::sdk::*`.

`main.rs` must not declare `mod` blocks — it accesses everything via
`use hello_client::*`.

---

## Build & Test

```bash
# Build everything
cargo build

# Run all hello_client tests (unit + integration)
cargo test -p hello_client

# Run only the integration tests
cargo test --test http_runner_tests

# Run hello_sandbox tests
cargo test -p hello-sandbox

# Lint (must pass before committing)
cargo clippy -- -D warnings

# Run the CLI
cargo run -- --request requests.http --param base_url=https://api.example.com

# Run with verbose output and JSON format
cargo run -- --request requests.http -v --format json
```

Integration tests use `mockito` for HTTP mocking. All tests must run with
`pool_size = 1` due to V8's single-thread constraint (see
hello_sandbox/CLAUDE.md).

---

## Adding a Feature to hello_client

Follow this order when the feature involves a new `.http` file syntax or new
test execution behavior:

1. **Extend types** in `src/http_request.rs` — add fields to `RequestEntry`,
   `HttpRequest`, `Script`, or related types as needed.

2. **Update the parser** in `src/client_parser.rs` — add or extend nom parsers
   to recognize the new syntax. The parser produces `RequestEntry` values.

3. **Update the bridge** in `src/runner.rs` — `entry_to_test_case()` maps
   `RequestEntry` fields to `TestCase` fields. Wire the new field here.

4. **Update the runner** in `src/http_runner.rs` if the new field affects
   execution (e.g., new capability passed to `caps()`, new phase behavior).

5. **Add integration tests** in `tests/http_runner_tests.rs`. Use `mockito` for
   any HTTP interactions. Wrap the test body in
   `LocalSet::new().run_until(async { ... })`.

6. **Re-export** any new public types from `src/lib.rs`.

---

## Adding a Feature to hello_sandbox

New capabilities follow the **SDK Pack pattern**:

1. **Create `src/sdk/xxx_sdk.rs`** implementing the `SdkExtension` trait:
   - `name()` — pack name string
   - `ops()` — list of `#[op2]` functions
   - `inject_op_state(state)` — insert per-slot state into `OpState`
   - `esm_files()` — return the JS shim source (embedded via `include_str!`)
   - `dts_files()` — return TypeScript declaration source

2. **Write `sdk-ts/src/xxx.js`** — the JS shim served as `sandbox:xxx`. Must be
   7-bit ASCII. Access ops via `const ops = globalThis.__sandbox_ops;`.

3. **Write `sdk-ts/types/xxx.d.ts`** — TypeScript declarations for the shim's
   exports.

4. **Register the pack** in `src/sdk/mod.rs` and export it from `src/lib.rs`.

5. **Add integration tests** in `tests/<feature>_tests.rs` following the
   `pool_size=1` + `LocalSet::new().run_until(async { ... })` pattern.

---

## Key Conventions

### Rust

- **All async functions that touch `HttpTestRunner` or `Sandbox` must run on a
  `LocalSet`.** V8 is `!Send`. Spawn `tokio::task::LocalSet` and use `run_until`
  or `spawn_local`.

- **Integration tests use `pool_size = 1`.** This is a hard requirement from
  V8's single-thread constraint. Multiple concurrent V8 instances in the same
  process will panic.

- **The `Mutex` guard must never be held across an `await` point.** Extract the
  value from the guard, drop the guard, then await. Grep for `lock().await` to
  catch violations. The `RuntimePool` explicitly tests this.

- **nom parsers in `client_parser.rs`** use the `nom` combinator style. Prefer
  `complete::*` variants for the top-level file parser since the full input is
  in memory.

- **`http_runner.rs` must not import from `crate::sdk::*`.** Use
  `hello_sandbox::sdk::*` directly. This enforces the crate boundary at the
  import level.

### JavaScript (sdk-ts/src/test.js and other shims)

- **7-bit ASCII only.** No Unicode characters outside ASCII 32–126. This is a
  hard deno_core requirement for ESM entry points.

- **Access ops via `const ops = globalThis.__sandbox_ops;`** — not via
  `Deno.core`, which is deleted by `core.js` at bootstrap time.

- **`results()` must reset `_failures = []` before returning.** Warm pool slots
  persist module-level state. Without the reset, a failure from run N leaks into
  run N+1.

---

## Module Visibility

| Module                               | Visibility   | Reason                                                   |
| ------------------------------------ | ------------ | -------------------------------------------------------- |
| `client_parser`                      | `pub(crate)` | Internal parser, not part of public API                  |
| `http_request`                       | `pub(crate)` | Internal types, exposed only through `lib.rs` re-exports |
| `metadata`                           | `pub(crate)` | Internal parser utility                                  |
| `http_runner`                        | `pub`        | Core library API                                         |
| `runner`                             | `pub`        | Core library API                                         |
| `flow`, `flow_parser`, `flow_runner` | `pub`        | Core library API                                         |
| `adapters`                           | `pub`        | Core library API                                         |

Top-level re-exports from `lib.rs`:
- **Adapters**: `BrunoAdapter`, `BrunoError`, `CurlAdapter`, `CurlError`, `OpenApiAdapter`, `OpenApiCollection`, `OpenApiError`, `OpenCollection`, `OpenCollectionAdapter`, `OpenCollectionError`, `PostmanAdapter`, `PostmanCollection`, `PostmanError`
- **Flow Control**: `CaptureBinding`, `CaptureExpr`, `FlowDef`, `FlowNode`, `ParallelGroup`, `StepDef`, `FlowResult`, `StepOutcome`, `parse_flow`, `run_flow`
- **HTTP Runner & Utilities**: `CollectionResult`, `HistorySink`, `HttpRequest`, `HttpResponse`, `HttpTestRunner`, `PhaseTimings`, `SecurityProfile`, `SqliteHistorySink`, `TestCase`, `TestResult`, `interpolate`
