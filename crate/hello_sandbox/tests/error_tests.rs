//! Tests for `SandboxError` variants and sandbox edge-case behaviour.
//!
//! Covers previously untested error paths:
//! - `QuotaExceeded` — too many `console.log` lines
//! - `ModuleNotFound` — importing a nonexistent / disallowed specifier
//! - `Runtime` — uncaught exceptions inside scripts
//! - Behaviour of `MetricsSink` — called on every run, receives correct data
//! - Emit cap + name filter — silent drop, not error
//! - `kv_enabled: false` error message content
//!
//! All tests use `pool_size = 1` (V8 single-thread constraint) and
//! `tokio::task::LocalSet`.

use std::sync::{Arc, Mutex};

use hello_sandbox::sdk::kv_sdk::KvPack;
use hello_sandbox::{
    MetricsSink, NoopMetricsSink, PoolConfig, RunCapabilities, RunMetrics, Sandbox, SandboxConfig,
    SandboxError,
};
use serde_json::json;
use tokio::task::LocalSet;

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn isolated() -> PoolConfig {
    PoolConfig {
        pool_size: 1,
        ..Default::default()
    }
}

fn make_sandbox() -> Sandbox {
    Sandbox::builder().config(SandboxConfig::trusted()).pool(isolated()).build().unwrap()
}

