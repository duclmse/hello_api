//! Public entry point — [`Sandbox`] and [`SandboxBuilder`].
//!
//! # Quick start
//!
//! ```rust,ignore
//! use tokio::task::LocalSet;
//! use hello_sandbox::{Sandbox, SandboxConfig};
//! use serde_json::json;
//!
//! let local = LocalSet::new();
//! local.run_until(async {
//!     let mut sb = Sandbox::new(SandboxConfig::trusted())?;
//!     sb.set_input("x", json!(42));
//!     let result = sb.run("return sandbox.readInput('x') * 2").await?;
//!     println!("{}", result.value); // 84
//!     Ok::<_, hello_sandbox::SandboxError>(())
//! }).await?;
//! ```
//!
//! **Must be used on a `tokio::task::LocalSet`** — the underlying V8 runtime
//! is `!Send`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::config::{IsolationLevel, RunCapabilities, RunMetrics, SandboxConfig};
use crate::error::SandboxError;
use crate::event::SandboxEvent;
use crate::loader::AllowlistModuleLoaderBuilder;
use crate::pool::{PoolConfig, PoolStats, RuntimeKind, RuntimePool};
use crate::sdk::core_sdk::CorePack;
use crate::sdk::{SdkExtension, SdkRegistry};

// ─── SandboxResult ────────────────────────────────────────────────────────────

/// The result of a single sandbox script execution.
#[derive(Debug)]
pub struct SandboxResult {
    /// The value of the last expression or `return` statement in the script.
    pub value: Value,
    /// Captured `console.*` output lines (in emission order).
    pub logs: Vec<String>,
    /// Events emitted via `sandbox.emit(name, payload)` during this run.
    pub events: Vec<SandboxEvent>,
    /// Wall-clock time for this run (includes pool checkout/checkin overhead).
    pub elapsed: Duration,
    /// Which execution path was used (warm pool slot vs. isolated runtime).
    pub runtime_kind: RuntimeKind,
    /// Per-run performance metrics (heap usage, script-execution elapsed time).
    pub metrics: RunMetrics,
}

// ─── SandboxBuilder ───────────────────────────────────────────────────────────

/// Fluent builder for [`Sandbox`].
///
/// # Example
///
/// ```rust,ignore
/// let mut sb = Sandbox::builder()
///     .config(SandboxConfig::power_user())
///     .pool(PoolConfig { pool_size: 2, ..Default::default() })
///     .sdk(KvPack)
///     .module("sandbox:helpers", "export const double = x => x * 2;")
///     .input("n", json!(21))
///     .build()?;
/// ```
pub struct SandboxBuilder {
    config: SandboxConfig,
    pool_config: PoolConfig,
    /// Extra SDK packs beyond CorePack (which is always prepended).
    extra_packs: Vec<Box<dyn SdkExtension>>,
    inputs: HashMap<String, Value>,
    loader_builder: AllowlistModuleLoaderBuilder,
    /// Override path to the `sandbox-worker` binary (used by `Untrusted` isolation).
    worker_binary: Option<PathBuf>,
}

impl Default for SandboxBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl SandboxBuilder {
    /// Create a new builder with default settings (PowerUser isolation,
    /// pool_size=1).
    ///
    /// `pool_size=1` is the safe default for `tokio::task::LocalSet` use:
    /// V8 allows only one isolate active on a thread at a time. Increase
    /// via `.pool()` only if you manage isolate lifetimes carefully.
    pub fn new() -> Self {
        Self {
            config: SandboxConfig::power_user(),
            // pool_size=1: one V8 isolate per LocalSet thread.
            pool_config: PoolConfig {
                pool_size: 1,
                ..PoolConfig::default()
            },
            extra_packs: vec![],
            inputs: HashMap::new(),
            loader_builder: AllowlistModuleLoaderBuilder::default(),
            worker_binary: None,
        }
    }

    /// Set the sandbox execution config.
    pub fn config(mut self, config: SandboxConfig) -> Self {
        self.config = config;
        self
    }

