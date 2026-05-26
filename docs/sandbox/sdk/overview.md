# SDK Overview — SdkExtension Trait & SdkRegistry

`src/sdk/mod.rs` defines the plugin interface for sandbox capabilities.

## SdkExtension Trait

Every SDK pack implements this trait:

```rust
pub trait SdkExtension {
    /// Unique name, becomes the `sandbox:<name>` module specifier.
    fn name(&self) -> &'static str;

    /// deno_core op declarations contributed by this pack.
    fn ops(&self) -> Vec<OpDecl>;

    /// ESM source files: (specifier, js_source) pairs.
    fn esm_files(&self) -> Vec<(&'static str, &'static str)>;

    /// TypeScript declarations for editor tooling (optional).
    fn ts_declarations(&self) -> &'static str { "" }

    /// ESM entry point specifier, if this pack provides one (CorePack only).
    fn esm_entry_point(&self) -> Option<&'static str> { None }

    /// Whether this pack is compatible with the pre-baked V8 snapshot (default: true).
    fn snapshot_compatible(&self) -> bool { true }

    /// JS code to inject into core.js before Object.freeze(globalThis).
    /// Used to install globals (e.g., timer functions) before the freeze.
    fn pre_freeze_globals(&self) -> Option<&'static str> { None }

    /// Called once per pool slot after the JsRuntime is created.
    /// Use to initialize per-slot state in OpState (e.g., SqliteStore, HttpState).
    fn inject_op_state(&self, op_state: &mut OpState) {}
}
```

---

## SdkRegistry

Manages the ordered list of packs for a `RuntimePool`. `CorePack` is always first.

```rust
let registry = SdkRegistry::new()
    .with(KvPack::default())
    .with(HttpPack::new(config))
    .with(CryptoPack)
    .with(SqlitePack::new());
```

Packs are registered in order. During runtime construction, the registry:
1. Collects all ops from all packs
2. Registers all ESM files in the loader
3. Sets the entry point (from CorePack)
4. Calls `inject_op_state` for each pack on every new slot

---

## Built-In Packs

| Pack | Module | Always Included | Source |
|------|--------|----------------|--------|
| `CorePack` | — | Yes | `core_sdk.rs` |
| `KvPack` | `sandbox:kv` | No | `kv_sdk.rs` |
| `HttpPack` | `sandbox:http` | No | `http_sdk.rs` |
| `CryptoPack` | `sandbox:crypto` | No | `crypto_sdk.rs` |
| `SqlitePack` | `sandbox:sqlite` | No | `sqlite_sdk.rs` |
| `TimerPack` | globals | No | `timer_sdk.rs` |
| `AssertPack` | `sandbox:assert` | No | `assert_sdk.rs` |
| `PmPack` | `sandbox:pm` | No | `pm_sdk.rs` |

`CorePack` is injected automatically by `SandboxBuilder`. You should not add it manually.

---

## Ops Architecture

Each pack contributes Rust ops via `#[op2]` macros. Ops are synchronous or async:

```rust
// Synchronous fast op (primitives only)
#[op2(fast)]
fn op_my_fast(state: &mut OpState, #[smi] n: u32) -> u32 { ... }

// Synchronous op with serde types
#[op2]
#[serde]
fn op_my_serde(state: &mut OpState, #[serde] input: MyInput) -> Result<MyOutput, JsErrorBox> { ... }

// Async op
#[op2(async)]
async fn op_my_async(state: Rc<RefCell<OpState>>, #[string] key: String) -> Result<Value, JsErrorBox> { ... }
```

Ops access per-run state via `RunState` and per-slot state via their pack's `Store` type, both stored in `OpState`.

---

## JS Shim Pattern

Each pack has a matching JS shim in `sdk-ts/src/<name>.js` that:
1. Accesses ops via `const ops = globalThis.__sandbox_ops;`
2. Wraps ops in a ergonomic JS API
3. Exports named symbols (no default exports)
4. Is 7-bit ASCII only (deno_core requirement)

```js
// sdk-ts/src/kv.js
const ops = globalThis.__sandbox_ops;

export const kv = Object.freeze({
  async get(key) { return await ops.op_kv_get(key); },
  async set(key, value) { return await ops.op_kv_set(key, JSON.stringify(value)); },
  // ...
});
```

---

## Adding a New Pack

1. Create `src/sdk/<name>_sdk.rs`
2. Define a `<Name>Store` struct if the pack needs per-slot state
3. Implement `SdkExtension` for `<Name>Pack`
4. Create `sdk-ts/src/<name>.js` — JS shim
5. Create `sdk-ts/types/<name>.d.ts` — TypeScript declarations
6. Add `pub mod <name>_sdk;` to `sdk/mod.rs`
7. Export the pack from `hello_sandbox::lib.rs`
8. If the pack adds globals (like TimerPack), use `pre_freeze_globals()`
9. If the pack needs per-slot state, implement `inject_op_state()`
10. Regenerate the snapshot: `cargo run --bin make-snapshot`
