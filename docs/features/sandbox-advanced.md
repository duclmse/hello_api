# hello_sandbox — Advanced Features

This document covers advanced SDK packs and runtime capabilities beyond the core
Phases 0–9 foundation (Transpiler, Loader, CorePack, SharedRuntime, KvPack,
CryptoPack, HttpPack, RuntimePool, public API, V8 heap limits, seccomp).

---

## Feature Overview

| Feature                            | Complexity | Status      |
| ---------------------------------- | ---------- | ----------- |
| Structured Metrics & Observability | Low        | Implemented |
| KV Backend Trait                   | Medium     | Implemented |
| V8 Compilation Cache               | High       | Implemented |
| Per-Run Rate Limiting              | Low        | Implemented |
| SQLite Pack                        | Medium     | Implemented |
| V8 Snapshot Fast Init              | High       | Implemented |
| Real-Time Event Streaming          | Medium     | Implemented |
| TimerPack (setTimeout/setInterval) | High       | Implemented |
| V8 Inspector / Debug Bridge        | High       | Planned     |

---

## Feature Relationships

```
Phases 0-9 (foundation)
  │
  ├── Metrics & Observability ──► Rate Limiting ──► Streaming Events
  │                                                       │
  ├── KV Backend Trait ──► SQLite Pack              TimerPack
  │                                                       │
  ├── Compilation Cache                          V8 Inspector (planned)
  │
  └── Snapshot Fast Init
```

---

## Structured Metrics & Observability

Every `SandboxResult` includes a `RunMetrics` struct with:

- `peak_heap_bytes` — polled from `v8_isolate().get_heap_statistics()` after the
  event loop drains
- `elapsed` — wall-clock duration of the run
- Per-op counters (HTTP calls, KV ops, emit calls)
- `tags` — key-value metadata from `RunCapabilities.tags`

An optional `MetricsSink` trait (`Arc<dyn MetricsSink + Send + Sync>`) is stored
in `SandboxConfig` with a no-op default. It is called synchronously after result
extraction, adding zero latency. OpenTelemetry export is available behind
`feature = ["otel"]`.

**Critical files:** `src/runtime.rs`, `src/config.rs`, `src/sandbox.rs`

---

## KV Backend Trait

`KvStore` is backed by a `KvBackend` trait:

```rust
trait KvBackend: Send + Sync + 'static {
    async fn get(&self, key: &str) -> Option<Value>;
    async fn set(&self, key: &str, value: Value);
    async fn delete(&self, key: &str);
    async fn list(&self, prefix: &str) -> Vec<(String, Value)>;
}
```

- `KvPack::new()` (zero-arg) uses `InMemoryKvBackend` — zero breaking change
- `KvPack::new(backend: impl KvBackend)` accepts Redis, SQLite, or any custom
  backend
- `Arc` shared across slots for shared-storage mode; fresh `Arc` per slot for
  isolated mode (current default)
- The JS `kv.*` API is unchanged — ops became `#[op2(async)]` transparently

**Critical file:** `src/sdk/kv_sdk.rs`

---

## V8 Compilation Cache

Scripts with the same source (`sha256(wrapped_source)`) skip V8 parse and
compile on repeated runs:

- `CodeCache: HashMap<[u8;32], Vec<u8>>` in `Arc<Mutex<...>>` shared across pool
  slots
- `AllowlistModuleLoader` gains `Option<Arc<Mutex<CodeCache>>>`. On `load()`, a
  cache hit injects the bytecode blob into `ModuleSource.code_cache`
- Cache key includes the `deno_core` version string to auto-invalidate on
  upgrades
- Corrupted bytecode causes transparent fallback to recompile — never a panic

**Critical files:** `src/loader.rs`, `src/runtime.rs`

---

## Per-Run Rate Limiting

Numeric quotas checked synchronously inside ops before any I/O:

```rust
// SandboxConfig
pub struct RateLimitConfig {
    pub http_calls_per_run: Option<usize>,
    pub kv_ops_per_run: Option<usize>,
    pub emit_calls_per_run: Option<usize>,
}
```

- Counters live in `RunState` and reset each run
- Violations return `SandboxError::RateLimitExceeded { resource, limit }` — not
  a silent drop
- `RunMetrics` includes consumed-vs-limit counters so hosts can distinguish
  normal completion from limit-triggered termination

**Critical files:** `src/config.rs`, `src/runtime.rs`, `src/sdk/core_sdk.rs`,
`src/sdk/kv_sdk.rs`, `src/sdk/http_sdk.rs`

---

## SQLite Pack (`sandbox:sqlite`)

Per-slot in-memory SQLite database exposed to scripts:

```js
import { db } from "sandbox:sqlite";
const rows = db.query("SELECT * FROM users WHERE id = ?", [userId]);
db.execute("INSERT INTO logs VALUES (?, ?)", [ts, msg]);
```

