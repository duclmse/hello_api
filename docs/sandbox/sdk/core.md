# CorePack — `sandbox:` Core API

`src/sdk/core_sdk.rs` + `sdk-ts/src/core.js`

CorePack is always included. It provides the `sandbox` global, `console`, and essential ops for scripts to communicate with the host.

---

## JavaScript API

### `sandbox.readInput(name)`

Read a named input value set by the host:

```js
const request = sandbox.readInput("_request");
const config = sandbox.readInput("settings");
```

Returns `null` if the input is not set. Values are JSON — arbitrary objects, arrays, strings, numbers, booleans, or null.

### `sandbox.emit(name, payload)`

Emit a named event to the host:

```js
sandbox.emit("user.created", { id: 42, name: "Alice" });
sandbox.emit("progress", { step: 1, total: 10 });
```

Events are received by the host via `SandboxResult::events` (collected mode) or the streaming `UnboundedReceiver<SandboxEvent>` (streaming mode).

Subject to `RunCapabilities`:
- `emit_enabled = Some(false)` → silently dropped
- `emit_allowed_names = Some([...])` → non-matching names silently dropped
- `emit_calls_limit` → `RateLimitExceeded` when exceeded

### `sandbox.tags()`

Read the per-run tags set by the host via `RunCapabilities::tags`:

```js
const tags = sandbox.tags();
console.log(tags.phase);   // "post", "pre", etc.
console.log(tags.test_id); // "my-test"
```

Returns a `Readonly<Record<string, string>>` (frozen object). Returns `{}` if no tags are set.

### `console`

Standard console API backed by `op_sandbox_print`:

```js
console.log("message");
console.error("error message");
console.warn("warning");
```

All output is collected in `SandboxResult::logs`. `console.error` and `console.warn` are also captured (not stderr).

---

## Security Bootstrap (`core.js`)

`core.js` is the ESM entry point for the sandbox. It runs once when the runtime slot is initialized:

1. Captures `const { ops } = Deno.core` before any freeze
2. Installs `globalThis.console` backed by `op_sandbox_print`
3. Installs `globalThis.sandbox` with `readInput`, `emit`, `tags`
4. Sets `globalThis.__sandbox_ops = ops` so shims can access ops
5. Injects `PRE_FREEZE_INJECTION` snippets from other packs (e.g., timer globals)
6. Freezes all built-in prototypes: `Object.prototype`, `Array.prototype`, `Function.prototype`, etc.
7. Calls `delete globalThis.Deno` — hides V8 internals
8. Calls `Object.freeze(globalThis)` — blocks new globals

After `core.js` runs, scripts cannot:
- Add new globals
- Modify built-in prototypes
- Access `Deno` internals
- Import from `ext:` or `node:` specifiers

---

## Ops

| Op | Description |
|----|-------------|
| `op_sandbox_print(msg, is_err)` | Append to `RunState.logs` |
| `op_read_input(name)` | Return input value from `RunState.inputs` |
| `op_emit(name, payload_json)` | Push event to `RunState.event_tx` with rate limit check |
| `op_read_tags()` | Return `RunState.tags` as a frozen object |

---

## TypeScript Declarations

`sdk-ts/types/core.d.ts`:

```typescript
declare const sandbox: {
    readInput(name: string): unknown;
    emit(name: string, payload?: unknown): void;
    tags(): Readonly<Record<string, string>>;
};

declare const console: {
    log(...args: unknown[]): void;
    error(...args: unknown[]): void;
    warn(...args: unknown[]): void;
};
```

These are ambient declarations (no import required).

---

## Source

`src/sdk/core_sdk.rs` — 4 ops
`sdk-ts/src/core.js` — bootstrap shim (7-bit ASCII)
`sdk-ts/types/core.d.ts` — TypeScript declarations
