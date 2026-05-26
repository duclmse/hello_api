use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

/// How much trust we extend to scripts — controls which isolation
/// mechanisms are layered on top of the base V8 sandbox.
///
/// ```text
/// Trusted   ─ same process, relaxed heap/timeout
/// PowerUser ─ same process, tighter limits, watchdog thread
/// Untrusted ─ child process + OS-level sandbox (seccomp/landlock on Linux)
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum IsolationLevel {
    /// Internal tooling, trusted authors.
    /// Process: same.  Watchdog: no.  OS sandbox: no.
    Trusted,

    /// Power-users / plugin authors.
    /// Process: same.  Watchdog: yes.  OS sandbox: no.
    #[default]
    PowerUser,

    /// Fully public / adversarial input.
    /// Process: child.  Watchdog: yes.  OS sandbox: seccomp+landlock (Linux).
    Untrusted,
}

// ─── RateLimitConfig ─────────────────────────────────────────────────────────

/// Per-run operation quotas.
///
/// Each field is `Option<usize>`: `None` means no limit; `Some(n)` means at
/// most `n` calls of that type per script execution.  Exceeding a limit causes
/// the script to receive a JS exception and the run to return
/// [`SandboxError::RateLimitExceeded`].
///
/// Set on [`SandboxConfig::rate_limits`] and applies to every run.
///
/// # Example
///
/// ```rust,ignore
/// let config = SandboxConfig {
///     rate_limits: RateLimitConfig {
///         http_calls_per_run: Some(5),
///         kv_ops_per_run: Some(50),
///         emit_calls_per_run: Some(20),
///     },
///     ..SandboxConfig::power_user()
/// };
/// ```
#[derive(Clone, Debug, Default)]
pub struct RateLimitConfig {
    /// Maximum number of outbound HTTP `fetch` calls per run.
    pub http_calls_per_run: Option<usize>,
    /// Maximum number of KV operations (`get`, `set`, `delete`, `list` each
    /// count as one) per run.
    pub kv_ops_per_run: Option<usize>,
    /// Maximum number of `sandbox.emit()` calls per run.
    pub emit_calls_per_run: Option<usize>,
}

// ─── PmTestResult ─────────────────────────────────────────────────────────────

/// Result of a single `pm.test()` call recorded by [`PmPack`].
///
/// Collected per-run in [`RunMetrics::pm_tests`].
///
/// [`PmPack`]: crate::sdk::pm_sdk::PmPack
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PmTestResult {
    /// The test name passed to `pm.test(name, fn)`.
    pub name: String,
    /// `true` if the test function returned without throwing.
    pub passed: bool,
}

// ─── RunMetrics ───────────────────────────────────────────────────────────────

/// Metrics recorded for a single sandbox run.
///
/// This struct is `#[non_exhaustive]` — new fields may be added in future
/// minor versions without breaking existing code that uses `..` in patterns.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct RunMetrics {
    /// Peak V8 heap usage (bytes) measured immediately after the event loop
    /// drains (`v8::HeapStatistics::used_heap_size`).
    pub peak_heap_bytes: usize,
    /// Wall-clock duration of the script execution (from `SharedRuntime::run`
    /// entry to return — does not include pool checkout/checkin overhead).
    pub elapsed: Duration,
    /// Number of `fetch` calls made during this run.
    pub http_calls: usize,
    /// Number of KV operations (`get`, `set`, `delete`, `list`) made during this run.
    pub kv_ops: usize,
    /// Number of `sandbox.emit()` calls made during this run.
    pub emit_calls: usize,
    /// Host-provided tags attached to this run via [`RunCapabilities::tags`].
    ///
    /// Forwarded verbatim from `RunCapabilities` — useful for correlating
    /// metrics with tenant, request ID, feature flag, or other routing metadata.
    pub tags: HashMap<String, String>,
    /// Number of `assert.*` calls from [`AssertPack`] that evaluated to `true`
    /// during this run. `0` when `AssertPack` is not registered.
    pub assertions_passed: usize,
    /// Number of `assert.*` calls from [`AssertPack`] that evaluated to `false`
    /// during this run. `0` when `AssertPack` is not registered.
    pub assertions_failed: usize,
    /// Results of `pm.test()` calls from [`PmPack`] (`sandbox:pm`).
    ///
    /// Each entry records the test name and whether it passed.
    /// Empty when `PmPack` is not registered or no `pm.test()` calls were made.
    pub pm_tests: Vec<PmTestResult>,
}

impl Default for RunMetrics {
    fn default() -> Self {
        Self {
            peak_heap_bytes: 0,
            elapsed: Duration::ZERO,
            http_calls: 0,
            kv_ops: 0,
            emit_calls: 0,
            tags: HashMap::new(),
            assertions_passed: 0,
            assertions_failed: 0,
            pm_tests: Vec::new(),
        }
    }
}

// ─── MetricsSink ──────────────────────────────────────────────────────────────

