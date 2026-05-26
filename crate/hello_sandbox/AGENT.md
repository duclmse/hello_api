# Agent.md — deno-sandbox Implementation Prompt

You are implementing `deno-sandbox`, a production-grade JavaScript/TypeScript
sandboxing library in Rust, built on `deno_core`. This document is the single
authoritative specification. Read it fully before writing any code.

---

## 1. Project Overview

The library lets a Rust host execute untrusted JS/TS scripts inside a V8 isolate
with configurable isolation, resource limits, and a typed SDK system for
exposing host capabilities to scripts. It is a Rust library crate (`lib.rs`),
not a binary.

---

## 2. Crate Layout

```
deno-sandbox/
├── Cargo.toml
├── src/
│   ├── lib.rs           — public re-exports
│   ├── config.rs        — IsolationLevel, SandboxConfig
│   ├── error.rs         — SandboxError (thiserror)
│   ├── event.rs         — SandboxEvent
│   ├── loader.rs        — AllowlistModuleLoader + Builder
│   ├── transpile.rs     — TS → JS via deno_ast
│   ├── runtime.rs       — SharedRuntime (core JsRuntime wrapper)
│   ├── pool.rs          — RuntimePool (warm + isolated hybrid)
│   ├── sandbox.rs       — Sandbox + SandboxBuilder (public API)
│   └── sdk/
│       ├── mod.rs       — SdkExtension trait + SdkRegistry
│       ├── core_sdk.rs  — CorePack (console, readInput, emit)
│       ├── kv_sdk.rs    — KvPack (per-slot HashMap store)
│       ├── crypto_sdk.rs — CryptoPack (hash, randomBytes, randomUUID)
│       └── http_sdk.rs  — HttpPack (allowlist-gated fetch)
├── sdk-ts/
│   ├── src/
│   │   ├── core.js      — bootstrap shim (console + sandbox globals)
│   │   ├── kv.js        — KV Promise API shim
│   │   ├── crypto.js    — crypto shim
│   │   └── http.js      — fetch shim + SandboxResponse class
│   └── types/
│       ├── core.d.ts    — declare const sandbox, console
│       ├── kv.d.ts      — export declare const kv
│       ├── crypto.d.ts  — export declare const crypto
│       └── http.d.ts    — export declare function fetch, class SandboxResponse
└── examples/
    └── demo.rs
```

---

## 3. Dependencies (Cargo.toml)

```toml
deno_core          = "0.311"
deno_ast           = { version = "0.43", features = ["transpiling"] }
tokio              = { version = "1", features = ["full"] }
serde              = { version = "1", features = ["derive"] }
serde_json         = "1"
anyhow             = "1"
thiserror          = "1"
tracing            = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }

[target.'cfg(unix)'.dependencies]
nix = { version = "0.29", features = ["process", "signal", "user"] }
```

---

## 4. IsolationLevel & SandboxConfig (`config.rs`)

Three trust tiers — each has a named constructor with preset defaults:

|                 | `Trusted` | `PowerUser` | `Untrusted`              |
| --------------- | --------- | ----------- | ------------------------ |
| Heap max        | 256 MB    | 64 MB       | 16 MB                    |
| Timeout         | 30 s      | 10 s        | 5 s                      |
| Max log lines   | 10 000    | 1 000       | 200                      |
| Watchdog thread | no        | yes         | yes                      |
| OS sandbox      | no        | no          | seccomp+landlock (Linux) |
| Modules default | enabled   | enabled     | disabled                 |

`SandboxConfig` fields: `isolation`, `timeout`, `heap_initial_bytes`,
`heap_max_bytes`, `max_log_lines`, `allow_modules`, `allow_typescript`,
`allow_events`.

---

## 5. Error Types (`error.rs`)

```rust
pub enum SandboxError {
    Timeout(Duration),
    OutOfMemory,
    QuotaExceeded(usize),
    ModuleNotFound(String),
    TranspileError(String),
    Runtime(#[from] anyhow::Error),
    ChildProcess(String),
}
```

---

## 6. SandboxEvent (`event.rs`)

```rust
pub struct SandboxEvent {
    pub name: String,
    pub payload: serde_json::Value,
    pub timestamp_ms: u64,   // ms since run start
}
```

Scripts push events via `sandbox.emit(name, payload)`. The host receives them
through an `mpsc::UnboundedSender<SandboxEvent>` during the run.