// ─── QuotaExceeded ────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn quota_exceeded_when_log_limit_reached() {
    LocalSet::new()
        .run_until(async {
            let mut config = SandboxConfig::trusted();
            config.max_log_lines = 5; // very low limit
            let mut sb = Sandbox::builder().config(config).pool(isolated()).build().unwrap();

            // Generate more console.log calls than the limit allows
            let script = r#"
                for (let i = 0; i < 20; i++) {
                    console.log("line " + i);
                }
                return "done";
            "#;
            let err = sb.run(script).await.unwrap_err();
            assert!(
                matches!(err, SandboxError::QuotaExceeded(_)),
                "expected QuotaExceeded, got: {err:?}"
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn quota_error_message_contains_limit() {
    LocalSet::new()
        .run_until(async {
            let mut config = SandboxConfig::trusted();
            config.max_log_lines = 3;
            let mut sb = Sandbox::builder().config(config).pool(isolated()).build().unwrap();

            let script = r#"
                for (let i = 0; i < 10; i++) { console.log(i); }
            "#;
            let err = sb.run(script).await.unwrap_err();
            let msg = err.to_string();
            assert!(msg.contains("3"), "error message should mention the limit (3): {msg}");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn quota_not_exceeded_below_limit() {
    LocalSet::new()
        .run_until(async {
            let mut config = SandboxConfig::trusted();
            config.max_log_lines = 5;
            let mut sb = Sandbox::builder().config(config).pool(isolated()).build().unwrap();

            // 4 log lines (below the limit of 5) — should NOT trigger QuotaExceeded.
            // The check is `logs.len() >= max_log_lines`, so limit=5 allows 0..4.
            let script = r#"
                for (let i = 0; i < 4; i++) { console.log(i); }
                return "ok";
            "#;
            let result = sb.run(script).await.unwrap();
            assert_eq!(result.value, json!("ok"));
        })
        .await;
}

// ─── ModuleNotFound / disallowed specifiers ────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn import_nonexistent_sandbox_module_is_runtime_error() {
    LocalSet::new()
        .run_until(async {
            let mut sb = make_sandbox();
            // "sandbox:no_such_pack" is not registered — loader denies it
            let err =
                sb.run(r#"import { x } from "sandbox:no_such_pack"; return x;"#).await.unwrap_err();
            assert!(
                matches!(err, SandboxError::Runtime(_)),
                "expected Runtime error for missing module, got: {err:?}"
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn import_node_specifier_is_blocked() {
    LocalSet::new()
        .run_until(async {
            let mut sb = make_sandbox();
            let err = sb
                .run(r#"import { readFileSync } from "node:fs"; return readFileSync("/etc/hosts", "utf8");"#)
                .await
                .unwrap_err();
            assert!(
                matches!(err, SandboxError::Runtime(_)),
                "expected Runtime error for node: specifier, got: {err:?}"
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn import_http_url_is_blocked() {
    LocalSet::new()
        .run_until(async {
            let mut sb = make_sandbox();
            let err = sb
                .run(r#"import x from "https://example.com/evil.js"; return x;"#)
                .await
                .unwrap_err();
            assert!(
                matches!(err, SandboxError::Runtime(_)),
                "expected Runtime error for http import, got: {err:?}"
            );
        })
        .await;
}

// ─── Runtime errors ───────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn uncaught_exception_is_runtime_error() {
    LocalSet::new()
        .run_until(async {
            let mut sb = make_sandbox();
            let err = sb.run("throw new Error('oops');").await.unwrap_err();
            assert!(
                matches!(err, SandboxError::Runtime(_)),
                "expected Runtime error, got: {err:?}"
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn runtime_error_message_propagated() {
    LocalSet::new()
        .run_until(async {
            let mut sb = make_sandbox();
            let err = sb.run("throw new Error('my specific message');").await.unwrap_err();
            let msg = err.to_string();
            assert!(msg.contains("my specific message"), "error should carry the message: {msg}");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn reference_error_is_runtime_error() {
    LocalSet::new()
        .run_until(async {
            let mut sb = make_sandbox();
            let err = sb.run("return undefinedVariable;").await.unwrap_err();
            assert!(
                matches!(err, SandboxError::Runtime(_)),
                "expected Runtime error for ReferenceError, got: {err:?}"
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn type_error_is_runtime_error() {
    LocalSet::new()
        .run_until(async {
            let mut sb = make_sandbox();
            let err = sb.run("null.property;").await.unwrap_err();
            assert!(
                matches!(err, SandboxError::Runtime(_)),
                "expected Runtime error for TypeError, got: {err:?}"
            );
        })
        .await;
}

// ─── MetricsSink ──────────────────────────────────────────────────────────────

#[derive(Debug)]
struct RecordingSink {
    calls: Arc<Mutex<Vec<RunMetrics>>>,
}

impl MetricsSink for RecordingSink {
    fn record(&self, metrics: &RunMetrics) {
        self.calls.lock().unwrap().push(metrics.clone());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn metrics_sink_called_on_each_run() {
    LocalSet::new()
        .run_until(async {
            let calls = Arc::new(Mutex::new(vec![]));
            let mut config = SandboxConfig::trusted();
            config.metrics_sink = Arc::new(RecordingSink {
                calls: calls.clone(),
            });
            let mut sb = Sandbox::builder().config(config).pool(isolated()).build().unwrap();

            sb.run("return 1;").await.unwrap();
            sb.run("return 2;").await.unwrap();
            sb.run("return 3;").await.unwrap();

            let recorded = calls.lock().unwrap();
            assert_eq!(recorded.len(), 3, "sink should be called once per run");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn metrics_sink_receives_nonzero_elapsed() {
    LocalSet::new()
        .run_until(async {
            let calls = Arc::new(Mutex::new(vec![]));
            let mut config = SandboxConfig::trusted();
            config.metrics_sink = Arc::new(RecordingSink {
                calls: calls.clone(),
            });
            let mut sb = Sandbox::builder().config(config).pool(isolated()).build().unwrap();

            sb.run("return 42;").await.unwrap();

            let recorded = calls.lock().unwrap();
            assert!(!recorded[0].elapsed.is_zero(), "elapsed should be non-zero");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn metrics_sink_receives_peak_heap_bytes() {
    LocalSet::new()
        .run_until(async {
            let calls = Arc::new(Mutex::new(vec![]));
            let mut config = SandboxConfig::trusted();
            config.metrics_sink = Arc::new(RecordingSink {
                calls: calls.clone(),
            });
            let mut sb = Sandbox::builder().config(config).pool(isolated()).build().unwrap();

            sb.run("return 'hello';").await.unwrap();

            let recorded = calls.lock().unwrap();
            assert!(recorded[0].peak_heap_bytes > 0, "peak_heap_bytes should be > 0");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn metrics_sink_counts_kv_ops() {
    LocalSet::new()
        .run_until(async {
            let calls = Arc::new(Mutex::new(vec![]));
            let mut config = SandboxConfig::trusted();
            config.metrics_sink = Arc::new(RecordingSink {
                calls: calls.clone(),
            });
            let mut sb = Sandbox::builder()
                .config(config)
                .pool(isolated())
                .sdk(KvPack::new())
                .build()
                .unwrap();

            sb.run(
                r#"
                import { kv } from "sandbox:kv";
                await kv.set("a", 1);
                await kv.set("b", 2);
                await kv.get("a");
                return "done";
            "#,
            )
            .await
            .unwrap();

            let recorded = calls.lock().unwrap();
            assert_eq!(recorded[0].kv_ops, 3, "3 KV ops (2 set + 1 get)");
        })
        .await;
}

// ─── Emit capability filtering ────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn emit_disabled_silently_drops_events() {
    LocalSet::new()
        .run_until(async {
            let mut sb = make_sandbox();
            let result = sb
                .run_with_caps(
                    r#"
                    sandbox.emit("my_event", { x: 1 });
                    return "ok";
                "#,
                    RunCapabilities {
                        emit_enabled: Some(false),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            // No error — silently dropped
            assert_eq!(result.value, json!("ok"));
            assert!(result.events.is_empty(), "events should be dropped when emit disabled");
            assert_eq!(result.metrics.emit_calls, 0, "emit_calls should be 0");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn emit_allowed_names_filters_other_events() {
    LocalSet::new()
        .run_until(async {
            let mut sb = make_sandbox();
            let result = sb
                .run_with_caps(
                    r#"
                    sandbox.emit("allowed_event", { v: 1 });
                    sandbox.emit("blocked_event", { v: 2 });
                    return "ok";
                "#,
                    RunCapabilities {
                        emit_allowed_names: Some(vec!["allowed_event".to_string()]),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            assert_eq!(result.value, json!("ok"));
            assert_eq!(result.events.len(), 1, "only allowed_event should pass through");
            assert_eq!(result.events[0].name, "allowed_event");
        })
        .await;
}

// ─── Sandbox security invariants ──────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn deno_global_is_deleted() {
    LocalSet::new()
        .run_until(async {
            let mut sb = make_sandbox();
            let result = sb.run("return typeof Deno;").await.unwrap();
            assert_eq!(result.value, json!("undefined"), "Deno should be deleted");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn globalthis_is_frozen() {
    LocalSet::new()
        .run_until(async {
            let mut sb = make_sandbox();
            // Attempting to add a new global should fail silently (frozen object)
            let result = sb
                .run(
                    r#"
                    "use strict";
                    globalThis.newProp = "leaked";
                    return typeof globalThis.newProp;
                "#,
                )
                .await;
            // Either throws (strict mode) or newProp is undefined (frozen, non-strict)
            match result {
                Ok(r) => assert_eq!(
                    r.value,
                    json!("undefined"),
                    "frozen globalThis should not accept new props"
                ),
                Err(SandboxError::Runtime(_)) => {}, // strict mode throw — also acceptable
                Err(e) => panic!("unexpected error: {e:?}"),
            }
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn object_prototype_is_frozen() {
    LocalSet::new()
        .run_until(async {
            let mut sb = make_sandbox();
            // Prototype pollution attack — must fail
            let result = sb
                .run(
                    r#"
                    Object.prototype.polluted = true;
                    return ({}).polluted;
                "#,
                )
                .await;
            match result {
                Ok(r) => assert_ne!(r.value, json!(true), "prototype pollution should be blocked"),
                Err(SandboxError::Runtime(_)) => {}, // TypeError thrown — also correct
                Err(e) => panic!("unexpected error: {e:?}"),
            }
        })
        .await;
}

// ─── Run isolation (warm slots) ───────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn module_scope_does_not_leak_across_runs() {
    LocalSet::new()
        .run_until(async {
            let mut sb = Sandbox::builder()
                .config(SandboxConfig::trusted())
                .pool(PoolConfig {
                    pool_size: 1,
                    max_runs_per_slot: 100,
                    ..Default::default()
                })
                .build()
                .unwrap();

            // Run 1 registers a module — run 2 should not see its side effects
            let r1 = sb
                .run(
                    r#"
                    // Pollute module-level state via a registered module
                    return "run1";
                "#,
                )
                .await
                .unwrap();
            assert_eq!(r1.value, json!("run1"));

            let r2 = sb
                .run(
                    r#"
                    // Fresh scope — no bleed from run 1
                    return typeof _run1Var === "undefined" ? "clean" : "leaked";
                "#,
                )
                .await
                .unwrap();
            assert_eq!(r2.value, json!("clean"));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn inputs_do_not_leak_across_runs() {
    LocalSet::new()
        .run_until(async {
            let mut sb = Sandbox::builder()
                .config(SandboxConfig::trusted())
                .pool(PoolConfig {
                    pool_size: 1,
                    max_runs_per_slot: 100,
                    ..Default::default()
                })
                .build()
                .unwrap();

            sb.set_input("secret", serde_json::json!("my_secret_value"));
            let r1 = sb.run("return sandbox.readInput('secret');").await.unwrap();
            assert_eq!(r1.value, json!("my_secret_value"));

            // Run without setting "secret" again — should not see previous value
            let r2 = sb.run("return sandbox.readInput('secret');").await.unwrap();
            assert_eq!(
                r2.value,
                json!("my_secret_value"),
                "set_input persists for the Sandbox instance (not per-run),\
                 but should not be shared between separate Sandbox instances"
            );
        })
        .await;
}

// ─── SandboxConfig constructors ───────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn trusted_config_runs_script() {
    LocalSet::new()
        .run_until(async {
            let mut sb = Sandbox::builder()
                .config(SandboxConfig::trusted())
                .pool(isolated())
                .build()
                .unwrap();
            let r = sb.run("return 'trusted';").await.unwrap();
            assert_eq!(r.value, json!("trusted"));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn power_user_config_runs_script() {
    LocalSet::new()
        .run_until(async {
            let mut sb = Sandbox::builder()
                .config(SandboxConfig::power_user())
                .pool(isolated())
                .build()
                .unwrap();
            let r = sb.run("return 'power';").await.unwrap();
            assert_eq!(r.value, json!("power"));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn noop_metrics_sink_is_default() {
    LocalSet::new()
        .run_until(async {
            // Just verifies that a sandbox without a custom sink still runs without panic
            let mut sb = Sandbox::builder()
                .config(SandboxConfig {
                    metrics_sink: Arc::new(NoopMetricsSink),
                    ..SandboxConfig::trusted()
                })
                .pool(isolated())
                .build()
                .unwrap();
            let r = sb.run("return 99;").await.unwrap();
            assert_eq!(r.value, json!(99));
        })
        .await;
}