    /// Set the pool config (size, run limits, idle timeout).
    pub fn pool(mut self, pool_config: PoolConfig) -> Self {
        self.pool_config = pool_config;
        self
    }

    /// Add an opt-in SDK pack (e.g. `KvPack`, `CryptoPack`, `HttpPack`).
    ///
    /// `CorePack` is always added automatically and must not be included here.
    pub fn sdk(mut self, pack: impl SdkExtension) -> Self {
        self.extra_packs.push(Box::new(pack));
        self
    }

    /// Pre-set an input value for scripts to read via `sandbox.readInput(key)`.
    pub fn input(mut self, key: impl Into<String>, value: Value) -> Self {
        self.inputs.insert(key.into(), value);
        self
    }

    /// Pre-register a user module importable from scripts under `specifier`.
    ///
    /// The specifier must start with `sandbox:` (e.g. `"sandbox:math"`).
    /// Source may be TypeScript or JavaScript.
    pub fn module(mut self, specifier: impl Into<String>, source: impl Into<String>) -> Self {
        self.loader_builder = self.loader_builder.register(specifier, source);
        self
    }

    /// Override the path to the `sandbox-worker` binary.
    ///
    /// Only relevant for `IsolationLevel::Untrusted` on Linux. By default the
    /// binary is resolved via [`crate::child::find_worker_binary`].
    pub fn worker_binary(mut self, path: impl Into<PathBuf>) -> Self {
        self.worker_binary = Some(path.into());
        self
    }

    /// Build the [`Sandbox`].
    ///
    /// Assembles the SDK registry (always prepending `CorePack`), registers
    /// TypeScript declaration files in the module loader, and returns a `Sandbox`
    /// ready for use.
    ///
    /// The underlying pool is initialised lazily on the first call to
    /// [`Sandbox::run()`], so modules and inputs can still be added after
    /// `build()` via [`Sandbox::register_module()`] / [`Sandbox::set_input()`].
    pub fn build(self) -> Result<Sandbox, SandboxError> {
        // CorePack is always first — CLAUDE.md invariant 1.
        let mut packs: Vec<Box<dyn SdkExtension>> = vec![Box::new(CorePack)];
        packs.extend(self.extra_packs);
        let sdk = SdkRegistry { packs };

        // Register .d.ts declarations so editor tooling can resolve them.
        let mut loader_builder = self.loader_builder;
        for (spec, dts) in sdk.all_declarations() {
            loader_builder = loader_builder.register(spec, dts);
        }

        Ok(Sandbox {
            config: self.config,
            pool_config: self.pool_config,
            sdk: Some(sdk),
            loader_builder,
            inputs: self.inputs,
            worker_binary: self.worker_binary,
            pool: None,
        })
    }
}

// ─── Sandbox ──────────────────────────────────────────────────────────────────

/// High-level entry point for executing scripts in an isolated V8 environment.
///
/// **Must be used on a `tokio::task::LocalSet`** — the underlying V8 runtime
/// is `!Send`.
///
/// # Lifecycle
///
/// 1. Create via [`Sandbox::new()`] or [`Sandbox::builder()`].
/// 2. Optionally configure inputs and modules via [`set_input()`] /
///    [`register_module()`].
/// 3. Call [`run()`] — repeated calls reuse the warm slot pool.
///
/// The pool is initialised lazily on the first [`run()`] call, so any setup
/// done between construction and the first run is always visible to scripts.
pub struct Sandbox {
    config: SandboxConfig,
    pool_config: PoolConfig,
    /// SDK registry moved into the pool on first run.
    sdk: Option<SdkRegistry>,
    /// User-registered modules — kept in sync with the pool's builder.
    loader_builder: AllowlistModuleLoaderBuilder,
    inputs: HashMap<String, Value>,
    /// Override path to the `sandbox-worker` binary (`Untrusted` isolation only).
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    worker_binary: Option<PathBuf>,
    /// `None` until the first call to [`run()`].
    pool: Option<RuntimePool>,
}