/// Observer called synchronously after each run with the run's metrics.
///
/// Implement this to forward metrics to a monitoring backend (Prometheus,
/// OpenTelemetry, etc.).  The no-op default [`NoopMetricsSink`] adds zero
/// overhead.
///
/// # Example
///
/// ```rust,ignore
/// #[derive(Debug)]
/// struct MyMetricsSink;
///
/// impl MetricsSink for MyMetricsSink {
///     fn record(&self, metrics: &RunMetrics) {
///         println!("heap: {} bytes, elapsed: {:?}", metrics.peak_heap_bytes, metrics.elapsed);
///     }
/// }
///
/// let config = SandboxConfig {
///     metrics_sink: std::sync::Arc::new(MyMetricsSink),
///     ..SandboxConfig::power_user()
/// };
/// ```
pub trait MetricsSink: Send + Sync + fmt::Debug + 'static {
    /// Called once per run, synchronously, after result extraction completes.
    fn record(&self, metrics: &RunMetrics);
}

/// No-op [`MetricsSink`] — the default when no sink is configured.
#[derive(Debug, Clone, Default)]
pub struct NoopMetricsSink;

impl MetricsSink for NoopMetricsSink {
    fn record(&self, _metrics: &RunMetrics) {}
}

// ─── SandboxConfig ────────────────────────────────────────────────────────────

/// Complete configuration for one `Sandbox` instance.
#[derive(Clone, Debug)]
pub struct SandboxConfig {
    // ── Isolation ───────────────────────────────────────────────────────────
    pub isolation: IsolationLevel,

    // ── Resource limits ─────────────────────────────────────────────────────
    /// Wall-clock budget per `run()` call.
    pub timeout: Duration,

    /// V8 initial heap size (bytes).
    pub heap_initial_bytes: usize,

    /// V8 max heap size (bytes).  OOM → script error, not process crash.
    pub heap_max_bytes: usize,

    /// Maximum number of `console.*` / event emissions per run.
    pub max_log_lines: usize,

    // ── Feature flags ───────────────────────────────────────────────────────
    /// Allow `import … from "sandbox:…"` module specifiers.
    pub allow_modules: bool,

    /// Transpile TypeScript before execution (requires `deno_ast`).
    pub allow_typescript: bool,

    /// Enable `sandbox.emit(event)` → host callback.
    pub allow_events: bool,

    // ── Observability ───────────────────────────────────────────────────────
    /// Receives per-run [`RunMetrics`] after each `run()` completes.
    ///
    /// Defaults to [`NoopMetricsSink`] (zero overhead).
    pub metrics_sink: Arc<dyn MetricsSink>,

    // ── Rate limiting ────────────────────────────────────────────────────────
    /// Per-run operation quotas.  Defaults to no limits.
    pub rate_limits: RateLimitConfig,

    // ── Timer limits ─────────────────────────────────────────────────────────
    /// Maximum number of `setInterval` callback invocations per interval
    /// timer per run.
    ///
    /// Prevents infinite polling by stopping re-armed intervals after this
    /// many calls.  Defaults to 1000.  Has no effect unless `TimerPack` is
    /// registered with the sandbox.
    pub max_interval_calls: usize,
}

// ─── RunCapabilities ──────────────────────────────────────────────────────────

/// Per-run capability constraints that narrow what a single script execution
/// may do, independently of the sandbox-level [`SandboxConfig`].
///
/// All fields are `Option<_>`: `None` means "inherit the sandbox-level default"
/// (no additional restriction beyond what the pool config already imposes).
/// Fields set to `Some(...)` override or further restrict the sandbox
/// configuration for that specific run.
///
/// Pass to [`crate::Sandbox::run_with_caps`] or
/// [`crate::Sandbox::run_streaming_with_caps`]. Use the default (all `None`)
/// for backward-compatible behaviour.
///
/// # Example
///
/// ```rust,ignore
/// use hello_sandbox::RunCapabilities;
///
/// let result = sandbox.run_with_caps(
///     "return kv.get('x')",
///     RunCapabilities {
///         kv_key_prefix: Some("user:123:".into()),
///         http_calls_limit: Some(0),  // no HTTP for this run
///         ..Default::default()
///     },
/// ).await?;
/// ```
#[derive(Clone, Debug, Default)]
pub struct RunCapabilities {
    // ── KV ───────────────────────────────────────────────────────────────────
    /// If `Some(false)`, all KV operations throw `CapabilityDenied` regardless
    /// of whether `KvPack` is registered.  `Some(true)` or `None` = enabled.
    pub kv_enabled: Option<bool>,

    /// If `Some(prefix)`, all KV key reads and writes are transparently
    /// namespaced: the actual stored key becomes `"{prefix}{user_key}"`.
    ///
    /// Scripts only see/use the unnamespaced key — the namespace is invisible
    /// at the JS level.  `op_kv_list` results have the prefix stripped before
    /// being returned to the script.
    pub kv_key_prefix: Option<String>,

    /// If `Some(n)`, override the pool-level `kv_ops_per_run` limit for this
    /// run only.  `Some(0)` blocks all KV operations.  `None` defers to the
    /// sandbox config.
    pub kv_ops_limit: Option<usize>,

