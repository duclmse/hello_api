# Phase 16 — Real-Time Event Streaming

## Overview

Phase 16 adds `Sandbox::run_streaming()` (and the lower-level
`RuntimePool::run_streaming()`), enabling hosts to receive `SandboxEvent`s as
they are emitted by scripts rather than waiting for the entire run to complete.

Before Phase 16, events were batch-collected after `SharedRuntime::run()`
returned and delivered via `SandboxResult::events`. With streaming, the host
gets an `UnboundedReceiver<SandboxEvent>` _before_ the script starts, letting it
react to events in real time.

---

## API

### `Sandbox::run_streaming`

```rust
pub fn run_streaming<'s>(
    &'s mut self,
    script: &'s str,
) -> (
    impl Future<Output = Result<SandboxResult, SandboxError>> + 's,
    UnboundedReceiver<SandboxEvent>,
)
```

Returns `(future, receiver)` **immediately** — the script has not started yet.

| Return value | Type                              | Send?   | Notes                                       |
| ------------ | --------------------------------- | ------- | ------------------------------------------- |
| `future`     | `impl Future<…>`                  | `!Send` | Must be `.await`ed on the owning `LocalSet` |
| `receiver`   | `UnboundedReceiver<SandboxEvent>` | `Send`  | Can be forwarded to other threads/tasks     |

`SandboxResult::events` is **always empty** when using this method — events go
exclusively to the receiver.

### `RuntimePool::run_streaming` (lower-level)

```rust
pub fn run_streaming<'s>(
    &'s self,
    source: &'s str,
    inputs: HashMap<String, Value>,
) -> (
    impl Future<Output = Result<SandboxResult, SandboxError>> + 's,
    UnboundedReceiver<SandboxEvent>,
)
```

Same contract as `Sandbox::run_streaming`, but operates directly on the pool.
`Sandbox::run_streaming` delegates here after pool initialisation.

---

## Usage

### Basic streaming

```rust
use tokio::task::LocalSet;
use hello_sandbox::{Sandbox, SandboxConfig, PoolConfig};

let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
LocalSet::new().block_on(&rt, async {
    let mut sandbox = Sandbox::builder()
        .config(SandboxConfig::trusted())
        .pool(PoolConfig { pool_size: 1, ..Default::default() })
        .build()
        .unwrap();

    let (fut, mut rx) = sandbox.run_streaming(r#"
        sandbox.emit("progress", { step: 1 });
        sandbox.emit("progress", { step: 2 });
        sandbox.emit("done",     { ok: true });
        return "finished";
    "#);

    // Consume events concurrently with the running script.
    let event_task = tokio::task::spawn_local(async move {
        while let Some(event) = rx.recv().await {
            println!("live: {} {:?}", event.name, event.payload);
        }
    });

    let result = fut.await.unwrap();
    event_task.await.unwrap();

    assert_eq!(result.value, serde_json::json!("finished"));
    assert!(result.events.is_empty()); // events were streamed, not batched
});
```

### Forwarding events to another thread

The receiver is `Send`, so it can be moved into a regular `tokio::spawn` task:

```rust
let (fut, rx) = sandbox.run_streaming(script);

// Forward events to a non-LocalSet thread.
let handle = tokio::spawn(async move {
    let mut rx = rx;
    while let Some(event) = rx.recv().await {
        // process event on a regular multi-threaded executor
        println!("{}", event.name);
    }
});

let result = fut.await?;
handle.await?;
```

### Interleaving with `run()`

`run()` and `run_streaming()` can be called alternately on the same `Sandbox`.
`run()` still batch-collects events into `SandboxResult::events`; only
`run_streaming()` uses the live receiver.

```rust
// Batch run — events in SandboxResult.events
let r1 = sandbox.run(script_a).await?;
println!("{} batched events", r1.events.len());

// Streaming run — events in receiver, SandboxResult.events is empty
let (fut, mut rx) = sandbox.run_streaming(script_b);
let r2 = fut.await?;
assert!(r2.events.is_empty());
```

---

## Architecture

```
Caller
  │
  │  sandbox.run_streaming(script)
  │  ─────────────────────────────►  Sandbox::run_streaming()
  │                                     │  (sync — no await)
  │                                     │  1. init pool if needed
  │                                     │  2. create (event_tx, event_rx)
  │                                     │  3. call pool.run_streaming(script, tx)
  │◄──── (future, event_rx) ────────────┘
  │
  │  // event_rx is now in caller's hands
  │  // future has NOT started yet
  │
  │  fut.await ────────────────────────►  RuntimePool::run_streaming_impl()
  │                                           │  execute_run(script, tx) ──►  SharedRuntime::run()
  │                                           │                                 │  (event_tx in RunState/OpState)
  │◄── SandboxEvent via rx.recv().await ◄─────┼─── sandbox.emit() fires ────────┘
  │◄── SandboxEvent via rx.recv().await ◄─────┘
  │
  │  // SharedRuntime::run() completes:
  │  //   1. RunState taken out of OpState → event_tx DROPPED
  │  //   2. rx.recv() returns None (channel closed)
  │
  │◄──── Ok(SandboxResult { events: vec![], … }) ─────────────────────────────
```

