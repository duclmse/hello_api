# Phase 17 — Per-Run Capability Constraints

## Overview

Phase 17 adds `RunCapabilities`, a per-run constraint struct that lets the host
narrow what a single script execution may do — independently of the
sandbox-level `SandboxConfig` that governs the pool as a whole.

Before this phase, the pool config was the only security knob: if `KvPack` was
registered, every script could read and write every key. Phase 17 closes this
gap by letting the host attach fine-grained restrictions to each individual
`run()` call.

---

## Problem

A host serving scripts from multiple tenants through one `Sandbox` instance had
no way to prevent tenant A's script from reading tenant B's KV keys, or to
restrict a particular script to read-only HTTP. Each tenant required a separate
`Sandbox` instance — expensive in terms of V8 isolate creation.

---

## Solution: `RunCapabilities`

```rust
use hello_sandbox::RunCapabilities;

let result = sandbox.run_with_caps(
    r#"
    import { kv } from "sandbox:kv";
    const val = await kv.get("balance");
    sandbox.emit("balance", { val });
    return val;
    "#,
    RunCapabilities {
        kv_key_prefix:       Some("tenant:42:".into()),  // namespace isolation
        emit_allowed_names:  Some(vec!["balance".into()]), // only this event name
        http_enabled:        Some(false),                  // no HTTP for this run
        ..Default::default()
    },
).await?;
```

---

## New Public API

### `RunCapabilities` struct (`config.rs`)

All fields are `Option<_>`. `None` means "defer to the sandbox-level default".
`Some(...)` overrides or further restricts for this single run.

| Field                   | Type                  | Effect                                                    |
| ----------------------- | --------------------- | --------------------------------------------------------- |
| `kv_enabled`            | `Option<bool>`        | `Some(false)` → all KV ops throw `CapabilityDenied`       |
| `kv_key_prefix`         | `Option<String>`      | Transparently namespaces all KV keys                      |
| `kv_ops_limit`          | `Option<usize>`       | Per-run KV op count override                              |
| `http_enabled`          | `Option<bool>`        | `Some(false)` → all HTTP fetches throw `CapabilityDenied` |
| `http_allowed_prefixes` | `Option<Vec<String>>` | Replace pool-level URL allowlist for this run             |
| `http_allowed_methods`  | `Option<Vec<String>>` | Restrict HTTP verbs (e.g. `["GET"]`)                      |
| `http_calls_limit`      | `Option<usize>`       | Per-run HTTP call count override                          |
| `emit_enabled`          | `Option<bool>`        | `Some(false)` → events silently dropped                   |
| `emit_allowed_names`    | `Option<Vec<String>>` | Allowlist of event names; others silently dropped         |
| `emit_calls_limit`      | `Option<usize>`       | Per-run emit call count override                          |

### `Sandbox` methods

```rust
// Existing (backward compatible — passes RunCapabilities::default()):
sandbox.run(script).await

// New:
sandbox.run_with_caps(script, caps).await
sandbox.run_streaming_with_caps(script, caps)
```

### `RuntimePool` methods

```rust
pool.run_with_caps(source, inputs, caps).await
pool.run_streaming_with_caps(source, inputs, caps)
```

### `SandboxError::CapabilityDenied(String)` variant (`error.rs`)

Raised (as a JS exception surfaced through `SandboxError::Runtime`) when a
capability check fails.

---

## Implementation Details

### Enforcement Layer

All enforcement happens in Rust ops, **before any I/O or network call**:

#### `kv_sdk.rs` — `kv_check_capabilities(state, &mut key)`

Replaces the old `kv_check_rate_limit` helper. Called synchronously at the top
of every `op_kv_*` op:

1. Check `kv_enabled == Some(false)` → error immediately.
2. Apply per-run rate limit: `kv_ops_limit.or(rate_limits.kv_ops_per_run)`.
3. Prepend `kv_key_prefix` to `key` in-place.
4. Return the prefix string so `op_kv_list` can strip it from results.

The namespace prefix is transparent to scripts — `kv.set("x", 1)` stores
`"{prefix}x"` in the backend, but `kv.list("")` returns `["x"]` (prefix
stripped).

#### `http_sdk.rs` — synchronous guard block in `op_http_fetch`

Before any `await`:

1. Check `http_enabled == Some(false)` → error.
2. Check `http_allowed_methods` — if set and method not in list → error.
3. Apply per-run rate limit:
   `http_calls_limit.or(rate_limits.http_calls_per_run)`.
4. Determine effective allowlist: `http_allowed_prefixes.unwrap_or(pool_list)`.
5. Check URL against effective allowlist.

#### `core_sdk.rs` — `op_emit`

1. Check `emit_enabled == Some(false)` → silent `Ok(())` (no error, events
   dropped).
2. Check `emit_allowed_names` — if name not in list → silent `Ok(())`.
3. Apply per-run rate limit:
   `emit_calls_limit.or(rate_limits.emit_calls_per_run)`.
4. Only then emit the event and increment `emit_calls`.

Note: the `emit_calls` counter and `RunMetrics::emit_calls` only count events
that actually passed through the filter, not silently dropped ones.

### Data flow

```
Sandbox::run_with_caps(script, caps)
  └─► RuntimePool::run_with_caps(source, inputs, caps)
        └─► execute_run(source, inputs, event_tx, caps)
              └─► SharedRuntime::run(source, inputs, event_tx, caps)
                    └─► RunState { capabilities: caps, ... }  ← stored in OpState
                          └─► ops read capabilities from RunState at each call site
```

