//! `SharedRuntime` — a single `JsRuntime` slot that can execute multiple
//! scripts in sequence.
//!
//! # Threading notes
//!
//! `JsRuntime` is `!Send`. All `async fn` methods on `SharedRuntime` must be
//! called from a `tokio::task::LocalSet`.

use std::borrow::Cow;
use std::collections::HashMap;
use std::ffi::c_void;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use deno_core::{
    v8, Extension, ExtensionFileSource, JsRuntime, ModuleSpecifier, PollEventLoopOptions,
    RuntimeOptions,
};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;
use tracing::{debug, warn};

use crate::config::{IsolationLevel, RateLimitConfig, RunCapabilities, RunMetrics, SandboxConfig};
use crate::event::SandboxEvent;
use crate::loader::AllowlistModuleLoaderBuilder;
use crate::sdk::SdkRegistry;

// ─── Per-run state (placed into OpState before each script execution) ─────────

/// State injected into `OpState` at the start of every `SharedRuntime::run()`.
///
/// Ops that call `#[state] state: &mut RunState` / `state: &mut OpState` and
/// then `state.borrow_mut::<RunState>()` always see the current run's data.
pub struct RunState {
    /// Host-provided key-value inputs for this run.
    pub inputs: HashMap<String, Value>,
    /// Log lines collected by `op_print` (backing `console.*`).
    pub logs: Vec<String>,
    /// Channel for forwarding `sandbox.emit()` events to the host.
    pub events: UnboundedSender<SandboxEvent>,
    /// Wall-clock start time for the current run.
    pub start: Instant,
    /// Maximum number of log lines before `QuotaExceeded` is returned.
    pub max_log_lines: usize,
    /// Set to `true` once the log quota has been exceeded.
    pub log_quota_exceeded: bool,

    // ── Rate limiting ────────────────────────────────────────────────────────
    /// Configured per-run limits (cloned from `SandboxConfig`).
    pub rate_limits: RateLimitConfig,
    /// Number of `fetch` calls made so far in this run.
    pub http_calls: usize,
    /// Number of KV operations made so far in this run.
    pub kv_ops: usize,
    /// Number of `sandbox.emit()` calls made so far in this run.
    pub emit_calls: usize,
    /// Set by an op when a rate limit is first exceeded.
    ///
    /// `(resource_name, limit_value)` — checked by `run()` after the event
    /// loop drains to return `SandboxError::RateLimitExceeded` instead of the
    /// generic runtime error the JS exception would otherwise produce.
    pub rate_limit_exceeded: Option<(String, usize)>,

    // ── Timers (TimerPack) ────────────────────────────────────────────────────
    /// Active timer cancel senders keyed by JS timer ID.
    ///
    /// Dropping a sender causes `op_timer_set`'s cancel receiver to resolve,
    /// returning `false` to the JS `.then` handler which cleans up the
    /// `_cbs` Map entry.  Dropping the entire `HashMap` (when `RunState` is
    /// taken from `OpState` at the end of every run) cancels all pending timers
    /// atomically — the same mechanism used to close the streaming `event_tx`.
    pub timers: HashMap<u32, oneshot::Sender<()>>,

    /// Maximum `setInterval` callback invocations per interval per run.
    /// Copied from `SandboxConfig::max_interval_calls`.
    pub max_interval_calls: usize,

    // ── Per-run capability constraints ────────────────────────────────────────
    /// Per-run capability constraints applied to this execution.
    ///
    /// These narrow what the script may do beyond the sandbox-level defaults.
    /// Ops read this at the start of each operation to enforce restrictions.
    pub capabilities: RunCapabilities,

    // ── Per-run tags ──────────────────────────────────────────────────────────
    /// Host-provided key-value tags for this run (from `RunCapabilities::tags`).
    ///
    /// Readable from scripts via `sandbox.tags()` (backed by `op_read_tags`).
    /// Also forwarded to `RunMetrics::tags` after the run completes.
    pub tags: std::collections::HashMap<String, String>,

    // ── sandbox:assert counters ───────────────────────────────────────────────
    /// Number of `assert.*` calls that evaluated to `true` in this run.
    /// Populated by `op_assert` from `AssertPack`.
    pub assert_passed: usize,

    /// Number of `assert.*` calls that evaluated to `false` in this run.
    /// Populated by `op_assert` from `AssertPack`.
    pub assert_failed: usize,

