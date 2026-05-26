use std::collections::HashMap;

use deno_core::{op2, OpDecl, OpState};
use deno_error::JsErrorBox;

use crate::event::SandboxEvent;
use crate::runtime::RunState;
use crate::sdk::SdkExtension;

/// Core SDK pack — always included (console, readInput, emit).
pub struct CorePack;

// ─── Ops ─────────────────────────────────────────────────────────────────────

/// Append a line to the run's log buffer (backing `console.*` in JS).
///
/// `is_err` is currently unused — error levels are prefixed by the JS shim.
/// Named `op_sandbox_print` to avoid colliding with deno_core's built-in `op_print`.
#[op2(fast)]
fn op_sandbox_print(state: &mut OpState, #[string] msg: String, _is_err: bool) {
    let run_state = state.borrow_mut::<RunState>();
    if run_state.logs.len() >= run_state.max_log_lines {
        run_state.log_quota_exceeded = true;
        return;
    }
    run_state.logs.push(msg);
}

/// Read a named input value provided by the host.
/// Returns the value as a JSON-deserialized object, or `null` if not set.
#[op2]
#[serde]
fn op_read_input(state: &mut OpState, #[string] key: String) -> Option<serde_json::Value> {
    state.borrow::<RunState>().inputs.get(&key).cloned()
}

/// Push a named event with a JSON payload string to the host channel.
///
/// Returns an error if the per-run `emit_calls_per_run` rate limit is exceeded.
/// Silently drops the event if `emit_enabled` is `Some(false)` or if the event
/// name is not in the `emit_allowed_names` allowlist.
#[op2(fast)]
fn op_emit(
    state: &mut OpState,
    #[string] name: String,
    #[string] payload_json: String,
) -> Result<(), JsErrorBox> {
    let run_state = state.borrow_mut::<RunState>();

    // Pack-level enable/disable — silently drop when disabled.
    if run_state.capabilities.emit_enabled == Some(false) {
        return Ok(());
    }

    // Per-run emit name allowlist — silently drop disallowed event names.
    if let Some(allowed) = &run_state.capabilities.emit_allowed_names {
        if !allowed.iter().any(|n| n == &name) {
            return Ok(());
        }
    }

    // Per-run rate limit: capability override takes precedence.
    let limit =
        run_state.capabilities.emit_calls_limit.or(run_state.rate_limits.emit_calls_per_run);
    run_state.emit_calls += 1;
    if let Some(lim) = limit {
        if run_state.emit_calls > lim {
            run_state.rate_limit_exceeded = Some(("emit".to_string(), lim));
            return Err(JsErrorBox::generic(format!("rate limit exceeded: emit (limit: {lim})")));
        }
    }

    let elapsed_ms = run_state.start.elapsed().as_millis() as u64;
    let payload: serde_json::Value =
        serde_json::from_str(&payload_json).unwrap_or(serde_json::Value::Null);
    let event = SandboxEvent::new(name, payload, elapsed_ms);
    let _ = run_state.events.send(event);
    Ok(())
}

/// Read the per-run tags provided by the host via `RunCapabilities::tags`.
///
/// Returns a plain JS object (`Record<string, string>`) — the JS shim wraps it
/// in `Object.freeze()` before exposing it as `sandbox.tags()`.
#[op2]
#[serde]
fn op_read_tags(state: &mut OpState) -> HashMap<String, String> {
    state.borrow::<RunState>().tags.clone()
}

// ─── SdkExtension impl ───────────────────────────────────────────────────────

impl SdkExtension for CorePack {
    fn name(&self) -> &'static str {
        "core"
    }

    fn ops(&self) -> Vec<OpDecl> {
        vec![
            op_sandbox_print(),
            op_read_input(),
            op_emit(),
            op_read_tags(),
        ]
    }

    fn esm_files(&self) -> Vec<(&'static str, &'static str)> {
        vec![("ext:sandbox_core/core.js", include_str!("../../sdk-ts/src/core.js"))]
    }

    fn esm_entry_point(&self) -> Option<&'static str> {
        Some("ext:sandbox_core/core.js")
    }

    fn ts_declarations(&self) -> &'static str {
        include_str!("../../sdk-ts/types/core.d.ts")
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdk::SdkExtension;

    #[test]
    fn core_pack_name() {
        assert_eq!(CorePack.name(), "core");
    }

    #[test]
    fn core_pack_has_four_ops() {
        assert_eq!(CorePack.ops().len(), 4);
    }

    #[test]
    fn core_pack_has_one_esm_file() {
        let files = CorePack.esm_files();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0, "ext:sandbox_core/core.js");
    }

    #[test]
    fn core_pack_has_entry_point() {
        assert_eq!(CorePack.esm_entry_point(), Some("ext:sandbox_core/core.js"));
    }

    #[test]
    fn core_js_source_is_non_empty() {
        let (_, src) = CorePack.esm_files()[0];
        assert!(!src.is_empty());
        assert!(src.contains("op_sandbox_print"));
        assert!(src.contains("op_read_input"));
        assert!(src.contains("op_emit"));
        assert!(src.contains("op_read_tags"));
    }

    #[test]
    fn core_js_exports_nothing() {
        let (_, src) = CorePack.esm_files()[0];
        // The shim must not export anything — scripts can't import from it.
        assert!(!src.contains("export "), "core.js must have zero exports");
    }

    #[test]
    fn core_js_freezes_global_this() {
        let (_, src) = CorePack.esm_files()[0];
        assert!(src.contains("Object.freeze(globalThis)"), "core.js must freeze globalThis");
    }

    #[test]
    fn core_js_deletes_deno() {
        let (_, src) = CorePack.esm_files()[0];
        assert!(src.contains("delete globalThis.Deno"), "core.js must delete globalThis.Deno");
    }

    #[test]
    fn core_js_exposes_sandbox_ops() {
        let (_, src) = CorePack.esm_files()[0];
        assert!(src.contains("__sandbox_ops"), "core.js must expose __sandbox_ops for SDK shims");
    }

    #[test]
    fn ts_declarations_non_empty() {
        assert!(!CorePack.ts_declarations().is_empty());
    }
}
