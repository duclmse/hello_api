//! `TimerPack` — sandbox-safe `setTimeout` / `setInterval` / `clearTimeout` /
//! `clearInterval` globals.
//!
//! # Architecture
//!
//! Timers are backed by `tokio::time::sleep`.  Cancellation uses a
//! per-timer `tokio::sync::oneshot` channel stored in `RunState.timers`:
//!
//! ```text
//! JS: setTimeout(cb, 100)
//!   → op_timer_set(id=1, delay_ms=100)
//!       stores (cancel_tx) in RunState.timers[1]
//!       tokio::select! {
//!           _ = sleep(100ms) => return true  (fired)
//!           _ = cancel_rx   => return false  (cancelled)
//!       }
//!
//! JS: clearTimeout(1)
//!   → op_timer_clear(id=1)
//!       removes cancel_tx from RunState.timers[1]  ← drops it
//!       cancel_rx resolves → op_timer_set returns false
//!
//! End of run:
//!   RunState dropped by try_take::<RunState>() in SharedRuntime::run()
//!   → all timers in RunState.timers are dropped
//!   → all pending op_timer_set futures return false immediately
//! ```
//!
//! `setInterval` is implemented entirely in JS (`timer_globals.js`) by
//! re-arming `op_timer_set` after each successful tick.  Re-arming is bounded
//! by `max_interval_calls` (read from `RunState` via `op_timer_max_interval_calls`)
//! to prevent infinite event-loop spinning.

use std::cell::RefCell;
use std::rc::Rc;

use deno_core::{op2, OpDecl, OpState};

use crate::runtime::RunState;
use crate::sdk::SdkExtension;

// ─── ops ─────────────────────────────────────────────────────────────────────

/// Arm a timer.  Returns `true` if the delay elapsed (timer fired); `false`
/// if the timer was cancelled (via `op_timer_clear`) or the run ended.
///
/// The cancel channel is stored in `RunState.timers[timer_id]`.  Dropping
/// `RunState` (at end of every run) cancels all outstanding timers.
#[op2(async(deferred), fast)]
pub async fn op_timer_set(
    state: Rc<RefCell<OpState>>,
    #[smi] timer_id: u32,
    #[smi] delay_ms: u32,
) -> Result<bool, deno_error::JsErrorBox> {
    use tokio::sync::oneshot;

    let (cancel_tx, cancel_rx) = oneshot::channel::<()>();

    {
        let mut op = state.borrow_mut();
        if !op.has::<RunState>() {
            // Run already ended — bail out without sleeping.
            return Ok(false);
        }
        op.borrow_mut::<RunState>().timers.insert(timer_id, cancel_tx);
    }

    let delay = std::time::Duration::from_millis(u64::from(delay_ms));

    // Poll both the sleep and the cancel receiver concurrently.
    // `cancel_rx` resolves when `cancel_tx` is dropped (clearTimeout) or
    // when the entire `RunState.timers` map is dropped (end of run).
    tokio::select! {
        _ = tokio::time::sleep(delay) => Ok(true),
        _ = cancel_rx => Ok(false),
    }
}

/// Cancel a timer by dropping its cancel sender.
///
/// Dropping the sender resolves the `cancel_rx` inside `op_timer_set`,
/// which causes that future to return `false` on its next poll.
#[op2(fast)]
pub fn op_timer_clear(state: &mut OpState, #[smi] timer_id: u32) {
    if state.has::<RunState>() {
        state.borrow_mut::<RunState>().timers.remove(&timer_id);
        // Dropped cancel_tx wakes the pending op_timer_set → returns false.
    }
}

/// Returns the configured `max_interval_calls` limit for this run.
///
/// Called once per `setInterval` invocation from `timer_globals.js` to
/// bound the number of times an interval timer may re-arm itself.
#[op2(fast)]
pub fn op_timer_max_interval_calls(state: &mut OpState) -> u32 {
    if state.has::<RunState>() {
        state.borrow::<RunState>().max_interval_calls as u32
    } else {
        0
    }
}

// ─── TimerPack ────────────────────────────────────────────────────────────────

/// SDK pack that installs `setTimeout`, `clearTimeout`, `setInterval`, and
/// `clearInterval` as sandboxed globals.
///
/// # Registering
///
/// ```rust,ignore
/// let mut sb = Sandbox::builder()
///     .config(SandboxConfig::trusted())
///     .sdk(TimerPack)
///     .build()
///     .unwrap();
/// ```
///
/// # Limits
///
/// `setInterval` callbacks are bounded by [`SandboxConfig::max_interval_calls`]
/// (default: 1000). After the limit is reached the interval is silently stopped.
///
/// All pending timers are automatically cancelled when a run completes — the
/// same `RunState` drop mechanism that closes the streaming event channel.
///
/// # Notes
///
/// - Extra callback arguments (`setTimeout(fn, delay, arg1, ...)`) are not
///   forwarded to the callback. Use a closure: `setTimeout(() => fn(arg), delay)`.
/// - Timer globals are installed via the `// PRE_FREEZE_INJECTION` mechanism in
///   `core.js` so they are available before `globalThis` is frozen.
pub struct TimerPack;

impl SdkExtension for TimerPack {
    fn name(&self) -> &'static str {
        "timer"
    }

    fn ops(&self) -> Vec<OpDecl> {
        vec![
            op_timer_set(),
            op_timer_clear(),
            op_timer_max_interval_calls(),
        ]
    }

    /// No `sandbox:` ESM module — timers are globals, not an import.
    fn esm_files(&self) -> Vec<(&'static str, &'static str)> {
        vec![]
    }

    fn ts_declarations(&self) -> &'static str {
        include_str!("../../sdk-ts/types/timer.d.ts")
    }

    /// Injects `setTimeout`, `clearTimeout`, `setInterval`, `clearInterval`
    /// into `core.js` before `Object.freeze(globalThis)`.
    fn pre_freeze_globals(&self) -> Option<&'static str> {
        Some(include_str!("../../sdk-ts/src/timer_globals.js"))
    }
}
