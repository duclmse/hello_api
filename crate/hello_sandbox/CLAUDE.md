# CLAUDE.md — Working Rules for deno-sandbox

This file governs how you work on this codebase. Read it before every task.

---

## Orientation

You are working on `deno-sandbox`, a Rust library crate. The authoritative
specification is in `AGENT.md`. When in doubt, `AGENT.md` wins. This file tells
you _how_ to work; `AGENT.md` tells you _what_ to build.

---

## Repository Map

```
src/
  lib.rs          public re-exports only — no logic here
  config.rs       IsolationLevel enum + SandboxConfig struct + 3 constructors
  error.rs        SandboxError (thiserror)
  event.rs        SandboxEvent struct
  loader.rs       AllowlistModuleLoader + AllowlistModuleLoaderBuilder
  transpile.rs    looks_like_typescript() + transpile()
  runtime.rs      RunState + SharedRuntime  ← core V8 logic lives here
  pool.rs         PoolConfig + SlotState + RuntimePool
  sandbox.rs      SandboxResult + SandboxBuilder + Sandbox  ← public entry point
  sdk/
    mod.rs        SdkExtension trait + SdkRegistry
    core_sdk.rs   CorePack  (always included)
    kv_sdk.rs     KvPack    (opt-in)
    crypto_sdk.rs CryptoPack (opt-in)
    http_sdk.rs   HttpPack  (opt-in)

sdk-ts/
  src/
    core.js       bootstrap shim — sets console, sandbox, freezes prototypes
    kv.js         KV Promise shim — exports { kv }
    crypto.js     crypto shim — exports { crypto }
    http.js       fetch shim — exports { fetch }, class SandboxResponse
  types/
    core.d.ts     declare const sandbox, console
    kv.d.ts       export declare const kv
    crypto.d.ts   export declare const crypto
    http.d.ts     export declare function fetch, class SandboxResponse
```

---

## Development Rules

### Before writing any code

1. Re-read the relevant module's doc comment at the top of the file.
2. Check `AGENT.md` §§ that apply to the module (e.g. §9 for sdk/mod.rs).
3. Understand which security invariants (AGENT.md §15) the module must uphold.

### Rust conventions

- All public types and functions must have doc comments.
- Use `thiserror` for all error types. Never use `unwrap()` in library code —
  only in examples and tests.
- Async functions that touch `JsRuntime` must only be called from a `LocalSet`.
  Document this constraint on the function if it is not immediately obvious.
- `#[op2(fast)]` for ops that take only primitives (no serde). `#[op2]` +
  `#[serde]` for ops that take/return structs. `#[op2(async)]` for async ops.
- Ops that need per-run state: `#[state] state: &mut RunState`. Ops that need
  per-slot state: `#[state] store: &mut MyPackStore`. Never reach outside
  OpState for mutable shared state.

### JavaScript shim conventions (`sdk-ts/src/*.js`)

- Every shim file must begin with `const { ops } = Deno.core;`.
- Shims must export named symbols only — no default exports.
- No shim may re-export or import from another shim. They are independent.
- `core.js` is the only shim that mutates `globalThis`. All others are pure
  modules.
- Objects exported from shims must be `Object.freeze()`d.

### TypeScript declaration conventions (`sdk-ts/types/*.d.ts`)

- `core.d.ts` uses `declare const` (ambient globals, no import needed).
- All other `.d.ts` files use `export declare` (requires explicit import).
- No implementation code in `.d.ts` files.
- Mirror the exact shape of the JS shim's exports.

### Adding a new SDK pack — checklist

- [ ] Create `src/sdk/<name>_sdk.rs`
- [ ] Define a `<Name>Store` struct if the pack has per-slot state
- [ ] Implement `SdkExtension` for `<Name>Pack`
- [ ] `fn name()` must return a unique lowercase string — it becomes the
      `sandbox:<n>` specifier
- [ ] Create `sdk-ts/src/<name>.js` — JS shim, exports named API, calls
      `ops.op_*`
- [ ] Create `sdk-ts/types/<name>.d.ts` — TS declarations mirroring the shim
- [ ] Add `pub mod <name>_sdk;` in `sdk/mod.rs`
- [ ] Document the pack in `AGENT.md §10` and register it in the demo

### Modifying the pool

- Slot transitions must always end in exactly one of: `Idle`, `CheckedOut`,
  `Stale`. A slot must never be left in `CheckedOut` after a `run_in_slot` call
  returns.
- A failed run must always mark the slot `Stale` — never return a potentially
  tainted `SharedRuntime` to the pool.
