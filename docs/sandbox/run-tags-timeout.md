# Phase 18 — Run Tags + Per-Run Timeout Override

## Overview

Phase 18 adds two new fields to `RunCapabilities`:

| Field              | Type                      | Purpose                                            |
| ------------------ | ------------------------- | -------------------------------------------------- |
| `timeout_override` | `Option<Duration>`        | Override the pool-level script timeout for one run |
| `tags`             | `HashMap<String, String>` | Attach arbitrary metadata to a run                 |

Tags travel in two directions: **into the script** (readable as
`sandbox.tags()`) and **into metrics** (forwarded verbatim to
`RunMetrics::tags`). The timeout override lets hosts widen or narrow the per-run
execution budget independently of the pool configuration.

---

## Problem

Before Phase 18, all runs inside one `Sandbox` shared the same timeout and had
no way to attach per-run metadata. Two common needs were unmet:

1. **Variable time budgets.** A background job may need 30 seconds; an
   interactive webhook needs 2 seconds. Maintaining separate `Sandbox` instances
   for each class is expensive.

2. **Routing and attribution.** A `MetricsSink` has no way to know which tenant,
   request, or feature flag a given run belongs to without an out-of-band
   correlation mechanism.

---

## Solution

### `RunCapabilities` additions (`config.rs`)

```rust
pub struct RunCapabilities {
    // … existing Phase 17 fields …

    /// Override the sandbox-level timeout for this run.
    /// None = use SandboxConfig::timeout.
    pub timeout_override: Option<Duration>,

    /// Arbitrary key-value metadata for this run.
    /// Readable inside the script via sandbox.tags().
    /// Forwarded verbatim to RunMetrics::tags.
    pub tags: HashMap<String, String>,
}
```

`RunCapabilities::default()` has `timeout_override: None` and
`tags: HashMap::new()` — a complete no-op; all existing behaviour is preserved.

### `RunMetrics` addition (`config.rs`)

```rust
pub struct RunMetrics {
    // … existing fields …
    pub tags: HashMap<String, String>,
}
```

Tags are copied from `RunCapabilities::tags` into `RunMetrics::tags` after the
run completes. `MetricsSink::record()` receives them and can use them to
annotate, route, or filter metrics.

### `sandbox.tags()` JavaScript API

```typescript
// In every script — no import needed.
const t = sandbox.tags(); // Readonly<Record<string, string>>
console.log(t["tenant"]); // e.g. "acme"
console.log(t["request_id"]); // e.g. "req-abc"
```

Returns a **frozen** `Record<string, string>`. Scripts cannot mutate tags.
Returns `{}` when no tags were set.

---

## Usage Examples

### Per-run timeout

```rust
use hello_sandbox::{Sandbox, RunCapabilities};
use std::time::Duration;

// Pool has a 10-second default.
let result = sandbox.run_with_caps(
    "/* background job */",
    RunCapabilities {
        timeout_override: Some(Duration::from_secs(60)),  // wider budget
        ..Default::default()
    },
).await?;

let result = sandbox.run_with_caps(
    "/* interactive hook */",
    RunCapabilities {
        timeout_override: Some(Duration::from_millis(500)),  // tighter budget
        ..Default::default()
    },
).await?;
```

### Tenant isolation with tags

```rust
use hello_sandbox::RunCapabilities;
use std::collections::HashMap;

let mut tags = HashMap::new();
tags.insert("tenant".into(), tenant_id.clone());
tags.insert("request_id".into(), request_id.clone());

let result = sandbox.run_with_caps(
    r#"
    const { tenant, request_id } = sandbox.tags();
    sandbox.emit("audit", { tenant, request_id, action: "script_run" });
    return process(sandbox.readInput("payload"));
    "#,
    RunCapabilities { tags, ..Default::default() },
).await?;

// Tags arrive in metrics for the MetricsSink too.
println!("tenant: {}", result.metrics.tags["tenant"]);
```

---

## Implementation Details

### Data flow