`RunCapabilities` is cloned into `RunState` at the start of every run and
discarded with the rest of `RunState` when the run completes. There is no
per-slot or per-pool persistence of capabilities.

### Backward Compatibility

`RunCapabilities::default()` has all fields set to `None` — a semantic no-op.
The existing `run()` and `run_streaming()` methods pass
`RunCapabilities::default()` internally, so all existing behaviour is unchanged.

---

## KV Namespace Prefix — Design Notes

The prefix is applied at the Rust op level, not in the JS shim. This means:

- Scripts never see the prefix — `kv.get("x")` works as expected.
- Scripts cannot bypass the prefix by constructing keys manually.
- `kv.list("")` returns un-prefixed keys (the prefix is stripped from results).
- Different `kv_key_prefix` values provide tenant isolation when using a shared
  `KvBackend` (e.g. a shared `InMemoryKvBackend` or a Redis backend).

---

## Rate Limit Priority

Per-run capability limits take precedence over pool-level `RateLimitConfig`:

```
effective_limit = caps.kv_ops_limit.or(config.rate_limits.kv_ops_per_run)
```

If a pool has `kv_ops_per_run: Some(100)` and a run has `kv_ops_limit: Some(5)`,
the run gets limit 5. If a pool has no limit (`None`) and a run sets
`kv_ops_limit: Some(5)`, the run gets limit 5. If both are `None`, no limit
applies.

---

## Files Changed

| File                          | Change                                                                                                                                          |
| ----------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------- |
| `src/config.rs`               | Added `RunCapabilities` struct                                                                                                                  |
| `src/error.rs`                | Added `SandboxError::CapabilityDenied` variant                                                                                                  |
| `src/runtime.rs`              | `RunState.capabilities` field; `SharedRuntime::run()` new `capabilities` param                                                                  |
| `src/pool.rs`                 | Threaded `RunCapabilities` through `execute_isolated`, `execute_run`, `run_streaming_impl`; added `run_with_caps` and `run_streaming_with_caps` |
| `src/sandbox.rs`              | Added `run_with_caps`, `run_streaming_with_caps`; `run`/`run_streaming` delegate to them                                                        |
| `src/sdk/kv_sdk.rs`           | `kv_check_rate_limit` → `kv_check_capabilities`; namespace prefix + enable/disable                                                              |
| `src/sdk/http_sdk.rs`         | Capability guard block: `http_enabled`, method filter, per-run allowlist, rate limit                                                            |
| `src/sdk/core_sdk.rs`         | `op_emit`: `emit_enabled`, name filter, per-run rate limit                                                                                      |
| `src/child.rs`                | Pass `RunCapabilities::default()` to child-process `SharedRuntime::run()`                                                                       |
| `src/lib.rs`                  | Re-export `RunCapabilities`                                                                                                                     |
| `tests/capabilities_tests.rs` | 15 new integration tests                                                                                                                        |
| `tests/runtime_tests.rs`      | Updated to pass `capabilities` to `SharedRuntime::run()`                                                                                        |
| `tests/sdk_tests.rs`          | Updated to pass `capabilities` to `SharedRuntime::run()`                                                                                        |
| `examples/core_demo.rs`       | Updated to pass `capabilities` to `SharedRuntime::run()`                                                                                        |
| `examples/sdk_demo.rs`        | Updated to pass `capabilities` to `SharedRuntime::run()`                                                                                        |
| `examples/runtime_demo.rs`    | Updated to pass `capabilities` to `SharedRuntime::run()`                                                                                        |

---

## Test Coverage (`tests/capabilities_tests.rs`)

| Test                                             | What it verifies                                        |
| ------------------------------------------------ | ------------------------------------------------------- |
| `kv_prefix_namespaces_stored_keys`               | Same key under different prefixes is isolated           |
| `kv_prefix_strips_from_list_results`             | `kv.list()` returns un-prefixed keys                    |
| `kv_disabled_blocks_all_ops`                     | `kv_enabled: Some(false)` causes KV ops to error        |
| `kv_ops_limit_overrides_pool_level`              | Per-run limit enforced when pool has no limit           |
| `http_disabled_blocks_all_fetches`               | `http_enabled: Some(false)` causes fetch to error       |
| `http_per_run_allowlist_blocks_pool_allowed_url` | Run cap replaces pool allowlist                         |
| `http_empty_allowlist_blocks_all_urls`           | `http_allowed_prefixes: Some(vec![])` blocks everything |
| `http_method_restriction_blocks_disallowed_verb` | `http_allowed_methods: Some(["GET"])` blocks POST       |
| `http_calls_limit_overrides_pool_level`          | Per-run HTTP limit enforced when pool has no limit      |
| `emit_disabled_silently_drops_all_events`        | No events, no error, counter stays zero                 |
| `emit_allowed_names_filters_events`              | Only whitelisted names forwarded                        |
| `emit_calls_limit_overrides_pool_level`          | Per-run emit limit enforced when pool has no limit      |
| `default_capabilities_are_noop`                  | `RunCapabilities::default()` leaves behaviour unchanged |
| `run_streaming_with_caps_filters_emit_names`     | Capabilities work through streaming path                |
| `combined_kv_and_emit_caps`                      | Multiple capability fields work together in one run     |
