# Config — SandboxConfig, RunCapabilities, RunMetrics, MetricsSink

`src/config.rs` defines the configuration and observability types for the
sandbox.

---

## SandboxConfig

Global configuration applied to the entire pool:

```rust
pub struct SandboxConfig {
    pub isolation: IsolationLevel,        // Trusted | PowerUser | Untrusted
    pub timeout: Duration,                // default per-run timeout (default: 30s)
    pub initial_heap_bytes: usize,        // V8 initial heap size (default: 8 MiB)
    pub max_heap_bytes: usize,            // V8 max heap size (default: 64 MiB)
    pub max_log_lines: usize,             // cap on console.log lines (default: 1000)
    pub typescript_enabled: bool,         // allow TS syntax (default: true)
    pub modules_enabled: bool,            // allow ESM imports (default: true)
    pub events_enabled: bool,             // allow sandbox.emit() (default: true)
    pub rate_limits: RateLimitConfig,     // per-run call quotas
    pub metrics_sink: Arc<dyn MetricsSink>, // observer for run metrics
    pub max_interval_calls: usize,        // max setInterval re-arms (default: 1000)
}
```

### Constructors

```rust
SandboxConfig::trusted()     // no watchdog, no limits
SandboxConfig::power_user()  // watchdog, default limits (recommended)
SandboxConfig::untrusted()   // child-process isolation (Linux only)
```

---

## IsolationLevel

```rust
pub enum IsolationLevel {
    Trusted,     // in-process, no watchdog thread
    PowerUser,   // in-process + watchdog thread (default)
    Untrusted,   // child process + seccomp (Linux); falls back to PowerUser on non-Linux
}
```

See [Isolation](isolation.md) for the child-process architecture.

---

## RateLimitConfig

Per-run call quotas. `None` means unlimited:

```rust
pub struct RateLimitConfig {
    pub http_calls_per_run: Option<usize>,  // default: None
    pub kv_ops_per_run: Option<usize>,      // default: None
    pub emit_calls_per_run: Option<usize>,  // default: None
}
```

When a limit is exceeded the op returns
`SandboxError::RateLimitExceeded { resource, limit }`.

---

## RunCapabilities

Per-run constraints that narrow what a single script execution is allowed to do.
All fields are `Option` — `None` means "inherit from `SandboxConfig`" (no
restriction).

```rust
pub struct RunCapabilities {
    // HTTP
    pub http_enabled: Option<bool>,
    pub http_allowed_methods: Option<Vec<String>>,
    pub http_allowed_prefixes: Option<Vec<String>>,
    pub http_calls_limit: Option<usize>,

    // KV
    pub kv_enabled: Option<bool>,
    pub kv_key_prefix: Option<String>,     // prepended to all keys; stripped from list results
    pub kv_ops_limit: Option<usize>,

    // Emit
    pub emit_enabled: Option<bool>,
    pub emit_allowed_names: Option<Vec<String>>,
    pub emit_calls_limit: Option<usize>,

    // Timeout override
    pub timeout_override: Option<Duration>, // overrides SandboxConfig::timeout for this run

    // Per-run metadata
    pub tags: HashMap<String, String>,      // forwarded to RunMetrics::tags
}
```

`Default::default()` is a no-op capabilities object (nothing restricted).

### Usage

```rust
let caps = RunCapabilities {
    http_enabled: Some(true),
    http_allowed_prefixes: Some(vec!["https://api.example.com".into()]),
    http_calls_limit: Some(10),
    kv_key_prefix: Some("user:42:".into()),
    tags: [("phase".into(), "post".into())].into(),
    ..Default::default()
};
sandbox.run_with_caps(script, caps).await?;
```

---

## RunMetrics

Collected after every run and returned in `SandboxResult::metrics`:

```rust
pub struct RunMetrics {
    pub peak_heap_bytes: usize,
    pub elapsed: Duration,
    pub http_calls: usize,
    pub kv_ops: usize,
    pub emit_calls: usize,
    pub tags: HashMap<String, String>,         // from RunCapabilities::tags
    pub assertions_passed: usize,              // AssertPack
    pub assertions_failed: usize,              // AssertPack
    pub pm_tests: Vec<PmTestResult>,           // PmPack
}
```

`RunMetrics` is `#[non_exhaustive]` — new fields may be added without a breaking
change.

### PmTestResult

```rust
pub struct PmTestResult {
    pub name: String,
    pub passed: bool,
}
```

---

## MetricsSink

Observer trait called synchronously after every run:

```rust
pub trait MetricsSink: Send + Sync + Debug + 'static {
    fn observe(&self, metrics: &RunMetrics);
}
```

The default implementation is `NoopMetricsSink` (zero overhead).

### Custom Sink Example

```rust
#[derive(Debug)]
struct LogSink;

impl MetricsSink for LogSink {
    fn observe(&self, m: &RunMetrics) {
        println!("heap={} elapsed={:?} http={}", m.peak_heap_bytes, m.elapsed, m.http_calls);
    }
}

let config = SandboxConfig {
    metrics_sink: Arc::new(LogSink),
    ..SandboxConfig::power_user()
};
```

---

## Source

`src/config.rs`
