# Sandbox — Public Entry Point

`src/sandbox.rs` is the public-facing API for the sandbox engine. It wraps
`RuntimePool` with a convenient builder and manages lazy pool creation.

## SandboxResult

Returned by every `run*` method:

```rust
pub struct SandboxResult {
    pub value: serde_json::Value,   // JSON-decoded return value of the script
    pub logs: Vec<String>,          // console.log / console.error output
    pub events: Vec<SandboxEvent>,  // events emitted via sandbox.emit()
                                    // (always empty when using run_streaming)
    pub elapsed: Duration,
    pub runtime_kind: RuntimeKind,  // Warm { slot } | Isolated
    pub metrics: RunMetrics,        // heap, timing, call counters, tags
}
```

---

## SandboxBuilder

Fluent builder for constructing a `Sandbox`.

```rust
let mut sandbox = SandboxBuilder::new()
    .config(SandboxConfig::power_user())
    .pool(PoolConfig::default())
    .sdk(KvPack::default())
    .sdk(HttpPack::new(HttpConfig { ... }))
    .input("user_id", json!(42))
    .module("sandbox:my_lib", "export function greet(n) { return 'hi ' + n; }")
    .worker_binary("/usr/local/bin/sandbox-worker")  // optional override
    .build()?;
```

### Builder Methods

| Method                       | Description                                                   |
| ---------------------------- | ------------------------------------------------------------- |
| `.config(SandboxConfig)`     | Isolation level, timeout, heap limits, etc.                   |
| `.pool(PoolConfig)`          | Pool size, slot lifetime, fallback policy                     |
| `.sdk(impl SdkExtension)`    | Register an SDK pack (KvPack, HttpPack, etc.)                 |
| `.input(key, value)`         | Set a named input available to all scripts                    |
| `.module(specifier, source)` | Register a user ESM module                                    |
| `.worker_binary(path)`       | Override the `sandbox-worker` binary path                     |
| `.build()`                   | Construct the `Sandbox` (pool is created lazily on first run) |

`SandboxBuilder::new()` automatically includes `CorePack`. You do not need to
register it manually.

---

## Sandbox

```rust
pub struct Sandbox { ... }
```

### Inputs

```rust
sandbox.set_input("key", json!({ "name": "Alice" }));
```

Sets a named input value available to scripts via `sandbox.readInput("key")`.
Calling `set_input` on a live pool updates the input for future runs; it does
not affect currently-executing scripts.

### Module Registration

```rust
sandbox.register_module("sandbox:helpers", js_source);
```

Registers a user ESM module. If called before the first `run()`, the module is
stored in the loader builder. If called after, all idle pool slots are marked
Stale so they are recycled with the updated module loader on next use.

### Run Methods

All run methods require a `LocalSet` (V8 is not `Send`):

```rust
// Basic run
let result: SandboxResult = sandbox.run(script).await?;

// Run with per-run capability constraints
let result = sandbox.run_with_caps(script, capabilities).await?;

// Streaming run — events arrive on the receiver as they are emitted
let (future, mut rx) = sandbox.run_streaming(script);
tokio::spawn(async move {
    while let Some(event) = rx.recv().await {
        println!("event: {} = {}", event.name, event.payload);
    }
});
let result = future.await?;

// Streaming with capabilities
let (future, rx) = sandbox.run_streaming_with_caps(script, capabilities);
```

### Script Return Value

A script communicates its return value via the `__RETURN__:` sentinel:

```js
const data = sandbox.readInput("items");
const filtered = data.filter((x) => x.active);
return filtered; // the wrapper injects: console.log("__RETURN__:" + JSON.stringify(result))
```

`SandboxResult::value` is `null` if the script returns nothing.

---

## Lazy Pool Creation

The `RuntimePool` (holding V8 isolates) is not created until the first `run()`
call. This means `SandboxBuilder::build()` is cheap and synchronous; the first
run pays the V8 startup cost (or not, if a snapshot is available).

```
SandboxBuilder::build() → Sandbox { pool: None, ... }
sandbox.run(script)      → creates pool on first call → RuntimePool
sandbox.run(script)      → pool already exists → reuse
```

---

## Thread Safety

`Sandbox` and `RuntimePool` are `!Send` — they must stay on the same thread they
were created on. Use `tokio::task::LocalSet` or `tokio::task::spawn_local`.

```rust
let local = tokio::task::LocalSet::new();
local.run_until(async move {
    let mut sandbox = SandboxBuilder::new().build()?;
    let result = sandbox.run("return 42;").await?;
    println!("{}", result.value);
    Ok::<_, SandboxError>(())
}).await?;
```
