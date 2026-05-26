# deno_core Sandbox

A production-oriented JS/TS sandbox built on `deno_core`, with three
configurable trust levels, a shared persistent runtime, and module/event
support.

## Architecture

```
Host (Rust/Tokio LocalSet)
└─ Sandbox
    ├─ SandboxConfig  (IsolationLevel, limits, feature flags)
    ├─ AllowlistModuleLoader  (sandbox: scheme only)
    └─ SharedRuntime  (one JsRuntime, many runs)
        ├─ sandbox_ext (bootstrap.js)
        │   ├─ op_print        → Vec<String> log buffer
        │   ├─ op_read_input   → host-supplied JSON inputs
        │   └─ op_emit         → mpsc::UnboundedSender<SandboxEvent>
        └─ per-run isolated ES module  (sandbox:run/<id>)
            ├─ transpile TS → JS (deno_ast)
            └─ user script (async IIFE, last expr returned)
```

No `fetch`, no `Deno.*`, no filesystem. `globalThis` is frozen post-bootstrap.

## Trust levels

|                 | `Trusted` | `PowerUser`            | `Untrusted`              |
| --------------- | --------- | ---------------------- | ------------------------ |
| Heap max        | 256 MB    | 64 MB                  | 16 MB                    |
| Timeout         | 30 s      | 10 s                   | 5 s                      |
| Watchdog thread | —         | ✅ terminate_execution | ✅                       |
| OS sandbox      | —         | —                      | seccomp+landlock (Linux) |
| Modules         | ✅        | ✅                     | configurable             |

## Features

|                                              | Status                              |
| -------------------------------------------- | ----------------------------------- |
| Isolated scopes per run (shared runtime)     | ✅ `sandbox:run/<id>` modules       |
| TypeScript transpilation                     | ✅ `deno_ast`                       |
| Allowlist module imports (`sandbox:` scheme) | ✅                                  |
| Streaming events host ← script               | ✅ `sandbox.emit(name, payload)`    |
| Wall-clock timeout                           | ✅ `tokio::time::timeout`           |
| CPU-spin timeout                             | ✅ watchdog + `terminate_execution` |
| Log/event quota                              | ✅ per-run limit                    |
| Named host→script inputs                     | ✅ `sandbox.readInput(key)`         |
| Child-process isolation (Linux)              | ✅ skeleton; needs seccomp rules    |

## Remaining work

- **V8 heap constraints** — wire
  `v8::CreateParams::new().heap_limits(initial, max)` into `RuntimeOptions` to
  get hard OOM kills instead of just GC pressure.
- **seccomp/landlock rules** — the child-process path forks the binary but
  doesn't yet install the syscall filter. Use the `seccompiler` crate.
- **Persistent module-level state** — scripts that want session state today must
  use `sandbox.readInput` / `sandbox.emit`. A `sandbox.getState()` /
  `setState()` round-trip op would be a clean addition.
- **Return-value schema validation** — validate the JSON result against a
  caller-supplied JSON Schema before surfacing it.
- **Metrics** — expose heap usage and op call counts via the `tracing` spans
  already in place.