    // ── sandbox:pm test results ───────────────────────────────────────────────
    /// Results of `pm.test()` calls recorded by `op_pm_test` (PmPack).
    ///
    /// Each entry holds the test name and pass/fail outcome.
    /// Empty when `PmPack` is not registered or no tests ran.
    pub pm_tests: Vec<crate::config::PmTestResult>,
}

// ─── OOM callback ─────────────────────────────────────────────────────────────

/// Data kept alive for the lifetime of the V8 isolate so the near-heap-limit
/// callback can signal an OOM and terminate execution.
struct OomCallbackData {
    oom_flag: Arc<AtomicBool>,
    isolate_handle: v8::IsolateHandle,
}

/// V8 near-heap-limit callback.
///
/// Called by V8 on the script thread when heap usage approaches
/// `config.heap_max_bytes`. Sets the OOM flag and terminates execution so the
/// script is cleanly aborted instead of crashing the process.
///
/// Returning `current_heap_limit + 1 MiB` buys V8 enough space to unwind
/// cleanly without triggering a second callback before `terminate_execution`
/// takes effect.
///
/// # Safety
/// `data` must be a valid `*const OomCallbackData` that outlives the isolate.
/// This is guaranteed because `OomCallbackData` is stored in a `Box` inside
/// `SharedRuntime`, and `SharedRuntime` drops `JsRuntime` (and thus the
/// isolate) **before** the `Box`.
unsafe extern "C" fn near_heap_limit_callback(
    data: *mut c_void,
    current_heap_limit: usize,
    _initial_heap_limit: usize,
) -> usize {
    let d = &*(data as *const OomCallbackData);
    d.oom_flag.store(true, Ordering::Release);
    d.isolate_handle.terminate_execution();
    // Return slightly more so V8 has room to unwind without a second callback.
    current_heap_limit + (1024 * 1024)
}

// ─── Import hoisting ──────────────────────────────────────────────────────────

/// Split `source` into `(imports, body)` where `imports` contains all leading
/// top-level `import` statements (with a trailing newline each) and `body` is
/// the rest of the source.
///
/// Static `import` declarations cannot appear inside an async function body.
/// By hoisting them to the module top level we allow scripts to mix `import`
/// statements with an `async`-IIFE body that uses `return`.
fn hoist_imports(source: &str) -> (String, String) {
    let mut imports = String::new();
    let mut body = String::new();
    // Blank lines before any import belong in the imports section too
    // so they don't accidentally set `past_imports = true`.
    let mut pending_blanks = String::new();
    let mut past_imports = false;

    for line in source.lines() {
        let trimmed = line.trim();
        if !past_imports && trimmed.is_empty() {
            // Hold blank lines: they precede the imports section.
            pending_blanks.push_str(line);
            pending_blanks.push('\n');
        } else if !past_imports
            && (trimmed.starts_with("import ") || trimmed.starts_with("import{"))
        {
            // Flush any accumulated blank lines into the imports section.
            imports.push_str(&pending_blanks);
            pending_blanks.clear();
            imports.push_str(line);
            imports.push('\n');
        } else {
            // Non-import, non-blank line: everything from here goes into body.
            past_imports = true;
            body.push_str(&pending_blanks);
            pending_blanks.clear();
            body.push_str(line);
            body.push('\n');
        }
    }
    // Trailing blank lines (after all imports, before any body) go to body.
    body.push_str(&pending_blanks);

    (imports, body)
}

// ─── SharedRuntime ────────────────────────────────────────────────────────────

/// A single `JsRuntime` slot capable of executing JS/TS scripts sequentially.
///
/// Build one with [`SharedRuntime::new`], then call [`SharedRuntime::run`] for
/// each script. The runtime is reused across runs; each run gets a fresh
/// `RunState` in `OpState`.
///
/// **Must be used on a `tokio::task::LocalSet`** — `JsRuntime` is `!Send`.
pub struct SharedRuntime {
    // Field order matters for drop order: `runtime` is dropped first, which
    // tears down the V8 isolate before `_oom_data` is freed. This ensures the
    // near-heap-limit callback cannot fire after the callback data is freed.
    runtime: JsRuntime,
    run_counter: u64,
    config: SandboxConfig,
    /// Set to `true` by the near-heap-limit callback.  Reset at the start of
    /// each `run()`.
    oom_detected: Arc<AtomicBool>,
    /// Keeps the callback data alive for the lifetime of the V8 isolate.
    _oom_data: Box<OomCallbackData>,
}