### Key invariant: RunState cleanup

Before Phase 16, `SharedRuntime::run()` left `RunState` (containing `event_tx`)
in `OpState` between runs. For isolated runtimes (`pool_size: 0`) this was
harmless — the `JsRuntime` was dropped after each run, which dropped `RunState`
and `event_tx`.

For warm pool slots, `JsRuntime` is reused across runs. Without cleanup,
`event_tx` from run N would stay alive until run N+1 replaced it via
`op_state.put(RunState {...})`, causing the streaming receiver from run N to
hang until run N+1 started.

**Fix:** `SharedRuntime::run()` now calls `op_state.try_take::<RunState>()`
immediately after the event loop drains (after extracting all needed fields).
This drops `event_tx` and closes the streaming receiver regardless of whether
the runtime is discarded or returned to the pool.

This change is **backward compatible** — `run()` is unaffected (its own
`event_rx` is drained right after `run()` returns anyway).

---

## Files Changed

| File                         | Change                                                                                                                                         |
| ---------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------- |
| `src/runtime.rs`             | Restructured post-event-loop cleanup: extract RunState fields, then `try_take::<RunState>()` to drop `event_tx`                                |
| `src/pool.rs`                | Added `execute_isolated()`, `execute_run()` (shared helpers); refactored `run()` to use them; added `run_streaming()` + `run_streaming_impl()` |
| `src/sandbox.rs`             | Added `Sandbox::run_streaming()` with `UnboundedReceiver` import                                                                               |
| `tests/streaming_tests.rs`   | 13 integration tests                                                                                                                           |
| `docs/phase-16-streaming.md` | This document                                                                                                                                  |

---

## Design Decisions

### Non-async `run_streaming` returning `(impl Future, Receiver)`

`run_streaming` is a **synchronous** function that creates the channel and
returns both the future and the receiver atomically. This is critical: if
`run_streaming` were `async`, the caller would need to await it before getting
the receiver, but by then the future might already have started (or even
completed).

The synchronous signature guarantees that:

1. The receiver is in the caller's hands before the script's first op is called.
2. The script cannot emit to the channel until the future is first polled.

### `!Send` future, `Send` receiver

`RuntimePool` and `JsRuntime` are `!Send` because V8 isolates are thread-affine.
The future returned by `run_streaming` borrows the pool (`&'s RuntimePool`), so
it is also `!Send`.

`UnboundedReceiver<SandboxEvent>` is `Send` because `SandboxEvent` is `Send`.
This lets hosts forward events to any thread — e.g., writing to a WebSocket from
a regular async task while the V8 execution stays on its `LocalSet`.

### `SandboxResult::events` is empty when streaming

When `run_streaming` is used, `SandboxResult::events` is always `vec![]`. This
is intentional:

- Collecting events into the Vec would require buffering all events in memory
  until the run completes, defeating the purpose of streaming.
- It keeps the API contract simple: streaming mode sends events live; batch mode
  (`run()`) collects them.
- Existing code using `run()` is unaffected — `SandboxResult::events` continues
  to be populated normally.

### Untrusted isolation deferred

Child-process isolation (`IsolationLevel::Untrusted`) requires a
newline-delimited streaming protocol between the parent and worker processes,
which is a significant protocol change. When `run_streaming()` is called with
`IsolationLevel::Untrusted`, a warning is logged and execution falls back to the
in-process pool path. Events still stream correctly; only the seccomp-level
isolation is missing. This will be addressed in a future phase.

---

## Comparison: `run()` vs `run_streaming()`

|                        | `run()`                         | `run_streaming()`                              |
| ---------------------- | ------------------------------- | ---------------------------------------------- |
| Returns                | `Future<Result<SandboxResult>>` | `(Future<Result<SandboxResult>>, Receiver)`    |
| `SandboxResult.events` | Populated after script ends     | Always empty                                   |
| Event delivery         | Batch (all at once)             | Live (as emitted)                              |
| Suitable for           | Simple request/response         | Progress bars, pipelines, long-running scripts |
| `Future: Send?`        | `!Send`                         | `!Send`                                        |
| `Receiver: Send?`      | N/A                             | `Send` ✓                                       |
| Untrusted (Linux)      | Child process (seccomp)         | In-process pool (warning logged)               |