---

## 7. AllowlistModuleLoader (`loader.rs`)

- Only resolves `sandbox:` scheme specifiers. Hard-block `ext:` and `node:` in
  `resolve()`.
- Builder pattern:
  `AllowlistModuleLoaderBuilder::register(specifier, source) -> Self`, then
  `.build() -> Result<AllowlistModuleLoader, SandboxError>` which transpiles all
  `.ts`/`.tsx` entries via `transpile.rs`.
- `AllowlistModuleLoaderBuilder` must be `Clone` so the pool can rebuild loaders
  for new slots.

---

## 8. TypeScript Transpilation (`transpile.rs`)

- Uses `deno_ast`: `parse_module` → `.transpile()` → ES2022 JS.
- Cheap heuristic `looks_like_typescript(source: &str) -> bool` to skip plain
  JS.
- `fn transpile(specifier: &str, source: &str, force_ts: bool) -> Result<String, SandboxError>`

---

## 9. SdkExtension Trait & Registry (`sdk/mod.rs`)

```rust
pub trait SdkExtension: Send + Sync + 'static {
    fn name(&self) -> &'static str;
    fn ops(&self) -> Vec<OpDecl>;
    fn esm_files(&self) -> Vec<(&'static str, &'static str)>;  // (specifier, js_source)
    fn ts_declarations(&self) -> &'static str { "" }
    fn initial_op_state(&self) -> Option<Box<dyn Any>> { None }
}
```

`SdkRegistry` collects packs and exposes:

- `all_ops() -> Vec<OpDecl>`
- `all_esm_files() -> Vec<(&'static str, &'static str)>`
- `all_declarations() -> Vec<(String, String)>` —
  `(sandbox:<n>.d.ts, dts_source)`

---

## 10. Built-in SDK Packs

### CorePack (always included, never user-registered)

Ops: `op_print`, `op_read_input`, `op_emit` Shim: `sdk-ts/src/core.js` — sets
`globalThis.console`, `globalThis.sandbox`, freezes all built-in prototypes,
deletes `globalThis.Deno`, freezes `globalThis`.

### KvPack (opt-in)

Op state: `KvStore(HashMap<String, Value>)` — per runtime slot, cleared on
recycle. Ops: `op_kv_get`, `op_kv_set`, `op_kv_delete`, `op_kv_list` Shim:
`sdk-ts/src/kv.js` — exports `{ kv }` with Promise-based API.

### CryptoPack (opt-in)

Ops: `op_crypto_hash(algorithm, data)`, `op_crypto_random_bytes(n)`,
`op_crypto_uuid()` No key generation or signing — those stay on the host. Shim:
`sdk-ts/src/crypto.js` — exports `{ crypto }`.

### HttpPack (opt-in, requires `HttpConfig`)

`HttpConfig { allowed_prefixes: Vec<String>, timeout: Duration, max_response_bytes: usize }`
Op state: `HttpState { config: HttpConfig }` Op: `op_http_fetch(url, opts)` —
**checks allowlist before any network I/O**. Shim: `sdk-ts/src/http.js` —
exports `{ fetch }`, wraps response in `SandboxResponse`.

---

## 11. Per-run State (`RunState` in `runtime.rs`)

```rust
pub struct RunState {
    pub inputs: HashMap<String, Value>,
    pub logs: Vec<String>,
    pub events: mpsc::UnboundedSender<SandboxEvent>,
    pub start: Instant,
    pub max_log_lines: usize,
    pub log_quota_exceeded: bool,
}
```

Injected into `OpState` before each run via
`op_state().borrow_mut().put(run_state)`. Each pack with per-run state uses
`#[state] state: &mut RunState`. Each pack with per-slot state uses its own type
(e.g. `#[state] store: &mut KvStore`).

---

## 12. SharedRuntime (`runtime.rs`)

- Wraps one `JsRuntime` + run counter.
- `new(config, loader, sdk: &SdkRegistry)`:
  1. Collect all ops from registry → build one `Extension` at runtime via
     `Extension::builder`.
  2. Embed all ESM shims into the extension.
  3. `JsRuntime::new(RuntimeOptions { extensions: vec![sdk_ext], module_loader, .. })`.
  4. Inject each pack's `initial_op_state()` into `op_state`.