- The `Mutex` guard must not be held across an `await` point. Extract the
  runtime, drop the guard, run async work, re-acquire guard to check in.

### Modifying the loader

- After any change to `AllowlistModuleLoader::resolve()`, verify that all three
  deny cases still work: `ext:`, `node:`, non-`sandbox:` absolute URLs.
- `.d.ts` files are registered as regular source entries in the loader. The
  runtime never executes them (they are never loaded as modules); they exist
  only so editor tooling can resolve `sandbox:kv.d.ts`.

---

## Security Checklist

Run through this mentally before every PR that touches `runtime.rs`,
`loader.rs`, `sdk/mod.rs`, or any `sdk-ts/src/*.js` file:

- [ ] `ext:` specifiers are blocked in `loader.rs::resolve()` before any other
      logic
- [ ] `core.js` captures `ops` in a block scope before deleting `Deno`
- [ ] `core.js` exports nothing —
      `import ... from "ext:sandbox_ext/bootstrap.js"` yields empty namespace
- [ ] `Object.freeze(globalThis)` is called at the end of `core.js`
- [ ] All built-in prototypes are frozen in `core.js` before `globalThis` is
      frozen
- [ ] `op_http_fetch` checks the allowlist before any `await` or network call
- [ ] Script errors always discard the slot (`SlotState::Stale`)
- [ ] Watchdog thread is spawned for `PowerUser` and `Untrusted` isolation
      levels
- [ ] Raw isolate pointer is only dereferenced inside the watchdog thread while
      `SharedRuntime::run()` is still on the stack

---

## Known Incomplete Areas (stubs to finish)

| Location                                   | What to complete                                                         |
| ------------------------------------------ | ------------------------------------------------------------------------ |
| `crypto_sdk.rs` `op_crypto_hash`           | Replace format mock with `ring::digest` or `sha2`                        |
| `crypto_sdk.rs` `op_crypto_random_bytes`   | Replace with `rand::rngs::OsRng`                                         |
| `crypto_sdk.rs` `op_crypto_uuid`           | Replace with `uuid::Uuid::new_v4()`                                      |
| `http_sdk.rs` `op_http_fetch`              | Replace stub with `reqwest` async client                                 |
| `runtime.rs` `SharedRuntime::new`          | Wire `v8::CreateParams::heap_limits(initial, max)` into `RuntimeOptions` |
| `sandbox.rs` `run_in_child_process`        | Install `seccompiler` filter before spawning worker                      |
| `loader.rs` `AllowlistModuleLoaderBuilder` | Add `#[derive(Clone)]` or manual `Clone` impl                            |

---

## Invariants That Must Never Be Broken

1. `CorePack` is always the first element of `SdkRegistry::packs`.
   `SandboxBuilder::build()` enforces this — do not remove it.
2. No shim file may call `Deno.core.ops` after `core.js` has run (it deletes
   `Deno`). Shims call ops only at shim load time via the captured `ops`
   reference. Wait — shims run after `core.js` deletes Deno. Therefore: shims
   that need ops must capture `Deno.core` **before** `core.js` runs. Current
   design: `core.js` runs as the `esm_entry_point`, other shims are loaded
   lazily on first `import`. Since `core.js` freezes `globalThis` and deletes
   `Deno`, other shims must capture ops via the extension's own `Deno.core`
   reference which is available at module evaluation time before freeze. Verify
   this is working correctly when adding new packs.
3. `AllowlistModuleLoaderBuilder` must be `Clone` so `RuntimePool` can build a
   fresh `AllowlistModuleLoader` for each new slot. If you add fields to the
   builder, they must be `Clone` too.
4. `RunState` is replaced entirely for each run. Ops that reference it via
   `#[state]` always see the current run's data. Never cache a `RunState`
   reference across an await.
5. The `__RETURN__:` sentinel prefix is the only mechanism for returning a value
   from a script. Do not change it without updating both `runtime.rs`
   (extraction) and the script wrapper (injection).

---

## Asking Clarifying Questions

Before implementing anything that is not explicitly specified in `AGENT.md`,
ask. Things that are commonly ambiguous:

- Scope of KV persistence (currently: per slot, cleared on recycle — not per
  user session)
- Whether a new pack should have per-run or per-slot state
- Whether `Untrusted` scripts on non-Linux should error or fall back to
  `PowerUser` (current policy: warn + fall back)
- Whether the child-process path should be synchronous or async (currently async
  via `tokio::process::Command`)
