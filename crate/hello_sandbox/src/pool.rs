//! Warm `SharedRuntime` pool.
//!
//! Each pool slot holds a reusable [`SharedRuntime`]. Slots are checked out
//! for each script execution and returned (or recycled) afterwards.
//!
//! # Threading
//!
//! [`RuntimePool`] is `!Send` because [`SharedRuntime`] is `!Send`. Use it
//! exclusively on a `tokio::task::LocalSet`.
//!
//! The internal state is guarded by a `RefCell`; borrows are always dropped
//! before any `await` point.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::config::{RunCapabilities, SandboxConfig};
use crate::event::SandboxEvent;
use crate::loader::{AllowlistModuleLoaderBuilder, CodeCache};
use crate::runtime::SharedRuntime;
use crate::sandbox::SandboxResult;
use crate::sdk::SdkRegistry;
use crate::SandboxError;

// ─── RuntimeKind ─────────────────────────────────────────────────────────────

/// Describes which execution path was used for a particular sandbox run.
///
/// Returned in [`SandboxResult::runtime_kind`] so hosts can distinguish warm
/// pool hits from isolated (freshly-created) runtimes.
#[derive(Debug, Clone)]
pub enum RuntimeKind {
    /// Ran in a warm pooled slot.  `slot` is the zero-based slot index.
    Warm { slot: usize },
    /// Ran in a freshly-created, immediately-discarded runtime (either
    /// `pool_size == 0`, all slots were checked out, or child-process isolation
    /// was used).
    Isolated,
}

// ─── PoolConfig ───────────────────────────────────────────────────────────────

/// Configuration for [`RuntimePool`].
#[derive(Clone, Debug)]
pub struct PoolConfig {
    /// Number of warm slots to maintain. `0` → all runs use isolated runtimes.
    pub pool_size: usize,
    /// Maximum number of script runs per slot before recycling.
    pub max_runs_per_slot: u64,
    /// Maximum time a slot may sit idle before it is recycled on next access.
    pub max_idle_duration: Duration,
    /// When `true` (default), fall back to an isolated one-shot runtime when
    /// all slots are checked out. When `false`, wait (yield loop) until a
    /// slot is available.
    pub fallback_to_isolated: bool,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            pool_size: 4,
            max_runs_per_slot: 100,
            max_idle_duration: Duration::from_secs(60),
            fallback_to_isolated: true,
        }
    }
}

impl PoolConfig {
    /// Optimised for maximum throughput: large pool, many runs per slot.
    pub fn high_throughput() -> Self {
        Self {
            pool_size: 8,
            max_runs_per_slot: 1_000,
            max_idle_duration: Duration::from_secs(300),
            fallback_to_isolated: true,
        }
    }

    /// Optimised for isolation: every run gets a brand-new runtime.
    ///
    /// `pool_size: 0` means no warm slots — every run creates and immediately
    /// discards a fresh `SharedRuntime`.  This is the safest preset for
    /// `tokio::task::LocalSet` use (never holds more than one V8 isolate at a time).
    pub fn high_isolation() -> Self {
        Self {
            pool_size: 0,
            max_runs_per_slot: 1,
            max_idle_duration: Duration::ZERO,
            fallback_to_isolated: true,
        }
    }
}

// ─── Slot ─────────────────────────────────────────────────────────────────────

enum Slot {
    /// Runtime is available for checkout.
    Idle {
        runtime: Box<SharedRuntime>,
        last_used: Instant,
    },
    /// Runtime is currently running a script.
    CheckedOut,
    /// Runtime encountered an error or exceeded its run limit; must be recycled.
    Stale,
}

// ─── Pool stats ───────────────────────────────────────────────────────────────

/// Snapshot of pool health.
#[derive(Debug, Clone)]
pub struct PoolStats {
    /// Number of slots currently idle (available for checkout).
    pub idle: usize,
    /// Number of slots currently executing a script.
    pub checked_out: usize,
    /// Number of slots marked stale (awaiting recycle on next checkout).
    pub stale: usize,
    /// Cumulative number of scripts executed by this pool (pooled + isolated).
    pub total_runs: u64,
}

// ─── Inner ────────────────────────────────────────────────────────────────────

struct PoolInner {
    slots: Vec<Slot>,
    total_runs: u64,
}

// ─── RuntimePool ─────────────────────────────────────────────────────────────

/// A pool of pre-warmed [`SharedRuntime`] slots.
///
/// Call [`RuntimePool::run`] to execute a script. The pool checks out a slot,
/// runs the script, and returns the slot to the pool (or marks it stale on
/// error). If no slot is available the behaviour is governed by
/// [`PoolConfig::fallback_to_isolated`].
///
/// **Must be used on a `tokio::task::LocalSet`.**
pub struct RuntimePool {
    inner: RefCell<PoolInner>,
    pool_config: PoolConfig,
    sandbox_config: SandboxConfig,
    loader_builder: AllowlistModuleLoaderBuilder,
    sdk: SdkRegistry,
}