- `run(source, inputs, event_tx)`:
  1. Transpile TS if `config.allow_typescript`.
  2. Put `RunState` into `OpState`.
  3. Spawn watchdog thread (PowerUser+Untrusted): sleep deadline →
     `isolate.terminate_execution()`.
  4. Wrap script:
     `const __result = await (async () => { <script> })(); op_print("__RETURN__:" + JSON.stringify(__result), false)`.
  5. `load_main_es_module_from_code` → `mod_evaluate` → `run_event_loop` →
     `recv.await`.
  6. Cancel watchdog flag.
  7. Extract `__RETURN__:` sentinel from logs → return `(Value, Vec<String>)`.

---

## 13. RuntimePool (`pool.rs`)

### PoolConfig

```rust
pub struct PoolConfig {
    pub pool_size: usize,            // warm slots; default 4
    pub max_runs_per_slot: u64,      // recycle after N runs; default 100
    pub max_idle_duration: Duration, // recycle if idle > this; default 5 min
    pub fallback_to_isolated: bool,  // burst path; default true
}
```

Presets: `PoolConfig::high_throughput()` (pool=8, runs=500),
`PoolConfig::high_isolation()` (pool=2, runs=10).

### SlotState enum

```rust
enum SlotState {
    Idle { runtime: SharedRuntime, run_count: u64, last_used: Instant },
    CheckedOut,
    Stale,
}
```

### Run priority

1. Idle healthy slot → `run_in_slot(idx)` → return to Idle on success, Stale on
   error.
2. All slots busy + `fallback_to_isolated` → one-shot `SharedRuntime` (created,
   used, dropped).
3. All slots busy + fallback disabled → spin `yield_now()` until a slot frees.

### Slot recycling triggers

- `run_count >= max_runs_per_slot`
- `now - last_used > max_idle_duration`
- Any script error

### `RuntimeKind` (returned in result)

```rust
pub enum RuntimeKind { Warm { slot: usize }, Isolated }
```

### `PoolStats`

```rust
pub struct PoolStats { pub idle: usize, pub checked_out: usize, pub stale: usize, pub total_runs: u64 }
```

---

## 14. Public API (`sandbox.rs`)

### SandboxResult

```rust
pub struct SandboxResult {
    pub value: Value,
    pub logs: Vec<String>,
    pub events: Vec<SandboxEvent>,
    pub elapsed: Duration,
    pub runtime_kind: RuntimeKind,
}
```

### SandboxBuilder (fluent)

```rust
Sandbox::builder()
    .config(SandboxConfig::power_user())
    .pool(PoolConfig::default())
    .input("key", json!(42))
    .module("sandbox:my_lib", "<js source>")
    .sdk(KvPack)
    .sdk(HttpPack::new(HttpConfig { .. }))
    .build()
```

`build()` always prepends `CorePack` before user-registered packs. `build()`
registers all pack `.d.ts` files into the loader builder as
`sandbox:<name>.d.ts` entries.

### Sandbox

```rust
impl Sandbox {
    pub fn builder() -> SandboxBuilder;
    pub fn new(cfg: SandboxConfig) -> Result<Self, SandboxError>;
    pub fn set_input(&mut self, key, value) -> &mut Self;
    pub async fn run(&mut self, script: &str) -> Result<SandboxResult, SandboxError>;
    pub async fn pool_stats(&self) -> PoolStats;
}
```

`run()` wraps `pool.run()` in `tokio::time::timeout(wall_timeout)`. Drains the
event channel after the run completes.

---

## 15. Security Invariants (enforce these everywhere)

1. **No `ext:` imports** — `AllowlistModuleLoader::resolve()` returns `Err` for
   any specifier starting with `ext:` or `node:`.
2. **ops captured in closure** — `core.js` captures `ops` in a block-scoped
   closure, then deletes `globalThis.Deno`. Scripts importing the bootstrap get
   an empty namespace (no exports).
3. **Prototype freeze** — `core.js` freezes `Object`, `Array`, `Function`,
   `String`, `Number`, `Boolean`, `Promise`, `RegExp`, `Error`, `Map`, `Set`,
   `WeakMap`, `WeakSet`, `ArrayBuffer`, typed arrays, `JSON`, `Math`, `Reflect`
   and their `.prototype` objects. This prevents cross-run prototype pollution
   in the shared runtime model.
4. **`globalThis` freeze** — after bootstrap, `Object.freeze(globalThis)`
   prevents scripts from injecting globals for sibling runs.
