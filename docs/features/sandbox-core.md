# hello_sandbox — Core Architecture

hello_sandbox is a production-grade JavaScript/TypeScript sandboxing library
built on `deno_core` (V8). It provides isolated, resource-controlled execution
of user scripts with a pluggable SDK pack system.

---

## Overview

The sandbox executes JS/TS scripts in isolated V8 environments with configurable
security tiers, resource limits, and opt-in capabilities. Scripts can read
host-provided inputs, emit events back to the host, and access only the SDK
packs explicitly granted to them. No filesystem, process, or network access
exists unless a pack provides it with allowlist enforcement.

---

## Component Layers

### Transpiler (`src/transpile.rs`)

Converts TypeScript (including TSX) to plain JavaScript using the `swc`
compiler. The function `looks_like_typescript` provides a fast heuristic check
before committing to a full parse. Transpilation happens at module registration
time, not at run time, so the cost is paid once.

- Entry:
  `transpile(specifier, source, force_ts) -> Result<String, SandboxError>`
- `force_ts = true` bypasses the heuristic and always transpiles
- Invalid TS returns `SandboxError::TranspileError`

### Module Loader (`src/loader.rs`)

`AllowlistModuleLoader` (built via `AllowlistModuleLoaderBuilder`) enforces the
allowlist of importable modules. All modules must be registered explicitly at
build time. Dynamic resolution from the network or filesystem is categorically
denied.

Resolution rules:

- `sandbox:*` specifiers are allowed and served from the compiled module map
- `ext:*`, `node:*`, `https:*`, `file:*`, and bare specifiers are hard-denied
- Relative `./helper` imports are allowed only when the referrer is a
  `sandbox:*` module
- The builder is `Clone` so multiple sandboxes can share the same module
  registry

### Core SDK Shim (`sdk-ts/src/core.js` via `CorePack`)

The bootstrap shim that every script runs inside. It is evaluated once before
any user code executes and sets up the invariant JS environment:

- Captures `Deno.core.ops` into a block-scoped variable before any freeze
- Installs `globalThis.console` (output captured into `SandboxResult.logs`)
- Installs `globalThis.sandbox` with `readInput`, `emit`, and `tags` methods
- Freezes all built-in prototypes (`Array`, `Object`, `Promise`, etc.)
- Deletes `globalThis.Deno` (ops are accessed via `globalThis.__sandbox_ops`)
- Freezes `globalThis` last

The shim file is 7-bit ASCII only (deno_core requirement for ESM entry points).

**Ops provided by `CorePack`:** `op_print`, `op_read_input`, `op_emit`

### SharedRuntime (`src/runtime.rs`)

One `JsRuntime` (one V8 isolate) that executes scripts end-to-end. Key
behaviors:

- Assembled from `SandboxConfig`, `AllowlistModuleLoader`, and registered SDK
  packs
- The `run(source, inputs, event_tx)` method runs the full execution pipeline:
  1. Module evaluation (transpile if needed, load dependencies)
  2. Script execution inside a `LocalSet` (V8 is `!Send`)
  3. `__RETURN__:` sentinel extraction from the log buffer for the return value
  4. Event draining from the channel
- A watchdog thread fires `terminate_execution` if the script exceeds its
  deadline
- `run_count()` tracks how many executions this runtime has served

The `__RETURN__:` sentinel is always the last matching log line (the injected
wrapper), so a user script that happens to log `__RETURN__:foo` does not corrupt
the result.

### SDK Pack System (`src/sdk/mod.rs`)

The `SdkExtension` trait defines the contract for optional capability packs.
Each pack contributes:

- Rust ops registered into the V8 extension
- Per-slot state injected into `OpState`
- A JS shim (served as a `sandbox:*` module)
- TypeScript declarations (served by the loader for editor support)

`SdkRegistry` collects packs and builds the combined `Extension`. `CorePack` is
always prepended automatically by `SandboxBuilder::build()`.

Available packs: `CorePack`, `KvPack`, `CryptoPack`, `HttpPack`, `SqlitePack`,
`TimerPack`, `AssertPack`, `PmPack`.

### RuntimePool (`src/pool.rs`)

Manages a pool of warm `SharedRuntime` slots to amortize V8 cold-start cost.
Each slot transitions through a state machine:

```
Idle → Checked-Out → Idle (success)
                  → Stale (runtime error)
Stale → recycled → new SharedRuntime (Idle)
```

Pool configuration via `PoolConfig`:

- `pool_size`: number of warm slots (0 = all runs use isolated fallback)
- `max_runs_per_slot`: recycled after N executions
- `max_idle_duration`: recycled if idle too long
- `fallback_to_isolated`: whether to create a one-shot runtime for concurrent
  overflow

The `Mutex` guarding pool state is never held across an `await` point. The
runtime is extracted from the guard, the guard dropped, the async run executed,
then the guard re-acquired for check-in.

### Public API (`src/sandbox.rs`)

`SandboxBuilder` is the ergonomic entry point:

```rust
Sandbox::builder()
    .sdk(KvPack)
    .sdk(HttpPack::new(HttpConfig { ... }))
    .module("sandbox:helpers", HELPERS_JS)
    .pool(PoolConfig::high_throughput())
    .build()
```

`Sandbox::run(script, caps)` wraps the pool with a wall-clock
`tokio::time::timeout` (distinct from the per-script watchdog) and drains the
event channel. `Sandbox::run_streaming()` returns the event receiver before the
script completes.

---

## Security Model

### Isolation Tiers

Three tiers configured via `SandboxConfig`:

| Tier      | `IsolationLevel` | Watchdog | V8 Heap Limit  | OS Isolation    |
| --------- | ---------------- | -------- | -------------- | --------------- |
| Trusted   | `Trusted`        | No       | None           | None            |
| PowerUser | `PowerUser`      | Yes      | 128 MB default | None            |
| Untrusted | `Untrusted`      | Yes      | 16 MB          | seccomp (Linux) |

### Watchdog

For `PowerUser` and `Untrusted` tiers, a background thread calls
`JsRuntime::terminate_execution()` after the configured timeout. The watchdog
uses an `AtomicBool` cancel flag that is always set before
`SharedRuntime::run()` returns (even on the error path) to prevent firing after
a successful run.

### V8 Heap Limits

`SharedRuntime::new()` passes `v8::CreateParams` with
`heap_limits(initial, max)` derived from `SandboxConfig`. When V8 triggers the
near-heap-limit callback, the sandbox returns `SandboxError::OutOfMemory`
instead of crashing the process.

### seccomp (Linux, Untrusted tier)

On Linux, `IsolationLevel::Untrusted` forks a child process. Before creating the
`JsRuntime`, the child installs a seccomp syscall allowlist. The parent
communicates via stdin/stdout JSON (`{ script, inputs, config }` in,
`{ value, logs, events }` out) and kills the child if it exceeds the timeout. On
non-Linux, the tier falls back to `PowerUser` with a `tracing::warn!`.

---

## Component Dependency Graph

```
Transpiler
  └─► Module Loader
        └─► Core SDK Shim
              └─► SharedRuntime
                    │
    ┌───────────────┼───────────────┐
  KvPack        CryptoPack       HttpPack
  SqlitePack    TimerPack        AssertPack
    └───────────────┼───────────────┘
                RuntimePool
                    │
              Public API (Sandbox)
    ┌───────────────┤
  V8 heap limits   OS sandbox (seccomp)
```

Packs are independently composable after `SharedRuntime` is in place.
`V8 heap limits` and `OS sandbox` are hardening layers on top of the working
runtime.

---

## Design Decisions & Mitigations

**`deno_core` version pinned exactly.** The `Extension::builder` dynamic op
registration API changes between minor versions. `deno_core = "0.380"` is
pinned. A compile-time smoke test catches breakage early.

**Watchdog raw pointer safety.** The `AtomicBool` cancel flag prevents
`terminate_execution` from firing after the runtime returns. Tests verify the
watchdog does not fire after a successful run.

**`Mutex` guard never held across `await`.** The pool extracts the runtime from
the guard, drops the guard, runs async, then re-acquires for check-in.
Concurrent pool tests verify this. The codebase is linted with a grep for
`lock().await` at call sites.

**Prototype freeze happens after shims are loaded.** `core.js` freezes
prototypes after SDK shims install their methods. Shims that use standard
`Array`/`Promise` methods run correctly before the freeze.

**`__RETURN__:` sentinel uses `rposition()`.** The last matching log line is
always the injected wrapper, so user scripts that happen to log the sentinel
string do not corrupt the return value.