impl RuntimePool {
    /// Create a new pool and pre-warm `pool_config.pool_size` slots.
    ///
    /// A shared [`CodeCache`] is automatically created and wired into all
    /// slots so that V8 bytecode is reused across the pool after the first
    /// compilation of each module.
    ///
    /// **Must be called on a `tokio::task::LocalSet`.**
    pub fn new(
        pool_config: PoolConfig,
        sandbox_config: SandboxConfig,
        loader_builder: AllowlistModuleLoaderBuilder,
        sdk: SdkRegistry,
    ) -> Self {
        // Attach a shared bytecode cache so all slots benefit from V8 compile
        // results after the first run of each module.
        let loader_builder = loader_builder.with_code_cache(CodeCache::new_shared());

        let mut slots = Vec::with_capacity(pool_config.pool_size);
        for _ in 0..pool_config.pool_size {
            let runtime = SharedRuntime::new(sandbox_config.clone(), loader_builder.clone(), &sdk);
            slots.push(Slot::Idle {
                runtime: Box::new(runtime),
                last_used: Instant::now(),
            });
        }
        Self {
            inner: RefCell::new(PoolInner {
                slots,
                total_runs: 0,
            }),
            pool_config,
            sandbox_config,
            loader_builder,
            sdk,
        }
    }

    // ── Checkout ─────────────────────────────────────────────────────────────

    /// Try to check out a slot. Returns `(slot_index, runtime)` or `None`.
    ///
    /// Stale slots and expired-idle slots are recycled (replaced with a fresh
    /// runtime) during this scan. Old runtimes are dropped before new ones are
    /// created to avoid nested V8 isolate construction. No borrow is held after
    /// this function returns.
    fn try_checkout(&self) -> Option<(usize, SharedRuntime)> {
        let now = Instant::now();
        let max_idle = self.pool_config.max_idle_duration;

        // Pass 1: identify indices that need recycling and collect the old
        // runtimes so we can drop them BEFORE creating new V8 isolates.
        let stale_indices: Vec<usize> = {
            let mut inner = self.inner.borrow_mut();
            let mut stale = Vec::new();
            for (idx, slot) in inner.slots.iter_mut().enumerate() {
                let needs_recycle = match slot {
                    Slot::Stale => true,
                    Slot::Idle { last_used, .. } => now.duration_since(*last_used) > max_idle,
                    Slot::CheckedOut => false,
                };
                if needs_recycle {
                    // Replace with CheckedOut temporarily so we can take the old value.
                    let mut taken = Slot::CheckedOut;
                    std::mem::swap(slot, &mut taken);
                    // The old runtime (if any) is now in `taken` and will be
                    // dropped when this block ends — before any new isolate is
                    // created.
                    drop(taken);
                    stale.push(idx);
                }
            }
            stale
        };
        // All old `SharedRuntime`s have been dropped by here.

        // Pass 2: create fresh runtimes for the recycled slots.
        // New runtimes are constructed OUTSIDE any borrow to avoid V8 isolate
        // creation while another borrow is active.
        if !stale_indices.is_empty() {
            // Build new runtimes without holding any borrow.
            let new_runtimes: Vec<(usize, SharedRuntime)> = stale_indices
                .into_iter()
                .map(|idx| {
                    let rt = SharedRuntime::new(
                        self.sandbox_config.clone(),
                        self.loader_builder.clone(),
                        &self.sdk,
                    );
                    (idx, rt)
                })
                .collect();

            // Now insert them into the slots.
            let mut inner = self.inner.borrow_mut();
            for (idx, rt) in new_runtimes {
                // Only replace if still CheckedOut (our placeholder).
                if matches!(inner.slots[idx], Slot::CheckedOut) {
                    inner.slots[idx] = Slot::Idle {
                        runtime: Box::new(rt),
                        last_used: Instant::now(),
                    };
                }
            }
        }

        // Pass 3: checkout the first Idle slot.
        let mut inner = self.inner.borrow_mut();
        for (idx, slot) in inner.slots.iter_mut().enumerate() {
            if matches!(slot, Slot::Idle { .. }) {
                let mut taken = Slot::CheckedOut;
                std::mem::swap(slot, &mut taken);
                if let Slot::Idle { runtime, .. } = taken {
                    return Some((idx, *runtime));
                }
            }
        }
        None
    }

    // ── Checkin ──────────────────────────────────────────────────────────────