impl SharedRuntime {
    /// Number of scripts executed so far on this runtime slot.
    pub fn run_count(&self) -> u64 {
        self.run_counter
    }

    /// Create a new runtime from a config, module loader builder, and SDK registry.
    ///
    /// - `ext:` ESM files from the registry are embedded in the extension and
    ///   evaluated eagerly at startup (entry point).
    /// - `sandbox:` ESM files are registered with the loader builder and loaded
    ///   lazily when scripts import them.
    /// - Per-slot op state (e.g. `KvStore`, `HttpState`) is injected after
    ///   the runtime is constructed.
    /// - V8 heap limits from `config.heap_initial_bytes` / `config.heap_max_bytes`
    ///   are wired into the isolate via `v8::CreateParams`.
    ///
    /// **Must be called from a `tokio::task::LocalSet`.**
    pub fn new(
        config: SandboxConfig,
        loader: AllowlistModuleLoaderBuilder,
        sdk: &SdkRegistry,
    ) -> Self {
        // ── Snapshot fast-init ────────────────────────────────────────────────
        //
        // When a pre-baked V8 snapshot is available (built by `make-snapshot`),
        // `core.js` has already been evaluated and its side effects (prototype
        // freezes, `globalThis` freeze, `__sandbox_ops` install) are baked in.
        //
        // In snapshot mode:
        //   - `ext:` ESM files are NOT included in the Extension (already baked).
        //   - `esm_entry_point` is NOT set (would fail: `Deno` is gone after freeze).
        //   - All ops are still registered — V8 re-wires native op handles on load.
        //   - `sandbox:` ESM files still go into the loader for lazy `import`s.
        //   - `startup_snapshot` is set in `RuntimeOptions`.
        let snapshot_bytes = crate::snapshot::get_snapshot();
        let using_snapshot = snapshot_bytes.is_some();

        // ── Collect ops ──────────────────────────────────────────────────────
        //
        // When a snapshot is loaded, ALL built-in pack ops must appear first
        // (in the same order as snapshot creation) so V8's external-reference
        // table bounds check passes (`ops.len() >= ops_in_snapshot`).
        //
        // Custom pack ops come after and are deduplicated against the builtin
        // set so registering a builtin pack explicitly does not duplicate ops.
        //
        // When no snapshot is used, take the fast path: just use the ops from
        // the user-registered packs.
        let all_ops = if using_snapshot {
            let mut ops = crate::snapshot::builtin_ops();
            let builtin_names: std::collections::HashSet<&str> =
                ops.iter().map(|o| o.name).collect();
            for op in sdk.all_ops() {
                if !builtin_names.contains(op.name) {
                    ops.push(op);
                }
            }
            ops
        } else {
            sdk.all_ops()
        };

        // ── Pre-freeze global injection ───────────────────────────────────────
        //
        // Some packs (e.g. TimerPack) need to install globals on `globalThis`
        // before `Object.freeze(globalThis)` runs in `core.js`. They provide
        // JS snippets via `SdkExtension::pre_freeze_globals()`.
        //
        // We collect all such snippets and do a text substitution on `core.js`
        // source, replacing `// PRE_FREEZE_INJECTION` with the joined code.
        // This substitution happens here (not at compile time) so custom packs
        // registered at runtime can also inject globals.
        //
        // In snapshot mode this is a no-op: the injection was applied when the
        // snapshot was created (see `make_snapshot.rs`), so the globals are
        // already baked into the frozen `globalThis`.
        let pre_freeze = sdk.collect_pre_freeze_globals();

        // ── ext: ESM files go into the Extension (evaluated eagerly) ─────────
        //
        // Skipped when a snapshot is loaded — core.js is already evaluated.
        let esm_sources: Vec<ExtensionFileSource> = if using_snapshot {
            vec![]
        } else {
            sdk.all_esm_files()
                .into_iter()
                .filter(|(spec, _)| spec.starts_with("ext:"))
                .map(|(spec, src)| {
                    // Apply pre-freeze injection: substitute the marker in
                    // core.js with any pack-provided global-install code.
                    let src: Arc<str> = if !pre_freeze.is_empty() {
                        Arc::from(src.replace("// PRE_FREEZE_INJECTION", &pre_freeze))
                    } else {
                        Arc::from(src)
                    };
                    ExtensionFileSource::new_computed(spec, src)
                })
                .collect()
        };

        // Snapshot: skip entry point (already ran during snapshot creation).
        let entry_point = if using_snapshot {
            None
        } else {
            sdk.esm_entry_point()
        };

        // ── Assemble a single dynamic Extension ──────────────────────────────
        let sdk_ext = Extension {
            name: "sandbox_sdk",
            ops: Cow::Owned(all_ops),
            esm_files: Cow::Owned(esm_sources),
            esm_entry_point: entry_point,
            ..Default::default()
        };

        // ── sandbox: ESM files go into the AllowlistModuleLoader ─────────────
        let mut builder = loader;
        for (spec, src) in sdk.all_esm_files() {
            if spec.starts_with("sandbox:") {
                builder = builder.register(spec, src);
            }
        }
        let module_loader = builder.build().expect("AllowlistModuleLoaderBuilder::build failed");

        // ── Wire V8 heap limits ───────────────────────────────────────────────
        let create_params = v8::CreateParams::default()
            .heap_limits(config.heap_initial_bytes, config.heap_max_bytes);

        let mut runtime = JsRuntime::new(RuntimeOptions {
            extensions: vec![sdk_ext],
            module_loader: Some(Rc::new(module_loader)),
            create_params: Some(create_params),
            startup_snapshot: snapshot_bytes,
            ..Default::default()
        });

        // ── Security post-snapshot setup ──────────────────────────────────────
        //
        // When loading a snapshot, `Object.freeze(globalThis)` and
        // `delete globalThis.Deno` were intentionally excluded from the snapshot
        // (see `make_snapshot.rs` for the trimming logic and rationale).
        //
        // We apply them here immediately after the runtime is constructed, before
        // any user script runs, so the security contract is still upheld.
        //
        // In non-snapshot mode these operations are part of `core.js` itself and
        // run during the ESM entry-point evaluation above — no extra step needed.
        if using_snapshot {
            runtime
                .execute_script(
                    "<sandbox_security>",
                    "delete globalThis.Deno; Object.freeze(globalThis);",
                )
                .expect("snapshot security post-init failed");
        }

        // ── Inject per-slot SDK state (KvStore, HttpState, etc.) ─────────────
        {
            let op_state_rc = runtime.op_state();
            let mut op_state = op_state_rc.borrow_mut();
            sdk.inject_all_op_state(&mut op_state);
        }

        // ── Install near-heap-limit OOM callback ──────────────────────────────
        //
        // The callback sets `oom_detected` and calls `terminate_execution()`
        // so the script is cleanly aborted rather than crashing the process.
        //
        // Safety contract: `OomCallbackData` lives inside `_oom_data` (a `Box`
        // field of `SharedRuntime`). Because Rust drops struct fields in
        // declaration order, `runtime` (the isolate) is dropped before
        // `_oom_data`, ensuring the callback data outlives the isolate.
        let oom_detected = Arc::new(AtomicBool::new(false));
        let isolate_handle = runtime.v8_isolate().thread_safe_handle();
        let oom_data = Box::new(OomCallbackData {
            oom_flag: Arc::clone(&oom_detected),
            isolate_handle,
        });
        // SAFETY: see contract above.
        // The `add_near_heap_limit_callback` call is inherently unsafe because
        // it accepts a raw callback pointer and raw data pointer.
        #[allow(unused_unsafe)]
        unsafe {
            runtime.v8_isolate().add_near_heap_limit_callback(
                near_heap_limit_callback,
                &*oom_data as *const OomCallbackData as *mut c_void,
            );
        }

        Self {
            runtime,
            run_counter: 0,
            config,
            oom_detected,
            _oom_data: oom_data,
        }
    }

