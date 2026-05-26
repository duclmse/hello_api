# Runtime Pool

`src/pool.rs` manages a pool of warm `SharedRuntime` (V8 isolate) slots to
amortize startup cost across multiple script runs.

## V8 Single-Thread Constraint

**Critical:** Only one V8 isolate can be "entered" on a thread at a time.
Creating multiple `JsRuntime` instances on the same thread causes a V8 fatal
crash. All tests must use `pool_size = 1` (or `PoolConfig::high_isolation()`).

In production, the pool is run on a `LocalSet` where tasks are dispatched
serially, so `pool_size > 1` is safe there — but tests run everything on one
thread.

---

## PoolConfig

Configuration for `RuntimePool`:

```rust
pub struct PoolConfig {
    pub pool_size: usize,           // number of warm slots (default: 4)
    pub max_runs_per_slot: usize,   // recycle after N runs (default: 100)
    pub max_idle_duration: Duration,// recycle after idle for this long (default: 60s)
    pub fallback_to_isolated: bool, // if no slot available, run isolated (default: true)
}
```

### Presets

| Preset                          | pool_size | Notes                                            |
| ------------------------------- | --------- | ------------------------------------------------ |
| `PoolConfig::default()`         | 4         | General purpose                                  |
| `PoolConfig::high_throughput()` | 8         | More warm slots for latency-sensitive apps       |
| `PoolConfig::high_isolation()`  | 0         | Every run gets a fresh isolate (safest, slowest) |

`pool_size = 0` disables the pool; every run creates and destroys a
`SharedRuntime`.

---

## RuntimeKind

Indicates which execution path was taken:

```rust
pub enum RuntimeKind {
    Warm { slot: usize },  // reused slot from the pool
    Isolated,              // fresh isolate (pool_size=0 or fallback)
}
```

Available in `SandboxResult::runtime_kind`.

---

## Slot Lifecycle

```
                  ┌──────────┐
                  │  Idle    │ ← slot available
                  └─────┬────┘
          checkout      │           recycle (error / max_runs / max_idle)
                  ┌─────▼─────┐
                  │ CheckedOut│ ← running a script
                  └─────┬─────┘
         check in (ok)  │
                  ┌─────▼─────┐
     register_    │   Stale   │ ← module or loader changed; needs rebuild
     module()     └─────┬─────┘
                        │ rebuild on next check-out
                  ┌─────▼─────┐
                  │   Idle    │
                  └───────────┘
```

- **Idle** — available for checkout
- **CheckedOut** — running a script; cannot be checked out again
- **Stale** — module loader was updated; slot will be recycled on next checkout

A slot is always marked **Stale** after a script error to prevent state leakage.

---

## RuntimePool

```rust
pub struct RuntimePool { ... }
```

### Run Methods

```rust
// Basic run — returns SandboxResult
pool.run(script, inputs).await?

// Run with per-run capability constraints
pool.run_with_caps(script, inputs, capabilities).await?

// Streaming run
let (future, rx) = pool.run_streaming(script, inputs);

// Streaming with capabilities
let (future, rx) = pool.run_streaming_with_caps(script, inputs, capabilities);
```

All methods return `Result<SandboxResult, SandboxError>`.

### PoolStats

```rust
let stats: PoolStats = pool.stats();
```

Reports current counts of idle, checked-out, and stale slots.

---

## Warm Slot State Pollution

Scripts running in warm slots share the module-level state of previously loaded
ESM modules. The `sandbox:test` module's `results()` function deliberately
resets `_failures = []` before returning to prevent test state from leaking
across runs in the same slot.

Any mutable state in user modules must be explicitly reset. The recommended
pattern is to reset at the start of each script or use the `results()` reset
convention.

---

## Source

`src/pool.rs`