    // ── HTTP ─────────────────────────────────────────────────────────────────
    /// If `Some(false)`, all HTTP fetches throw `CapabilityDenied` regardless
    /// of whether `HttpPack` is registered.  `Some(true)` or `None` = enabled.
    pub http_enabled: Option<bool>,

    /// If `Some(prefixes)`, replace the pool-level HTTP allowlist for this run.
    ///
    /// An empty `Vec` blocks all URLs.  Prefixes work exactly like
    /// `HttpConfig::allowed_prefixes`.  `None` defers to the pool-level list.
    pub http_allowed_prefixes: Option<Vec<String>>,

    /// If `Some(methods)`, restrict HTTP to these methods only (case-sensitive,
    /// e.g. `vec!["GET".into(), "HEAD".into()]`).  Any other method throws
    /// `CapabilityDenied`.  `None` = any method is allowed.
    pub http_allowed_methods: Option<Vec<String>>,

    /// If `Some(n)`, override the pool-level `http_calls_per_run` limit for
    /// this run only.  `Some(0)` blocks all HTTP calls.  `None` defers to the
    /// sandbox config.
    pub http_calls_limit: Option<usize>,

    // ── Emit ─────────────────────────────────────────────────────────────────
    /// If `Some(false)`, `sandbox.emit()` calls are silently dropped — events
    /// never reach the host.  `Some(true)` or `None` = enabled.
    pub emit_enabled: Option<bool>,

    /// If `Some(names)`, only events whose `name` is in this set are forwarded
    /// to the host.  Events with other names are silently dropped.
    /// `None` = all events are forwarded.
    pub emit_allowed_names: Option<Vec<String>>,

    /// If `Some(n)`, override the pool-level `emit_calls_per_run` limit for
    /// this run only.  `Some(0)` blocks all emit calls.  `None` defers to the
    /// sandbox config.
    pub emit_calls_limit: Option<usize>,

    // ── Timeout ───────────────────────────────────────────────────────────────
    /// If `Some(d)`, override the sandbox-level `SandboxConfig::timeout` for
    /// this specific run.
    ///
    /// Useful for granting longer budgets to trusted background jobs while
    /// keeping the pool-level default tight.  Must not exceed any OS-level
    /// process timeout.
    pub timeout_override: Option<Duration>,

    // ── Tags ──────────────────────────────────────────────────────────────────
    /// Arbitrary string key-value pairs attached to this run.
    ///
    /// Tags flow in two directions:
    ///   1. **Into the script** — readable as `sandbox.tags()` (returns a
    ///      frozen `Record<string, string>`).
    ///   2. **Into metrics** — forwarded verbatim to [`RunMetrics::tags`] so
    ///      the [`MetricsSink`] can route or annotate observability data without
    ///      needing to correlate on a separate channel.
    ///
    /// Common uses: tenant ID, request ID, feature flag, A/B variant.
    pub tags: HashMap<String, String>,
}

impl SandboxConfig {
    /// Sensible defaults for `IsolationLevel::Trusted`.
    pub fn trusted() -> Self {
        Self {
            isolation: IsolationLevel::Trusted,
            timeout: Duration::from_secs(30),
            heap_initial_bytes: 8 * 1024 * 1024, //  8 MB
            heap_max_bytes: 256 * 1024 * 1024,   // 256 MB
            max_log_lines: 10_000,
            allow_modules: true,
            allow_typescript: true,
            allow_events: true,
            metrics_sink: Arc::new(NoopMetricsSink),
            rate_limits: RateLimitConfig::default(),
            max_interval_calls: 1_000,
        }
    }

    /// Sensible defaults for `IsolationLevel::PowerUser`.
    pub fn power_user() -> Self {
        Self {
            isolation: IsolationLevel::PowerUser,
            timeout: Duration::from_secs(10),
            heap_initial_bytes: 4 * 1024 * 1024, //  4 MB
            heap_max_bytes: 64 * 1024 * 1024,    // 64 MB
            max_log_lines: 1_000,
            allow_modules: true,
            allow_typescript: true,
            allow_events: true,
            metrics_sink: Arc::new(NoopMetricsSink),
            rate_limits: RateLimitConfig::default(),
            max_interval_calls: 1_000,
        }
    }

    /// Sensible defaults for `IsolationLevel::Untrusted`.
    pub fn untrusted() -> Self {
        Self {
            isolation: IsolationLevel::Untrusted,
            timeout: Duration::from_secs(5),
            heap_initial_bytes: 2 * 1024 * 1024, //  2 MB
            heap_max_bytes: 16 * 1024 * 1024,    // 16 MB
            max_log_lines: 200,
            allow_modules: false, // modules disabled for untrusted by default
            allow_typescript: true,
            allow_events: true,
            metrics_sink: Arc::new(NoopMetricsSink),
            rate_limits: RateLimitConfig::default(),
            max_interval_calls: 1_000,
        }
    }
}
