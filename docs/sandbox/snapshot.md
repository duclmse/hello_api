# V8 Snapshot — Fast Cold Starts

`src/snapshot.rs` supports pre-baked V8 bytecode snapshots to eliminate JS
parse/compile overhead on cold starts.

## What Is a Snapshot?

A V8 snapshot is a serialized heap image that captures the state of a V8 isolate
after running the bootstrap code (CorePack, SDK shims, prototype freezing). When
a new isolate starts from a snapshot, it skips re-parsing and re-executing all
of that code — the heap is restored instantly.

Without snapshot: ~50ms cold start (V8 init + parse + compile CorePack + all SDK
shims) With snapshot: ~5ms cold start (V8 init + heap restore)

---

## Using the Snapshot

The snapshot is automatically used if `snapshot/snapshot.bin` and
`snapshot/version.txt` are present and up-to-date. `build.rs` validates the
version string on every build:

```
{CARGO_PKG_VERSION}/deno_core-0.380.1
```

If the version doesn't match (e.g., after a dependency upgrade), `build.rs`
emits a warning and sets `cfg(has_snapshot = false)`. The runtime falls back to
normal initialization.

### Regenerating the Snapshot

After changing any SDK shim (`.js` files in `sdk-ts/src/`) or bumping
`deno_core`:

```bash
cargo run --bin make-snapshot
cargo build
```

`make-snapshot` writes `snapshot/snapshot.bin` and `snapshot/version.txt`.
Commit both files to git.

---

## `get_snapshot()`

```rust
pub fn get_snapshot() -> Option<&'static [u8]>
```

Returns the pre-baked snapshot bytes if available (`cfg(has_snapshot)`), or
`None`. The bytes are embedded at compile time via `include_bytes!`.

---

## `builtin_ops()` and `builtin_pre_freeze_globals()`

Used by `make-snapshot` to register exactly the same ops and globals that will
be present in production:

```rust
pub fn builtin_ops() -> Vec<OpDecl>
pub fn builtin_pre_freeze_globals() -> Vec<&'static str>
```

The snapshot binary registers all built-in pack ops and runs all pre-freeze
injection snippets (e.g., timer globals) before serializing.

---

## `make-snapshot` Binary

`src/bin/make_snapshot.rs`:

1. Creates a `JsRuntimeForSnapshot` with all built-in ops
2. Registers `core.js` as the ESM entry point
3. Runs pre-freeze global injection snippets
4. Calls `.snapshot()` to serialize the heap
5. Writes `snapshot/snapshot.bin` and `snapshot/version.txt`

---

## Runtime Integration

In `src/runtime.rs`, `SharedRuntime::new()` checks `get_snapshot()`:

**With snapshot:**

- Sets `RuntimeOptions::startup_snapshot = Some(bytes)`
- Skips registering ESM extension files that are already in the snapshot
- No ESM entry point (already executed in snapshot)

**Without snapshot:**

- Registers all ESM extension files normally
- Sets `CorePack` as ESM entry point
- Full parse + compile on first run

---

## Snapshot Compatibility

SDK packs can declare whether they are snapshot-compatible:

```rust
impl SdkExtension for MyPack {
    fn snapshot_compatible(&self) -> bool {
        true  // default
    }
}
```

A pack that is not snapshot-compatible will register its ESM files even when a
snapshot is active (overriding the snapshotted version). This is useful for
packs with dynamic configuration that can't be baked in.

---

## Source

`src/snapshot.rs` — 104 lines `src/bin/make_snapshot.rs` — snapshot generation
binary `build.rs` — snapshot version validation `snapshot/snapshot.bin` —
pre-baked bytecode (~698 KiB) `snapshot/version.txt` — version string for
invalidation