```
RunCapabilities { timeout_override, tags, … }
  │
  ▼  (extracted before caps consumed into RunState)
effective_timeout = caps.timeout_override.unwrap_or(config.timeout)
run_tags          = caps.tags.clone()
  │
  ├─► RunState { tags: run_tags.clone(), capabilities: caps, … }  ← in OpState
  │       └─► op_read_tags() reads RunState::tags
  │
  ├─► watchdog thread sleeps for effective_timeout
  │       └─► SandboxError::Timeout(effective_timeout) if it fires
  │
  └─► RunMetrics { tags: run_tags, … }  ← returned to caller + MetricsSink
```

**Critical ordering**: `effective_timeout` and `run_tags` are extracted from
`capabilities` _before_ `capabilities` is moved into `RunState`. This is
necessary because:

- The watchdog thread is spawned after `RunState` is injected and needs
  `effective_timeout` as a captured local.
- `RunMetrics` is assembled after the event loop when `RunState` has already
  been taken out of `OpState`.

### `op_read_tags` (`core_sdk.rs`)

```rust
#[op2]
#[serde]
fn op_read_tags(state: &mut OpState) -> HashMap<String, String> {
    state.borrow::<RunState>().tags.clone()
}
```

Reads from `RunState::tags` (a `HashMap<String, String>` cloned from
`RunCapabilities::tags` at run start). Deno's `#[serde]` serializes it as a
plain JS object.

### `sandbox.tags()` shim (`core.js`)

```javascript
globalThis.sandbox = Object.freeze({
  readInput: (key) => ops.op_read_input(key),
  emit: (name, payload) => ops.op_emit(name, JSON.stringify(payload ?? null)),
  tags: () => Object.freeze(ops.op_read_tags()),
});
```

`Object.freeze()` is called on the result so scripts cannot mutate the returned
object. Each call to `sandbox.tags()` gets a fresh frozen snapshot.

### Timeout reporting

When `timeout_override` is set and the watchdog fires, `SandboxError::Timeout`
reports the _effective_ timeout (the override value), not the pool default. This
matches what the caller actually requested.

---

## Files Changed

| File                      | Change                                                                                                                                                         |
| ------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `src/config.rs`           | Added `HashMap` import; `timeout_override` + `tags` to `RunCapabilities`; `tags` to `RunMetrics` + `Default`                                                   |
| `src/runtime.rs`          | Added `tags` to `RunState`; extract `effective_timeout`/`run_tags` before consuming caps; watchdog uses `effective_timeout`; `RunMetrics` includes `tags`      |
| `src/sdk/core_sdk.rs`     | Added `op_read_tags`; updated `ops()` to 4 ops; updated `core_pack_has_three_ops` → `core_pack_has_four_ops`; added `op_read_tags` to source-content assertion |
| `sdk-ts/src/core.js`      | Added `tags: () => Object.freeze(ops.op_read_tags())` to `globalThis.sandbox`                                                                                  |
| `sdk-ts/types/core.d.ts`  | Added `tags(): Readonly<Record<string, string>>` to sandbox declaration                                                                                        |
| `snapshot/snapshot.bin`   | Regenerated (new op added to CorePack)                                                                                                                         |
| `tests/run_tags_tests.rs` | 11 new integration tests                                                                                                                                       |

---

## Test Coverage (`tests/run_tags_tests.rs`)

| Test                                             | What it verifies                                            |
| ------------------------------------------------ | ----------------------------------------------------------- |
| `tags_empty_by_default`                          | `sandbox.tags()` returns `{}` when no tags provided         |
| `tags_visible_inside_script`                     | Multiple tags all readable inside the script                |
| `tags_individual_lookup_inside_script`           | Single tag readable by key                                  |
| `tags_object_is_frozen`                          | Assigning to `sandbox.tags()` result throws in strict mode  |
| `tags_forwarded_to_run_metrics`                  | `RunMetrics::tags` contains the host-provided tags          |
| `tags_empty_in_metrics_when_none_set`            | `RunMetrics::tags` is empty when no tags provided           |
| `tags_do_not_bleed_between_runs`                 | Tags from run N are not visible in run N+1                  |
| `multiple_tags_all_present`                      | All keys from a multi-entry map are present                 |
| `timeout_override_shorter_kills_script`          | Short override terminates an infinite loop                  |
| `timeout_override_longer_allows_slow_script`     | Long override prevents premature termination                |
| `default_capabilities_noop_for_tags_and_timeout` | `RunCapabilities::default()` is a no-op for both new fields |