    /// Return a runtime to the pool as `Idle`, or mark the slot `Stale`.
    ///
    /// If the runtime has reached `max_runs_per_slot`, it is marked `Stale`
    /// regardless of success (it will be recycled on the next checkout).
    fn checkin(&self, slot_idx: usize, runtime: SharedRuntime, failed: bool) {
        let mut inner = self.inner.borrow_mut();
        inner.total_runs += 1;
        if !failed && runtime.run_count() < self.pool_config.max_runs_per_slot {
            inner.slots[slot_idx] = Slot::Idle {
                runtime: Box::new(runtime),
                last_used: Instant::now(),
            };
        } else {
            inner.slots[slot_idx] = Slot::Stale;
            // runtime is dropped here, freeing the V8 isolate.
        }
    }

    // ── Core execution (shared by run and run_streaming) ─────────────────────

    /// Execute `source` in a brand-new, immediately-discarded runtime.
    ///
    /// `event_tx` is passed directly to the runtime; the caller owns the paired
    /// receiver and is responsible for consuming it (or dropping it).
    ///
    /// Returns `(value, logs, metrics)` on success.
    async fn execute_isolated(
        &self,
        source: &str,
        inputs: HashMap<String, Value>,
        event_tx: tokio::sync::mpsc::UnboundedSender<SandboxEvent>,
        capabilities: RunCapabilities,
    ) -> Result<
        (serde_json::Value, Vec<String>, crate::config::RunMetrics, RuntimeKind),
        SandboxError,
    > {
        let mut runtime =
            SharedRuntime::new(self.sandbox_config.clone(), self.loader_builder.clone(), &self.sdk);
        let result = runtime.run(source, inputs, event_tx, capabilities).await;
        // runtime dropped here — V8 isolate freed before any new one is created.
        drop(runtime);
        {
            let mut inner = self.inner.borrow_mut();
            inner.total_runs += 1;
        }
        let (value, logs, metrics) = result?;
        Ok((value, logs, metrics, RuntimeKind::Isolated))
    }

    /// Execute `source` in a pooled or isolated runtime, passing `event_tx`
    /// directly to the runtime.
    ///
    /// The caller owns the paired `UnboundedReceiver` and receives events as
    /// they are emitted. Returns `(value, logs, metrics, kind)` on success.
    ///
    /// **Must be called from a `tokio::task::LocalSet`.**
    async fn execute_run(
        &self,
        source: &str,
        inputs: HashMap<String, Value>,
        event_tx: tokio::sync::mpsc::UnboundedSender<SandboxEvent>,
        capabilities: RunCapabilities,
    ) -> Result<
        (serde_json::Value, Vec<String>, crate::config::RunMetrics, RuntimeKind),
        SandboxError,
    > {
        // pool_size == 0: always isolated.
        if self.pool_config.pool_size == 0 {
            return self.execute_isolated(source, inputs, event_tx, capabilities).await;
        }

        // Obtain a slot.
        let (slot_idx, mut runtime) = if self.pool_config.fallback_to_isolated {
            match self.try_checkout() {
                Some(slot) => slot,
                None => return self.execute_isolated(source, inputs, event_tx, capabilities).await,
            }
        } else {
            // Cooperative yield loop until a slot is available.
            // The borrow from try_checkout() is dropped before each yield.
            loop {
                if let Some(slot) = self.try_checkout() {
                    break slot;
                }
                tokio::task::yield_now().await;
            }
        };

        // Run the script (no RefCell borrow held across this await).
        let result = runtime.run(source, inputs, event_tx, capabilities).await;

        // Check the runtime back in.
        let failed = result.is_err();
        self.checkin(slot_idx, runtime, failed);

        let (value, logs, metrics) = result?;
        Ok((value, logs, metrics, RuntimeKind::Warm { slot: slot_idx }))
    }

    // ── Streaming implementation ──────────────────────────────────────────────

    /// Async implementation called by [`run_streaming`]. Runs the script with
    /// the provided `event_tx`; events are sent live to the paired receiver
    /// already returned to the host. `SandboxResult::events` is empty.
    ///
    /// [`run_streaming`]: RuntimePool::run_streaming
    async fn run_streaming_impl(
        &self,
        source: &str,
        inputs: HashMap<String, Value>,
        event_tx: tokio::sync::mpsc::UnboundedSender<SandboxEvent>,
        capabilities: RunCapabilities,
    ) -> Result<SandboxResult, SandboxError> {
        let start = Instant::now();
        let (value, logs, metrics, runtime_kind) =
            self.execute_run(source, inputs, event_tx, capabilities).await?;
        Ok(SandboxResult {
            value,
            logs,
            // Events were sent live to the host's UnboundedReceiver.
            events: vec![],
            elapsed: start.elapsed(),
            runtime_kind,
            metrics,
        })
    }

    // ── Public API ───────────────────────────────────────────────────────────

