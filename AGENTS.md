# AGENTS.md — hello_client Workspace

This file is the authoritative agent guide for the `hello_client` workspace.
Read it fully before starting any task. For the `hello_sandbox` sub-crate, also
read `hello_sandbox/AGENT.md` and `hello_sandbox/CLAUDE.md`.

---

## Workspace Structure

```
hello_client/              ← root package (lib + bin) — HTTP client layer
  src/
    lib.rs                 public re-exports + module declarations
    main.rs                CLI entry point (thin — delegates to lib)
    http_runner.rs         HttpTestRunner, TestCase, TestResult, CollectionResult,
                           SecurityProfile, HistorySink, SqliteHistorySink, interpolate
    runner.rs              bridge: .http file parser → TestCase → HttpTestRunner
    client_parser.rs       nom parser for .http file format
    http_request.rs        RequestEntry, HttpRequest, Url, UrlSegment, Script types
    metadata.rs            ### comment-block metadata parser
    request_runner.rs      (extended runner utilities)
    script_executor.rs     (script execution helpers)
    adapters/
      mod.rs               adapter registry + re-exports
      postman.rs           Postman Collection v2.0/v2.1 import/export
      bruno.rs             Bruno .bru file import/export
      bru_parser.rs        nom parser for .bru format (pub(crate))
  sdk-ts/
    src/test.js            sandbox:test assertion library (registered as user module)
    types/test.d.ts        TypeScript declarations for sandbox:test
  tests/
    http_runner_tests.rs   integration tests (mockito, pool_size=1)
  Cargo.toml

hello_sandbox/             ← sandbox engine (pure V8/deno_core layer)
  — see hello_sandbox/AGENT.md for the full spec

hello_tui/                 ← interactive TUI binary (ratatui + crossterm)
  src/
    main.rs                entry point: arg parsing + TUI event loop
    app.rs                 App state machine (TestRow, Phase, spinner)
    ui.rs                  ratatui rendering (header, list, detail, statusbar)
    runner.rs              background thread: LocalSet + HttpTestRunner per-test
    event.rs               RunnerEvent enum (TestStarted, TestFinished, Done, Error)
  Cargo.toml
```

---

## Crate Boundaries

| Crate           | Responsibility                                                                                                     |
| --------------- | ------------------------------------------------------------------------------------------------------------------ |
| `hello_sandbox` | V8 runtime, Sandbox, SandboxBuilder, SDK packs (HttpPack, KvPack, etc.), RunCapabilities, RunMetrics, SandboxError |
| `hello_client`  | HttpTestRunner orchestration, .http file parsing, CLI, adapters, sandbox:test JS library                           |
| `hello_tui`     | Interactive terminal UI — runs tests live and displays results with ratatui                                         |

**Hard rule**: `hello_client` depends on `hello_sandbox`; never the reverse.
`hello_tui` depends on `hello_client`; never the reverse.
`http_runner.rs` imports `hello_sandbox::` types directly — it must **not**
`use crate::sdk::*`.

---

## Module Visibility

| Module                                                              | Visibility   | Purpose              |
| ------------------------------------------------------------------- | ------------ | -------------------- |
| `client_parser`, `http_request`, `metadata`, `adapters::bru_parser` | `pub(crate)` | Internal parsers     |
| `http_runner`, `runner`, `adapters`                                 | `pub`        | External library API |

Top-level re-exports from `lib.rs`:

```rust
interpolate, CollectionResult, HistorySink, HttpRequest, HttpResponse,
HttpTestRunner, SecurityProfile, SqliteHistorySink, TestCase, TestResult,
BrunoAdapter, BrunoError, PostmanAdapter, PostmanCollection, PostmanError
```

The binary (`main.rs`) uses `use hello_client::*` — it does **not** declare
`mod` blocks.

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

- `{{var}}` — substituted from `--param key=value` or runner env
- Pre-script runs before the fetch; may return `{ url, method, headers, body }`
  to override the request
- Post-script runs after the fetch; reads `sandbox.readInput("_response")`, must
  `return results()`
- Multiple entries in one file are separated by `###` metadata blocks

---

## HttpTestRunner Execution Flow

```
Phase 1 — pre_script (if Some)
  set_input("_request", ...)  →  run_with_caps(pre_script, caps("pre"))
  merge returned { url/method/headers/body } into effective_request

Phase 2 — fetch (always)
  set_input("_request", ...)  →  run_with_caps(FETCH_SCRIPT, caps("fetch"))
  FETCH_SCRIPT calls ops.op_http_fetch directly
  body returned as base64 (body_b64), decoded to UTF-8 in Rust via base64 crate

Phase 3 — post_script (if Some)
  set_input("_request", ...)
  set_input("_response", ...)  →  run_with_caps(post_script, caps("post"))
  parse result.value as { pass: bool, failures: string[] }
```

- `TestCase::http_allowed_prefixes` overrides the pack-level allowlist per test.
- `runner.rs::run_collection_from_str` auto-extracts scheme+host prefixes from
  all request URLs and passes them to
  `HttpTestRunner::builder().allowed_prefixes(...)`.

---

## Adapters

