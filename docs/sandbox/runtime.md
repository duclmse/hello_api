# SharedRuntime & RunState

`src/runtime.rs` contains the core V8 execution logic. Each pool slot holds one
`SharedRuntime`.

## RunState

`RunState` is injected into `OpState` at the start of every `run()` call and
removed at the end. It provides per-run mutable state for all ops.

```rust
pub struct RunState {
    // Inputs set by sandbox.set_input() / run's input map
    pub inputs: HashMap<String, Value>,

    // Output accumulation
    pub logs: Vec<String>,
    pub event_tx: Option<UnboundedSender<SandboxEvent>>,

    // Rate limiting counters
    pub http_calls: usize,
    pub kv_ops: usize,
    pub emit_calls: usize,
    pub rate_limit_exceeded: bool,

    // Timer state (TimerPack)
    pub timers: HashMap<u32, oneshot::Sender<()>>,
    pub max_interval_calls: usize,

    // Capability constraints for this run
    pub capabilities: RunCapabilities,

    // Per-run metadata
    pub tags: HashMap<String, String>,

    // Assertion tracking (AssertPack)
    pub assert_passed: usize,
    pub assert_failed: usize,

    // pm.test() results (PmPack)
    pub pm_tests: Vec<PmTestResult>,
}
```

`RunState` is replaced entirely for each run. Ops access it via
`state.borrow_mut::<RunState>()`. It must never be cached across an `await`
point.

At the end of every run, `SharedRuntime::run()` calls
`op_state.try_take::<RunState>()` to:

1. Drop `event_tx` immediately, closing the streaming receiver
2. Extract metrics, logs, and events
3. Prevent state leakage into the next run on warm slots

---

## SharedRuntime

One `SharedRuntime` per pool slot. Wraps a `JsRuntime` and manages the full
lifecycle of a single script execution.

### Construction

```rust
SharedRuntime::new(config, loader, sdk_registry, snapshot) -> Result<Self>
```

- Configures heap limits via `v8::CreateParams::heap_limits(initial, max)`
- Installs the near-heap-limit callback (`OomCallbackData`)
- Registers all SDK extension ops and ESM files
- Applies V8 snapshot if available (skips JS extension files that are in the
  snapshot)

### `run()`

```rust
async fn run(
    &mut self,
    script: &str,
    inputs: HashMap<String, Value>,
    event_tx: Option<UnboundedSender<SandboxEvent>>,
    capabilities: RunCapabilities,
) -> Result<(Value, Vec<String>, RunMetrics), SandboxError>
```

Execution flow:

1. Build and inject `RunState` into `OpState`
2. Spawn watchdog thread (if `PowerUser` or `Untrusted` isolation)
3. Wrap script in the `__RETURN__:` sentinel template
4. Load and execute the script via `load_side_es_module_from_code`
5. Drive the event loop to completion
6. Extract the return value from the last `__RETURN__:` log line
7. Check for OOM flag (set by heap limit callback)
8. Check for timeout flag (set by watchdog)
9. `try_take::<RunState>()` — collect metrics, close event channel
10. Return `(value, logs, metrics)`

### Return Value Sentinel

Scripts return values via `console.log`:

```
console.log("__RETURN__:" + JSON.stringify(__result ?? null))
```

The wrapper template injects this line. `runtime.rs` extracts the last log line
matching `__RETURN__:` and JSON-decodes it as `SandboxResult::value`.

### Watchdog Thread

For `PowerUser` and `Untrusted` isolation levels, a watchdog thread is spawned
before the event loop. It sleeps for the effective timeout duration, then calls
`isolate_handle.terminate_execution()` and sets a `timeout_flag` atomic.

The effective timeout is:

```
capabilities.timeout_override.unwrap_or(config.timeout)
```

### OOM Callback

V8 calls `near_heap_limit_callback` when the heap approaches its limit. The
callback:

1. Sets `oom_detected` atomic flag
2. Calls `isolate_handle.terminate_execution()`

`run()` checks `oom_detected` before `timeout_flag` and returns
`SandboxError::OutOfMemory` if set.

`OomCallbackData` is stored in a `Box` field `_oom_data` of `SharedRuntime`.
Drop order ensures V8 is torn down before the callback data is freed.

---

## Module Loading

Scripts are loaded via `load_side_es_module_from_code` (not
`load_main_es_module_from_code`). This allows the same `JsRuntime` slot to run
multiple scripts across its lifetime.

Scripts are wrapped in an ESM module:

```js
// wrapper injected by runtime.rs
let __result = await (async () => {
  // ... user script ...
})();
console.log("__RETURN__:" + JSON.stringify(__result ?? null));
```

---

## Source

`src/runtime.rs`