impl Sandbox {
    /// Create a new `Sandbox` with the given execution config and default pool.
    ///
    /// Equivalent to `Sandbox::builder().config(config).build()`.
    ///
    /// **Must be used on a `tokio::task::LocalSet`.**
    pub fn new(config: SandboxConfig) -> Result<Self, SandboxError> {
        SandboxBuilder::new().config(config).build()
    }

    /// Start a fluent [`SandboxBuilder`].
    pub fn builder() -> SandboxBuilder {
        SandboxBuilder::new()
    }

    /// Set (or replace) an input value accessible inside scripts via
    /// `sandbox.readInput(key)`.
    ///
    /// Takes effect immediately; the next [`run()`] call will see the new value.
    pub fn set_input(&mut self, key: impl Into<String>, value: Value) {
        self.inputs.insert(key.into(), value);
    }

    /// Register a module importable from scripts under `specifier`.
    ///
    /// `specifier` must start with `sandbox:` (e.g. `"sandbox:math"`).
    /// Source may be TypeScript or JavaScript.
    ///
    /// If the pool has already been initialised, any existing idle slots are
    /// marked stale so they are recycled (and the new module loader installed)
    /// on their next checkout.
    pub fn register_module(&mut self, specifier: impl Into<String>, source: impl Into<String>) {
        let spec: String = specifier.into();
        let src: String = source.into();
        self.loader_builder = self.loader_builder.clone().register(spec.clone(), src.clone());
        if let Some(pool) = &mut self.pool {
            pool.register_user_module(spec, src);
        }
    }

    /// Execute `script` (JS or TS) and return its result.
    ///
    /// For `IsolationLevel::Untrusted` on Linux, the script is executed in a
    /// fresh child process (`sandbox-worker`) with a seccomp syscall filter
    /// installed inside the child. On non-Linux platforms, a warning is logged
    /// and execution falls back to `PowerUser`-level isolation.
    ///
    /// For all other isolation levels, the pool is initialised lazily on the
    /// first call. Subsequent calls reuse warm slots where available, falling
    /// back to isolated one-shot runtimes when all slots are checked out
    /// (controlled by [`PoolConfig::fallback_to_isolated`]).
    ///
    /// **Must be called from a `tokio::task::LocalSet`.**
    pub async fn run(&mut self, script: &str) -> Result<SandboxResult, SandboxError> {
        self.run_with_caps(script, RunCapabilities::default()).await
    }

    /// Execute `script` with per-run capability constraints.
    ///
    /// Identical to [`run`] but applies `caps` on top of the sandbox-level
    /// configuration for this single execution. The capabilities narrow (never
    /// widen) what the script is allowed to do.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use hello_sandbox::RunCapabilities;
    ///
    /// let result = sandbox.run_with_caps(
    ///     r#"import { kv } from "sandbox:kv"; await kv.set("x", 1); return "ok";"#,
    ///     RunCapabilities {
    ///         kv_key_prefix: Some("user:42:".into()),
    ///         ..Default::default()
    ///     },
    /// ).await?;
    /// ```
    ///
    /// **Must be called from a `tokio::task::LocalSet`.**
    pub async fn run_with_caps(
        &mut self,
        script: &str,
        caps: RunCapabilities,
    ) -> Result<SandboxResult, SandboxError> {
        // ── Untrusted: OS-level isolation via child process ───────────────────
        #[cfg(target_os = "linux")]
        if self.config.isolation == IsolationLevel::Untrusted {
            let worker_binary =
                self.worker_binary.clone().unwrap_or_else(crate::child::find_worker_binary);
            return crate::child::run_in_child_process(
                script,
                &self.inputs,
                &self.config,
                &worker_binary,
            )
            .await;
        }

        #[cfg(not(target_os = "linux"))]
        if self.config.isolation == IsolationLevel::Untrusted {
            tracing::warn!(
                "IsolationLevel::Untrusted is only supported on Linux; \
                 falling back to PowerUser isolation"
            );
        }

        // ── Trusted / PowerUser: in-process pool ─────────────────────────────
        if self.pool.is_none() {
            let sdk = self.sdk.take().expect("Sandbox SDK was already consumed — this is a bug");
            self.pool = Some(RuntimePool::new(
                self.pool_config.clone(),
                self.config.clone(),
                self.loader_builder.clone(),
                sdk,
            ));
        }
        let inputs = self.inputs.clone();
        self.pool.as_ref().unwrap().run_with_caps(script, inputs, caps).await
    }