    /// Execute `source` in a pooled (or isolated) runtime.
    ///
    /// Returns a [`SandboxResult`] containing the return value, captured logs,
    /// emitted events (batch-collected after completion), and wall-clock elapsed
    /// time.
    ///
    /// **Must be called from a `tokio::task::LocalSet`.**
    pub async fn run(
        &self,
        source: &str,
        inputs: HashMap<String, Value>,
    ) -> Result<SandboxResult, SandboxError> {
        self.run_with_caps(source, inputs, RunCapabilities::default()).await
    }

    /// Execute `source` with explicit per-run capability constraints.
    ///
    /// Identical to [`run`] but applies `caps` on top of the sandbox-level
    /// config for this single execution.
    ///
    /// [`run`]: RuntimePool::run
    pub async fn run_with_caps(
        &self,
        source: &str,
        inputs: HashMap<String, Value>,
        caps: RunCapabilities,
    ) -> Result<SandboxResult, SandboxError> {
        let start = Instant::now();
        let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
        let (value, logs, metrics, runtime_kind) =
            self.execute_run(source, inputs, event_tx, caps).await?;
        let events: Vec<SandboxEvent> = std::iter::from_fn(|| event_rx.try_recv().ok()).collect();
        Ok(SandboxResult {
            value,
            logs,
            events,
            elapsed: start.elapsed(),
            runtime_kind,
            metrics,
        })
    }

    /// Execute `source` in a pooled (or isolated) runtime with real-time event
    /// streaming.
    ///
    /// Returns `(future, receiver)` immediately — **before the script starts**.
    /// Events emitted by `sandbox.emit()` are sent to `receiver` as they fire
    /// rather than being collected into [`SandboxResult::events`] (which is
    /// always empty when this method is used).
    ///
    /// # Usage
    ///
    /// ```rust,ignore
    /// let (fut, mut rx) = pool.run_streaming(source, inputs);
    ///
    /// // Spawn a local task to consume events as they arrive.
    /// tokio::task::spawn_local(async move {
    ///     while let Some(event) = rx.recv().await {
    ///         println!("event: {} {:?}", event.name, event.payload);
    ///     }
    /// });
    ///
    /// // Await the script result.
    /// let result = fut.await?;
    /// ```
    ///
    /// The returned `receiver` is [`Send`] so it can be forwarded to other
    /// threads or tasks.
    ///
    /// **The returned future is `!Send`** — it must be awaited on the same
    /// `tokio::task::LocalSet` that owns the pool.
    ///
    /// **Must be called from a `tokio::task::LocalSet`.**
    pub fn run_streaming<'s>(
        &'s self,
        source: &'s str,
        inputs: HashMap<String, Value>,
    ) -> (
        impl std::future::Future<Output = Result<SandboxResult, SandboxError>> + 's,
        tokio::sync::mpsc::UnboundedReceiver<SandboxEvent>,
    ) {
        self.run_streaming_with_caps(source, inputs, RunCapabilities::default())
    }

    /// Execute `source` with real-time streaming and per-run capability
    /// constraints.
    ///
    /// Identical to [`run_streaming`] but applies `caps` on top of the
    /// sandbox-level config for this single execution.
    ///
    /// [`run_streaming`]: RuntimePool::run_streaming
    pub fn run_streaming_with_caps<'s>(
        &'s self,
        source: &'s str,
        inputs: HashMap<String, Value>,
        caps: RunCapabilities,
    ) -> (
        impl std::future::Future<Output = Result<SandboxResult, SandboxError>> + 's,
        tokio::sync::mpsc::UnboundedReceiver<SandboxEvent>,
    ) {
        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();
        let fut = self.run_streaming_impl(source, inputs, event_tx, caps);
        (fut, event_rx)
    }

    /// Register a user module in the pool's loader builder.
    ///
    /// Existing idle slots are marked stale so they are recycled (with the
    /// updated loader) on their next checkout. Already-running slots are
    /// unaffected; they continue with their current loader until checked back in
    /// and eventually recycled.
    pub fn register_user_module(
        &mut self,
        specifier: impl Into<String>,
        source: impl Into<String>,
    ) {
        self.loader_builder = self.loader_builder.clone().register(specifier, source);
        // Invalidate idle slots so new loader takes effect on next access.
        let mut inner = self.inner.borrow_mut();
        for slot in &mut inner.slots {
            if matches!(slot, Slot::Idle { .. }) {
                *slot = Slot::Stale;
            }
        }
    }

    /// Snapshot of current pool health.
    pub fn pool_stats(&self) -> PoolStats {
        let inner = self.inner.borrow();
        let idle = inner.slots.iter().filter(|s| matches!(s, Slot::Idle { .. })).count();
        let checked_out = inner.slots.iter().filter(|s| matches!(s, Slot::CheckedOut)).count();
        let stale = inner.slots.iter().filter(|s| matches!(s, Slot::Stale)).count();
        PoolStats {
            idle,
            checked_out,
            stale,
            total_runs: inner.total_runs,
        }
    }
}