| Adapter          | Entry point               | Format                                        |
| ---------------- | ------------------------- | --------------------------------------------- |
| `PostmanAdapter` | `src/adapters/postman.rs` | Postman Collection JSON v2.0 / v2.1           |
| `BrunoAdapter`   | `src/adapters/bruno.rs`   | Bruno `.bru` file (parsed by `bru_parser.rs`) |

Both adapters convert their native format into `TestCase` values consumable by
`HttpTestRunner`. Add new adapters as `pub mod <name>;` in `adapters/mod.rs`
with corresponding `pub use` re-exports.

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
  accumulator

Always call `return results();` as the last expression in a post-script.

---

## Variable Interpolation

`interpolate(template, env)` replaces `{{key}}` with env values; unknown keys
are left unchanged. Applied in `http_runner.rs::interpolate_request` before
Phase 1 and again after pre-script merge.

Set runner-level env via `HttpTestRunner::set_env()` or `builder().env(k, v)`.
CLI params (`--param key=value`) are forwarded to both runner env and
`entry_to_test_case()`.

---

## Security Profiles

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
- Integration tests use `pool_size = 1` (V8 single-thread constraint)
- Never use `unwrap()` in library code — only in tests and examples

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

### Adding a new adapter

1. Create `src/adapters/<name>.rs` implementing the conversion to
   `Vec<TestCase>`
2. Create `src/adapters/<name>_parser.rs` (pub(crate)) if a custom parser is
   needed
3. Add `pub mod <name>;` and `pub(crate) mod <name>_parser;` in
   `src/adapters/mod.rs`
4. Re-export `<Name>Adapter` and `<Name>Error` from `adapters/mod.rs` and
   `lib.rs`

---

## Key Invariants

1. Dependency direction is one-way: `hello_client` → `hello_sandbox`, never
   reversed.
2. `pool_size = 1` in all integration tests — V8 fatal crash if multiple
   isolates are entered on the same thread.
3. `sandbox:test` module source must remain 7-bit ASCII or deno_core will reject
   it.
4. Post-scripts must always call `return results()` to prevent state leaking
   across warm sandbox pool slots.
5. HTTP fetch allowlist is enforced per `TestCase::http_allowed_prefixes` — do
   not bypass it in runner code.

---

## hello_tui — Interactive TUI Runner

`hello_tui` is a standalone binary crate that presents a live terminal UI for
running `.http` collections. It depends only on `hello_client` (the lib) and
the `ratatui` / `crossterm` ecosystem.

### Architecture

```
main thread                         background thread
──────────────────────────────────  ─────────────────────────────────────────
parse_collection()  ← hello_client  tokio single-thread runtime + LocalSet
build App with Pending rows          build HttpTestRunner (builder pattern)
spawn_runner(test_cases, tx)  ──→   for each TestCase:
                                       send TestStarted(i)
TUI event loop (80 ms ticks):          runner.run_test(tc).await
  rx.try_recv() → update App            send TestFinished(i, result)
  crossterm key events                 send Done { elapsed_ms }
  ratatui render
```

The runner thread communicates via `mpsc::SyncSender<RunnerEvent>` with
capacity 64. The TUI drains it non-blockingly each frame with `try_recv`.

### Key Design Decisions

- **Parse before run**: `hello_client::runner::parse_collection` is called
  upfront so all test names are visible in the list before any request fires.
- **One test at a time**: `HttpTestRunner::run_test` is called in a loop
  (not `run_collection`) so a `TestFinished` event is sent after each test.
- **Single tokio runtime on background thread**: the V8 `!Send` constraint
  requires everything on one thread. `LocalSet::run_until` is used, matching
  the same pattern as `hello_client`'s `main.rs`.
- **Boxed TestResult in RunnerEvent**: `TestResult` is large (~312 bytes);
  boxing it keeps the enum variants balanced and avoids stack bloat.

### Key Bindings

| Key | Action |
|-----|--------|
| `j` / `↓` | next test |
| `k` / `↑` | previous test |
| `d` | scroll detail panel down |
| `u` | scroll detail panel up |
| `l` | toggle log output in detail |
| `q` / `Esc` | quit (prints summary to stdout) |

### Development Rules

- Do **not** add async code to `hello_tui/src/main.rs` — it is synchronous;
  all async work lives on the background thread in `runner.rs`.
- Do **not** import `hello_sandbox` directly — go through `hello_client` types.
- The `event` module name clashes with `crossterm::event`; always import
  crossterm event functions directly (`use crossterm::event::{poll, read, …}`).

---

## Commands

```bash
# Build everything
cargo build

# Run hello_client integration tests
cargo test --test http_runner_tests

# Run all hello_client tests (unit + integration)
cargo test -p hello_client

# Run hello_sandbox tests
cargo test -p hello-sandbox

# Run the CLI binary
cargo run -- --request requests.http --param base_url=https://api.example.com

# Run the TUI binary
cargo run -p hello_tui -- requests.http --param base_url=https://api.example.com

# Build the TUI binary only
cargo build -p hello_tui

# Lint (warnings are errors)
cargo clippy -- -D warnings
```
