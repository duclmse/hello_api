# KvPack — Key-Value Store

`src/sdk/kv_sdk.rs` + `sdk-ts/src/kv.js`

KvPack provides a per-slot key-value store accessible from scripts. The backing store is pluggable via the `KvBackend` trait.

---

## Registration

```rust
use hello_sandbox::{SandboxBuilder, SandboxConfig};
use hello_sandbox::sdk::kv_sdk::{KvPack, InMemoryKvBackend};

let sandbox = SandboxBuilder::new()
    .config(SandboxConfig::power_user())
    .sdk(KvPack::default())          // in-memory backend
    // or:
    .sdk(KvPack::with_backend(my_backend))  // custom backend
    .build()?;
```

---

## JavaScript API

```js
import { kv } from "sandbox:kv";

// Set a value (any JSON-serializable value)
await kv.set("user:42", { name: "Alice", active: true });

// Get a value
const user = await kv.get("user:42");
// → { name: "Alice", active: true }
// → null if not found

// Delete a value
await kv.delete("user:42");

// List all keys with a prefix
const keys = await kv.list("user:");
// → ["user:42", "user:99"]
```

All methods return Promises. Values are JSON-serialized before storage.

---

## KvBackend Trait

```rust
pub trait KvBackend: Send + Sync + 'static {
    fn get(&self, key: String) -> BoxFuture<'_, Option<Value>>;
    fn set(&self, key: String, value: Value) -> BoxFuture<'_, ()>;
    fn delete(&self, key: String) -> BoxFuture<'_, ()>;
    fn list(&self, prefix: String) -> BoxFuture<'_, Vec<String>>;
}
```

Uses `BoxFuture` to avoid the `async-trait` dependency.

### InMemoryKvBackend

The default backend. Thread-safe in-process `HashMap` wrapped in `Arc<Mutex<_>>`.

```rust
// In-memory backend shared across all slots
let backend = Arc::new(InMemoryKvBackend::new());
let pack = KvPack::with_backend(backend);
```

State is shared across all pool slots within a process. To isolate slots, use `KvPack::default()` (each slot creates its own in-memory backend independently via the factory function).

### Custom Backend

```rust
#[derive(Clone)]
struct RedisBackend { client: redis::Client }

impl KvBackend for RedisBackend {
    fn get(&self, key: String) -> BoxFuture<'_, Option<Value>> {
        Box::pin(async move { /* redis GET */ })
    }
    // ...
}

let pack = KvPack::with_backend(RedisBackend { client });
```

---

## RunCapabilities Interaction

KV operations are subject to `RunCapabilities`:

| Capability | Effect |
|------------|--------|
| `kv_enabled = Some(false)` | All ops return `CapabilityDenied` |
| `kv_key_prefix = Some("user:42:")` | Prefix prepended to all keys; stripped from list results |
| `kv_ops_limit = Some(n)` | `RateLimitExceeded` after n ops |

KV key prefix example:
```js
// With kv_key_prefix = "user:42:"
await kv.set("prefs", value);      // actually stores at "user:42:prefs"
const keys = await kv.list("");    // returns ["prefs"] (prefix stripped)
```

This namespacing ensures different runs cannot interfere with each other's KV data.

---

## Ops

| Op | Direction | Description |
|----|-----------|-------------|
| `op_kv_get(key)` | async | Retrieve value by key |
| `op_kv_set(key, value_json)` | async | Store value |
| `op_kv_delete(key)` | async | Remove key |
| `op_kv_list(prefix)` | async | List keys matching prefix |

---

## TypeScript Declarations

```typescript
// sdk-ts/types/kv.d.ts
export declare const kv: {
    get(key: string): Promise<unknown>;
    set(key: string, value: unknown): Promise<void>;
    delete(key: string): Promise<void>;
    list(prefix: string): Promise<string[]>;
};
```

---

## Source

`src/sdk/kv_sdk.rs`
`sdk-ts/src/kv.js`
`sdk-ts/types/kv.d.ts`