- `SqliteStore { conn: rusqlite::Connection }` in `OpState`;
  `Connection::open_in_memory()` opened in `inject_op_state` — fresh DB per
  slot, cleared on slot recycle
- `op_db_query(sql, params) -> Vec<Vec<Value>>` and
  `op_db_execute(sql, params) -> usize` are synchronous (in-memory SQLite
  queries are microsecond-range)
- `rusqlite` with `features = ["bundled"]` — no system SQLite dependency
- `SqlitePack::new_file(path)` returns `Err` when isolation is `Untrusted`
  (seccomp blocks the `open()` syscall)

**New files:** `src/sdk/sqlite_sdk.rs`, `sdk-ts/src/sqlite.js`,
`sdk-ts/types/sqlite.d.ts`

---

## V8 Snapshot-Based Fast Init

Pre-baked SDK bootstrap embedded at compile time cuts per-slot cold-start from
~15 ms to ~2 ms:

- `tools/make_snapshot.rs` (xtask) evaluates `CorePack` bootstrap in a scratch
  `JsRuntime`, calls `JsRuntime::snapshot()`, writes the blob
- The blob is embedded via `include_bytes!` in `src/snapshot.rs` as
  `static SNAPSHOT: &[u8]`
- `RuntimeOptions::startup_snapshot = Some(deno_core::Snapshot::Static(SNAPSHOT))`
- `CorePack::esm_entry_point()` returns `None` when a snapshot is loaded
  (already evaluated)
- A snapshot version hash is embedded alongside; mismatch triggers fallback to
  non-snapshot init with `tracing::warn!`
- CI rebuilds the snapshot on any change to `sdk-ts/src/core.js` or `CorePack`

**New files:** `src/snapshot.rs`, `tools/make_snapshot.rs` **Modified:**
`src/runtime.rs`

---

## Real-Time Event Streaming

`Sandbox::run_streaming()` exposes the event channel before script completion:

```rust
let (result_future, event_rx) = sandbox.run_streaming(script, caps).await;
// stream events as they arrive
while let Some(event) = event_rx.recv().await { ... }
let result = result_future.await?;
```

- Internal plumbing already exists: `RunState.events` is an
  `UnboundedSender<SandboxEvent>`
- `SandboxResult.events` (`Vec`) remains for non-streaming callers — fully
  backward compatible
- `SandboxEvent: Send` so the receiver can be forwarded across threads
- Child-process streaming (Untrusted tier) is deferred pending a
  newline-delimited protocol extension for the stdin/stdout channel

**Critical files:** `src/pool.rs`, `src/sandbox.rs`, `src/runtime.rs`

---

## TimerPack (`setTimeout` / `setInterval`)

Sandbox-safe timer globals firing within the V8 event loop:

```js
setTimeout(() => sandbox.emit("tick", {}), 100);
const id = setInterval(() => { ... }, 50);
clearInterval(id);
```

- Ops: `op_timer_set(delay_ms, callback_id) -> timer_id`,
  `op_timer_clear(timer_id)`
- Per-slot state: `TimerStore { pending: BTreeMap<Instant, Vec<CallbackId>> }`
  (cleared between runs)
- Timer delays clamped to `min(delay, remaining_watchdog_budget)`. Post-deadline
  timers are silently cancelled
- `setInterval` bounded by `max_interval_calls: usize` in `SandboxConfig` to
  prevent infinite polling
- `globalThis` freeze compatibility: `SdkExtension` gained a
  `pre_freeze_globals()` default method that returns a JS snippet injected
  before the `core.js` freeze block

**New files:** `src/sdk/timer_sdk.rs`, `sdk-ts/src/timer.js`,
`sdk-ts/types/timer.d.ts` **Modified:** `sdk-ts/src/core.js`, `src/sdk/mod.rs`

---

## V8 Inspector / Debug Bridge (Planned)

Optional Chrome DevTools Protocol (CDP) WebSocket server per sandbox slot,
enabling breakpoints, step-through debugging, and heap snapshot capture via
Chrome DevTools.

**Design:**

- `SandboxBuilder::debug_port(port: u16)` — gated on `feature = ["inspector"]`
- `DebugBridge` wraps `JsRuntimeInspector` + `tokio::net::TcpListener`. A side
  task on the same `LocalSet` accepts WebSocket connections and shuttles CDP
  messages
- Debug mode forces `IsolationLevel::Trusted`; watchdog is disabled (a paused
  breakpoint would otherwise trigger timeout)
- Heap snapshots exposed as CDP `HeapProfiler.takeHeapSnapshot`
- One concurrent inspector connection per slot; second connection rejected with
  HTTP 503
- `feature = ["inspector"]` is zero-overhead when not enabled

**New files:** `src/debug.rs` **Cargo.toml addition:**
`tokio-tungstenite = { version = "0.21", optional = true }`