    /// Execute `source` (JS or TS) in an isolated module scope.
    ///
    /// Returns `(return_value, logs, metrics)` on success. The return value is
    /// the result of the last expression or an explicit `return` statement in
    /// the script, serialised as `serde_json::Value`. `metrics` contains heap
    /// usage and elapsed time for this run.
    ///
    /// **Must be called from a `tokio::task::LocalSet`.**
    pub async fn run(
        &mut self,
        source: &str,
        inputs: HashMap<String, Value>,
        event_tx: UnboundedSender<SandboxEvent>,
        mut capabilities: RunCapabilities,
    ) -> Result<(Value, Vec<String>, RunMetrics), crate::SandboxError> {
        self.run_counter += 1;
        let run_id = self.run_counter;
        let run_start = std::time::Instant::now();

        // Reset the OOM flag so a previous OOM does not poison this run.
        self.oom_detected.store(false, Ordering::Release);

        // ── TypeScript transpile (if enabled) ────────────────────────────────
        let js_source = if self.config.allow_typescript {
            crate::transpile::transpile(&format!("sandbox:run/{run_id}"), source, false)?
        } else {
            source.to_string()
        };

        // ── Extract per-run overrides from capabilities ───────────────────────
        //
        // `timeout_override` and `tags` must be extracted BEFORE capabilities
        // is moved into RunState, so they remain accessible for the watchdog
        // thread and for RunMetrics construction after the event loop.
        let effective_timeout = capabilities.timeout_override.unwrap_or(self.config.timeout);
        let run_tags = std::mem::take(&mut capabilities.tags);

        // ── Inject fresh per-run state into OpState ───────────────────────────
        self.runtime.op_state().borrow_mut().put(RunState {
            inputs,
            logs: Vec::new(),
            events: event_tx,
            start: Instant::now(),
            max_log_lines: self.config.max_log_lines,
            log_quota_exceeded: false,
            rate_limits: self.config.rate_limits.clone(),
            http_calls: 0,
            kv_ops: 0,
            emit_calls: 0,
            rate_limit_exceeded: None,
            timers: HashMap::new(),
            max_interval_calls: self.config.max_interval_calls,
            tags: run_tags.clone(),
            capabilities,
            assert_passed: 0,
            assert_failed: 0,
            pm_tests: Vec::new(),
        });

        // ── Watchdog thread (PowerUser + Untrusted) ───────────────────────────
        //
        // Two flags:
        //  `cancel`        — set after the run completes, prevents watchdog firing.
        //  `timed_out`     — set by watchdog just before it calls terminate_execution.
        //
        // After the run, if `timed_out` is true we return `SandboxError::Timeout`
        // regardless of whatever error V8 surfaced.
        let cancel = Arc::new(AtomicBool::new(false));
        let timed_out = Arc::new(AtomicBool::new(false));
        let _watchdog = if self.config.isolation != IsolationLevel::Trusted {
            let cancel_w = cancel.clone();
            let timed_out_w = timed_out.clone();
            let deadline = effective_timeout;
            let handle = self.runtime.v8_isolate().thread_safe_handle();
            Some(std::thread::spawn(move || {
                std::thread::sleep(deadline);
                if !cancel_w.load(Ordering::Relaxed) {
                    warn!("watchdog: terminating after {:?}", deadline);
                    timed_out_w.store(true, Ordering::Relaxed);
                    handle.terminate_execution();
                }
            }))
        } else {
            None
        };

        // ── Wrap script to capture the return value ───────────────────────────
        //
        // Static `import` declarations must be at the module top level — they
        // cannot appear inside an async IIFE.  We hoist them above the wrapper
        // so the module loader can resolve them before the IIFE body executes.
        //
        // The IIFE captures the `return` value and emits it via `console.log`
        // with a sentinel prefix that `run()` extracts after the event loop.
        let (imports, body) = hoist_imports(&js_source);
        let wrapped = format!(
            r#"{imports}const __result = await (async () => {{
{body}
}})();
console.log("__RETURN__:" + JSON.stringify(__result ?? null));"#,
            imports = imports,
            body = body,
        );

        let specifier = ModuleSpecifier::parse(&format!("sandbox:run/{run_id}")).unwrap();

        // ── Load, evaluate, and drain the event loop ─────────────────────────
        let result = async {
            // Use `load_side_es_module_from_code` so multiple runs can share
            // a single JsRuntime — main-module status is irrelevant here.
            let mod_id = self
                .runtime
                .load_side_es_module_from_code(&specifier, wrapped)
                .await
                .map_err(|e| crate::SandboxError::Runtime(anyhow::anyhow!("{e}")))?;

            let recv = self.runtime.mod_evaluate(mod_id);
            self.runtime
                .run_event_loop(PollEventLoopOptions::default())
                .await
                .map_err(|e| crate::SandboxError::Runtime(anyhow::anyhow!("{e}")))?;
            recv.await.map_err(|e| crate::SandboxError::Runtime(anyhow::anyhow!("{e}")))?;

            Ok::<_, crate::SandboxError>(())
        }
        .await;

        // Signal the watchdog that the run completed (prevents spurious firing).
        cancel.store(true, Ordering::Relaxed);

        // ── Extract and clean up RunState ──────────────────────────────────────
        //
        // Always take RunState out of OpState immediately after the event loop.
        // This drops `event_tx`, which closes any streaming receiver the caller
        // is holding — even when the runtime is returned to a warm pool slot
        // (where the JsRuntime itself is not dropped between runs).
        //
        // `rate_limit_exceeded` is extracted here instead of being read via a
        // separate borrow later, so we can do the cleanup in one place.
        let (
            http_calls,
            kv_ops,
            emit_calls,
            assert_passed,
            assert_failed,
            pm_tests,
            mut logs,
            log_quota_exceeded,
            rate_limit_exceeded,
        ) = {
            let op_state_rc = self.runtime.op_state();
            let mut op_state = op_state_rc.borrow_mut();
            let fields = {
                let run_state = op_state.borrow_mut::<RunState>();
                (
                    run_state.http_calls,
                    run_state.kv_ops,
                    run_state.emit_calls,
                    run_state.assert_passed,
                    run_state.assert_failed,
                    std::mem::take(&mut run_state.pm_tests),
                    std::mem::take(&mut run_state.logs),
                    run_state.log_quota_exceeded,
                    run_state.rate_limit_exceeded.clone(),
                )
            };
            // Remove RunState from OpState — drops event_tx, closing streaming receivers.
            op_state.try_take::<RunState>();
            fields
        };

        // ── Error priority: OOM > Timeout > RateLimit > other errors ──────────
        //
        // Both OOM and Timeout call `terminate_execution()`, which produces a
        // generic V8 termination error. We check OOM first since the OOM
        // callback fires synchronously on the script thread before the watchdog
        // thread has a chance to set the timeout flag.
        //
        // Rate-limit errors are JS exceptions thrown by ops: they appear as
        // `SandboxError::Runtime` in `result`. We intercept them here by
        // checking the flag set by the op before the generic error propagates.
        if self.oom_detected.load(Ordering::Acquire) {
            return Err(crate::SandboxError::OutOfMemory);
        }
        if timed_out.load(Ordering::Relaxed) {
            return Err(crate::SandboxError::Timeout(effective_timeout));
        }
        if result.is_err() {
            if let Some((resource, limit)) = rate_limit_exceeded {
                return Err(crate::SandboxError::RateLimitExceeded { resource, limit });
            }
        }
        result?;

        // ── Extract return value from the log buffer ──────────────────────────
        let value = logs
            .iter()
            .rposition(|l| l.starts_with("__RETURN__:"))
            .map(|idx| {
                let line = logs.remove(idx);
                serde_json::from_str(&line["__RETURN__:".len()..]).unwrap_or(Value::Null)
            })
            .unwrap_or(Value::Null);

        if log_quota_exceeded {
            return Err(crate::SandboxError::QuotaExceeded(self.config.max_log_lines));
        }

        // ── Collect V8 heap statistics ────────────────────────────────────────
        //
        // Polled after the event loop drains so we capture the peak allocation
        // from this run.  `get_heap_statistics` is a cheap V8 API call.
        let peak_heap_bytes = {
            let stats = self.runtime.v8_isolate().get_heap_statistics();
            stats.used_heap_size()
        };

        let metrics = RunMetrics {
            peak_heap_bytes,
            elapsed: run_start.elapsed(),
            http_calls,
            kv_ops,
            emit_calls,
            tags: run_tags,
            assertions_passed: assert_passed,
            assertions_failed: assert_failed,
            pm_tests,
        };

        // Call the configured metrics sink synchronously (zero-overhead no-op
        // by default) before returning to the pool/caller.
        self.config.metrics_sink.record(&metrics);

        debug!(run_id, "run complete");
        Ok((value, logs, metrics))
    }
}
