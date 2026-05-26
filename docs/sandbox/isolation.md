# Isolation Levels & Child-Process Worker

The sandbox supports three isolation levels with different security/performance
trade-offs.

## IsolationLevel

```rust
pub enum IsolationLevel {
    Trusted,
    PowerUser,   // default
    Untrusted,
}
```

| Level       | Process       | Watchdog      | seccomp     | Use Case                               |
| ----------- | ------------- | ------------- | ----------- | -------------------------------------- |
| `Trusted`   | In-process    | No            | No          | Internal scripts you fully control     |
| `PowerUser` | In-process    | Yes (timeout) | No          | Semi-trusted scripts, typical use case |
| `Untrusted` | Child process | Yes           | Yes (Linux) | User-provided scripts, untrusted code  |

---

## Trusted

No watchdog thread. Scripts run directly in the calling process with no timeout
enforcement beyond normal Tokio cancellation. Intended for internal, controlled
scripts only.

---

## PowerUser

Runs the script in-process but spawns a watchdog thread before each run:

```
main thread           watchdog thread
    │                       │
    ├─ spawn watchdog ──────►│
    │                       │ sleep(effective_timeout)
    ├─ run script            │
    │                       │ timeout!
    │                       ├─ isolate_handle.terminate_execution()
    ├─ event loop ends       │ set timeout_flag
    └─ check timeout_flag    │
```

The watchdog holds an `IsolateHandle` (a `Send + Clone` V8 handle obtained via
`JsRuntime::v8_isolate().thread_safe_handle()`). It can safely call
`terminate_execution()` from a different thread.

---

## Untrusted — Child Process Isolation

On Linux, `IsolationLevel::Untrusted` spawns a `sandbox-worker` child process
for each run. Scripts execute in an isolated process with a seccomp filter
installed.

On non-Linux platforms, a warning is emitted and execution falls back to
`PowerUser` (in-process).

### Architecture

```
Parent process                     Child process (sandbox-worker)
─────────────────                  ─────────────────────────────
Sandbox::run()
  │
  ├─ serialize WorkerRequest
  │   { script, inputs, timeout_ms,
  │     heap sizes, flags }
  │
  ├─ spawn sandbox-worker ─────►  worker.rs: run_worker()
  │   stdin/stdout JSON protocol      │
  │                                   ├─ deserialize WorkerRequest
  │                                   ├─ install seccomp filter
  │                                   ├─ build SandboxBuilder
  │                                   ├─ run script
  │                                   └─ serialize WorkerResponse
  │
  ├─ read stdout (WorkerResponse) ◄──
  └─ return SandboxResult
```

### Wire Protocol

**WorkerRequest** (sent to child via stdin):

```rust
pub struct WorkerRequest {
    pub script: String,
    pub inputs: HashMap<String, Value>,
    pub timeout_ms: u64,
    pub initial_heap_bytes: usize,
    pub max_heap_bytes: usize,
    pub max_log_lines: usize,
    pub typescript_enabled: bool,
    pub modules_enabled: bool,
    pub events_enabled: bool,
}
```

**WorkerResponse** (returned from child via stdout):

Contains the script result, logs, events, metrics, or an error string.

### seccomp Filter

Installed in the child process **after** V8 initialization but **before** script
execution. Restricts available syscalls to a safe allowlist (memory management,
V8 internals, etc.). Attempts to use blocked syscalls (e.g., opening files,
creating sockets) are killed with `SIGSYS`.

**Note:** File-backed `SqlitePack` will fail under `Untrusted` isolation because
`open()` is blocked by seccomp.

### Worker Binary Location

The worker binary is found via:

1. `SANDBOX_WORKER_BIN` environment variable
2. A `sandbox-worker` binary adjacent to the main executable
3. `PATH` lookup

Override via `SandboxBuilder::worker_binary(path)`.

To build the worker:

```bash
cargo build --bin sandbox-worker
```

### Streaming Not Supported

`run_streaming` with `IsolationLevel::Untrusted` emits a warning and falls back
to in-process execution. The child-process path does not support real-time event
streaming.

---

## Source

`src/child.rs` — child-process protocol, seccomp filter,
`run_in_child_process()` `src/bin/worker.rs` — thin binary calling
`hello_sandbox::child::run_worker()`