5. **Watchdog** — for PowerUser and Untrusted, a native OS thread sleeps for the
   deadline and calls `v8::Isolate::terminate_execution()` via raw pointer. This
   kills CPU-spinning scripts (`while(true){}`), not just async-parked ones.
6. **Error → Stale** — any script error causes the pool slot to be marked Stale
   and the `SharedRuntime` dropped. A fresh runtime is created for the next run.
7. **HTTP allowlist checked before network I/O** — `op_http_fetch` rejects
   non-prefixed URLs with `Err` before any `reqwest`/`hyper` call.
8. **One OpState type per pack** — pack state types must be unique Rust types.
   Two instances of the same pack must use distinct newtypes.

---

## 16. JS/TS Surface (what scripts see)

```ts
// Always available — no import needed (injected by core.js)
sandbox.readInput<T>(key: string): T
sandbox.emit(name: string, payload?: unknown): void
console.log / .info / .warn / .error / .debug

// Opt-in via sdk() on builder:
import { kv }     from "sandbox:kv";
import { crypto } from "sandbox:crypto";
import { fetch }  from "sandbox:http";

// Host-registered modules:
import { anything } from "sandbox:my_lib";

// Scripts must NOT be able to reach:
// - ext:* (blocked in loader)
// - node:* (blocked in loader)
// - https:*, file:*, relative paths outside sandbox: (blocked in loader)
// - Deno.* (deleted from globalThis)
// - Raw Deno.core.ops.* (Deno deleted; bootstrap exports nothing)
```

---

## 17. Custom SDK Pack Pattern

```rust
// Rust
pub struct MyPack;

#[op2(fast)]
fn op_my_op(#[string] input: String) -> String { ... }

impl SdkExtension for MyPack {
    fn name(&self) -> &'static str { "my_pack" }
    fn ops(&self) -> Vec<OpDecl> { vec![op_my_op()] }
    fn esm_files(&self) -> Vec<(&'static str, &'static str)> {
        vec![("sandbox:my_pack", include_str!("../../sdk-ts/src/my_pack.js"))]
    }
    fn ts_declarations(&self) -> &'static str {
        include_str!("../../sdk-ts/types/my_pack.d.ts")
    }
}
```

```js
// sdk-ts/src/my_pack.js
const { ops } = Deno.core;
export const myFn = (input) => ops.op_my_op(input);
```

```ts
// sdk-ts/types/my_pack.d.ts
export declare function myFn(input: string): string;
```

---

## 18. Known Placeholders (must be completed for production)

- `op_crypto_hash` / `op_crypto_random_bytes` / `op_crypto_uuid` — replace mock
  implementations with `ring` or `sha2` + `rand::rngs::OsRng` + `uuid` crates.
- `op_http_fetch` — replace stub with `reqwest` async HTTP client.
- V8 heap constraints — wire `v8::CreateParams::new().heap_limits(initial, max)`
  into `RuntimeOptions` (currently only `heap_initial_bytes`/`heap_max_bytes` on
  config but not plumbed into V8).
- Child-process OS sandbox — `IsolationLevel::Untrusted` on Linux forks the
  binary but does not yet install `seccomp` syscall filter. Use `seccompiler`
  crate.
- `AllowlistModuleLoaderBuilder` needs `#[derive(Clone)]` or a manual `Clone`
  impl so the pool can rebuild loaders per slot.

---

## 19. Execution Model Constraints

- `JsRuntime` is `!Send`. All runtime operations must run on a
  `tokio::task::LocalSet`.
- `RuntimePool` is `Send + Sync` (state behind `Arc<Mutex<_>>`); the `JsRuntime`
  values inside slots are extracted, used, and returned only on the LocalSet
  thread.
- `Sandbox` must be driven from within a `LocalSet::run_until` block.
- The watchdog thread holds a raw `*mut v8::Isolate`. This is safe because the
  isolate lives for the duration of `SharedRuntime::run()` and
  `terminate_execution` is documented as safe to call from other threads.

---

## 20. Required Public Exports (`lib.rs`)

```rust
pub use config::{IsolationLevel, SandboxConfig};
pub use error::SandboxError;
pub use event::SandboxEvent;
pub use pool::{PoolConfig, PoolStats, RuntimeKind};
pub use sandbox::{Sandbox, SandboxBuilder, SandboxResult};
pub use sdk::{SdkExtension, SdkRegistry};
```
