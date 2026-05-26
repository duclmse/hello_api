//! Assertion pack — `sandbox:assert`.
//!
//! Provides a formal assertion op that records pass/fail counts in
//! [`RunState`] and surfaces them in [`RunMetrics`] after the run.
//!
//! Unlike `sandbox:test` (a pure JS library), this pack is backed by a Rust op,
//! so assertion counts flow through [`RunMetrics::assertions_passed`] and
//! [`RunMetrics::assertions_failed`] without requiring a `results()` call.
//!
//! # Usage
//!
//! ```js
//! import { assert } from "sandbox:assert";
//!
//! const res = sandbox.readInput("_response");
//! assert.equal(res.status, 200, "expect 200 OK");
//! assert.contains(res.body, "success", "body should say success");
//! assert.ok(res.ok);
//! // No return needed — assertions are tracked automatically in RunMetrics.
//! ```

use deno_core::{op2, OpDecl, OpState};

use crate::runtime::RunState;
use crate::sdk::SdkExtension;

// ─── Op ───────────────────────────────────────────────────────────────────────

/// Record the outcome of a single assertion in [`RunState`].
///
/// Called by every `assert.*` method in `sdk-ts/src/assert.js`.
/// Increments `assert_passed` on success and `assert_failed` on failure.
/// Never throws — all failures are collected silently.
#[op2(fast)]
fn op_assert(state: &mut OpState, pass: bool, #[string] _message: String) {
    let run_state = state.borrow_mut::<RunState>();
    if pass {
        run_state.assert_passed += 1;
    } else {
        run_state.assert_failed += 1;
    }
}

// ─── Pack ─────────────────────────────────────────────────────────────────────

/// Assertion SDK pack — registers `sandbox:assert` and wires `op_assert` into
/// the runtime.
///
/// Adds [`RunMetrics::assertions_passed`] and [`RunMetrics::assertions_failed`]
/// tracking to every run where the pack is active.
pub struct AssertPack;

impl SdkExtension for AssertPack {
    fn name(&self) -> &'static str {
        "assert"
    }

    fn ops(&self) -> Vec<OpDecl> {
        vec![op_assert()]
    }

    fn esm_files(&self) -> Vec<(&'static str, &'static str)> {
        vec![("sandbox:assert", include_str!("../../sdk-ts/src/assert.js"))]
    }

    fn ts_declarations(&self) -> &'static str {
        include_str!("../../sdk-ts/types/assert.d.ts")
    }
}