    /// Execute `script` with real-time event streaming.
    ///
    /// Returns `(future, receiver)` **immediately** — the script has not started
    /// yet.  Events emitted by `sandbox.emit()` are forwarded to `receiver` as
    /// they fire rather than being batch-collected at script completion.
    /// [`SandboxResult::events`] is always empty when this method is used.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let (fut, mut rx) = sandbox.run_streaming("sandbox.emit('ping', 1); return 42");
    ///
    /// tokio::task::spawn_local(async move {
    ///     while let Some(event) = rx.recv().await {
    ///         println!("live event: {} {:?}", event.name, event.payload);
    ///     }
    /// });
    ///
    /// let result = fut.await?;
    /// println!("return value: {}", result.value);
    /// ```
    ///
    /// # Notes
    ///
    /// - The returned **future** is `!Send`; it must be awaited on the same
    ///   `tokio::task::LocalSet` that owns the sandbox.
    /// - The returned **receiver** is [`Send`] and can be forwarded to other
    ///   threads or tasks.
    /// - `IsolationLevel::Untrusted` with child-process isolation uses the
    ///   in-process pool path instead (seccomp sandboxing is deferred for
    ///   streaming); a warning is logged when this occurs.
    ///
    /// **Must be called from a `tokio::task::LocalSet`.**
    pub fn run_streaming<'s>(
        &'s mut self,
        script: &'s str,
    ) -> (
        impl std::future::Future<Output = Result<SandboxResult, SandboxError>> + 's,
        UnboundedReceiver<SandboxEvent>,
    ) {
        self.run_streaming_with_caps(script, RunCapabilities::default())
    }

    /// Execute `script` with real-time event streaming and per-run capability
    /// constraints.
    ///
    /// Identical to [`run_streaming`] but applies `caps` on top of the
    /// sandbox-level configuration for this single execution.
    ///
    /// Returns `(future, receiver)` immediately — the script has not started
    /// yet. Events are forwarded to `receiver` as they fire.
    ///
    /// **Must be called from a `tokio::task::LocalSet`.**
    ///
    /// [`run_streaming`]: Sandbox::run_streaming
    pub fn run_streaming_with_caps<'s>(
        &'s mut self,
        script: &'s str,
        caps: RunCapabilities,
    ) -> (
        impl std::future::Future<Output = Result<SandboxResult, SandboxError>> + 's,
        UnboundedReceiver<SandboxEvent>,
    ) {
        // Untrusted streaming via child process is deferred. Fall through to the
        // in-process pool path with a warning on all platforms.
        if self.config.isolation == IsolationLevel::Untrusted {
            tracing::warn!(
                "run_streaming does not yet support IsolationLevel::Untrusted \
                 child-process isolation; falling back to in-process pool execution"
            );
        }

        // Initialise the pool on the first call (same lazy-init as run()).
        if self.pool.is_none() {
            let sdk = self.sdk.take().expect("Sandbox SDK was already consumed — this is a bug");
            self.pool = Some(RuntimePool::new(
                self.pool_config.clone(),
                self.config.clone(),
                self.loader_builder.clone(),
                sdk,
            ));
        }

        let inputs = self.inputs.clone();
        let pool = self.pool.as_ref().unwrap();
        pool.run_streaming_with_caps(script, inputs, caps)
    }

    /// Snapshot of the underlying pool health.
    ///
    /// Returns zeroed stats if the pool has not been initialised yet (i.e.
    /// before the first [`run()`] call).
    pub fn pool_stats(&self) -> PoolStats {
        match &self.pool {
            Some(pool) => pool.pool_stats(),
            None => PoolStats {
                idle: 0,
                checked_out: 0,
                stale: 0,
                total_runs: 0,
            },
        }
    }
}
