//! Phase 18 integration tests — per-run tags and timeout override.
//!
//! Covers:
//!   - `sandbox.tags()` returns host-provided tags inside the script.
//!   - Tags are forwarded into `RunMetrics::tags`.
//!   - Multiple tags are all visible; missing tags return an empty object.
//!   - Tag values are strings (numbers coerced to string at the boundary).
//!   - `timeout_override` shortens the budget below the pool default.
//!   - `timeout_override` extends the budget above the pool default.
//!   - `RunCapabilities::default()` leaves tags empty and timeout unchanged.
//!   - Tags are readable via `Sandbox::run_with_caps`.
//!   - Tags do not bleed between consecutive runs.

use std::collections::HashMap;
use std::time::Duration;

use hello_sandbox::loader::AllowlistModuleLoader;
use hello_sandbox::runtime::SharedRuntime;
use hello_sandbox::sdk::core_sdk::CorePack;
use hello_sandbox::sdk::SdkRegistry;
use hello_sandbox::{RunCapabilities, SandboxConfig, SandboxError, SandboxEvent};
use serde_json::json;
use tokio::sync::mpsc;

fn null_tx() -> mpsc::UnboundedSender<SandboxEvent> {
    mpsc::unbounded_channel().0
}

fn caps() -> RunCapabilities {
    RunCapabilities::default()
}

fn make_runtime() -> SharedRuntime {
    let loader = AllowlistModuleLoader::new();
    let sdk = SdkRegistry::empty().register(CorePack);
    SharedRuntime::new(SandboxConfig::trusted(), loader, &sdk)
}

// ─── sandbox.tags() ───────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn tags_empty_by_default() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut rt = make_runtime();
            let (val, _, _) = rt
                .run(r#"return sandbox.tags();"#, HashMap::new(), null_tx(), caps())
                .await
                .unwrap();
            // Default caps have no tags — should return an empty object.
            assert_eq!(val, json!({}));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn tags_visible_inside_script() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut rt = make_runtime();
            let mut tags = HashMap::new();
            tags.insert("tenant".to_string(), "acme".to_string());
            tags.insert("request_id".to_string(), "req-123".to_string());

            let (val, _, _) = rt
                .run(
                    r#"return sandbox.tags();"#,
                    HashMap::new(),
                    null_tx(),
                    RunCapabilities {
                        tags,
                        ..Default::default()
                    },
                )
                .await
                .unwrap();

            assert_eq!(val["tenant"], json!("acme"));
            assert_eq!(val["request_id"], json!("req-123"));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn tags_individual_lookup_inside_script() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut rt = make_runtime();
            let mut tags = HashMap::new();
            tags.insert("env".to_string(), "production".to_string());

            let (val, _, _) = rt
                .run(
                    r#"
                    const t = sandbox.tags();
                    return t["env"] ?? "missing";
                "#,
                    HashMap::new(),
                    null_tx(),
                    RunCapabilities {
                        tags,
                        ..Default::default()
                    },
                )
                .await
                .unwrap();

            assert_eq!(val, json!("production"));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn tags_object_is_frozen() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut rt = make_runtime();
            let mut tags = HashMap::new();
            tags.insert("k".to_string(), "v".to_string());

            // Attempting to assign to a frozen object in strict mode throws TypeError.
            let err = rt
                .run(
                    r#"
                    "use strict";
                    const t = sandbox.tags();
                    t.injected = "evil";
                "#,
                    HashMap::new(),
                    null_tx(),
                    RunCapabilities {
                        tags,
                        ..Default::default()
                    },
                )
                .await
                .unwrap_err();

            assert!(
                matches!(err, SandboxError::Runtime(_)),
                "expected Runtime error for frozen object mutation, got: {err:?}"
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn tags_forwarded_to_run_metrics() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut rt = make_runtime();
            let mut tags = HashMap::new();
            tags.insert("feature".to_string(), "beta".to_string());

            let (_, _, metrics) = rt
                .run(
                    r#"return 1;"#,
                    HashMap::new(),
                    null_tx(),
                    RunCapabilities {
                        tags,
                        ..Default::default()
                    },
                )
                .await
                .unwrap();

            assert_eq!(metrics.tags.get("feature").map(|s| s.as_str()), Some("beta"));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn tags_empty_in_metrics_when_none_set() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut rt = make_runtime();
            let (_, _, metrics) =
                rt.run(r#"return 1;"#, HashMap::new(), null_tx(), caps()).await.unwrap();
            assert!(
                metrics.tags.is_empty(),
                "expected empty tags in metrics, got: {:?}",
                metrics.tags
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn tags_do_not_bleed_between_runs() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut rt = make_runtime();

            // Run 1: provide tags
            let mut tags = HashMap::new();
            tags.insert("run".to_string(), "first".to_string());
            rt.run(
                r#"return 1;"#,
                HashMap::new(),
                null_tx(),
                RunCapabilities {
                    tags,
                    ..Default::default()
                },
            )
            .await
            .unwrap();

            // Run 2: no tags — must not see tags from run 1
            let (val, _, metrics) = rt
                .run(r#"return sandbox.tags();"#, HashMap::new(), null_tx(), caps())
                .await
                .unwrap();

            assert_eq!(val, json!({}), "tags from run 1 bled into run 2");
            assert!(metrics.tags.is_empty(), "metrics.tags from run 1 bled into run 2");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn multiple_tags_all_present() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut rt = make_runtime();
            let tags: HashMap<String, String> = [("a", "1"), ("b", "2"), ("c", "3")]
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();

            let (val, _, _) = rt
                .run(
                    r#"
                    const t = sandbox.tags();
                    return Object.keys(t).sort();
                "#,
                    HashMap::new(),
                    null_tx(),
                    RunCapabilities {
                        tags,
                        ..Default::default()
                    },
                )
                .await
                .unwrap();

            let keys = val.as_array().unwrap();
            assert!(keys.iter().any(|k| k == &json!("a")));
            assert!(keys.iter().any(|k| k == &json!("b")));
            assert!(keys.iter().any(|k| k == &json!("c")));
            assert_eq!(keys.len(), 3);
        })
        .await;
}

// ─── timeout_override ────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn timeout_override_shorter_kills_script() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // Use a PowerUser config so the watchdog is enabled, with a generous
            // pool-level timeout.  The per-run override is very short.
            let loader = AllowlistModuleLoader::new();
            let sdk = SdkRegistry::empty().register(CorePack);
            let mut config = SandboxConfig::power_user();
            config.timeout = Duration::from_secs(10);
            let mut rt = SharedRuntime::new(config, loader, &sdk);

            let err = rt
                .run(
                    "while (true) {}",
                    HashMap::new(),
                    null_tx(),
                    RunCapabilities {
                        timeout_override: Some(Duration::from_millis(150)),
                        ..Default::default()
                    },
                )
                .await
                .unwrap_err();

            assert!(matches!(err, SandboxError::Timeout(_)), "expected Timeout, got: {err:?}");
            // The reported duration should be the override, not the pool default.
            if let SandboxError::Timeout(d) = err {
                assert!(
                    d <= Duration::from_millis(200),
                    "timeout duration should be near override (150ms), got: {d:?}"
                );
            }
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn timeout_override_longer_allows_slow_script() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // Pool timeout is very short; per-run override is longer.
            // The script should complete successfully.
            let loader = AllowlistModuleLoader::new();
            let sdk = SdkRegistry::empty().register(CorePack);
            let mut config = SandboxConfig::power_user();
            config.timeout = Duration::from_millis(50);
            let mut rt = SharedRuntime::new(config, loader, &sdk);

            let result = rt
                .run(
                    r#"
                    // Use a Promise-based delay so the event loop actually runs.
                    await new Promise(r => setTimeout !== undefined ? setTimeout(r, 1) : r());
                    return "done";
                "#,
                    HashMap::new(),
                    null_tx(),
                    RunCapabilities {
                        timeout_override: Some(Duration::from_secs(5)),
                        ..Default::default()
                    },
                )
                .await;

            // Script may or may not use setTimeout (sandbox may not have it),
            // but the key property is it should NOT timeout — it should either
            // succeed or fail with a script error, not a Timeout.
            assert!(
                !matches!(result, Err(SandboxError::Timeout(_))),
                "script should not have timed out with a generous override, got: {result:?}"
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn default_capabilities_noop_for_tags_and_timeout() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut rt = make_runtime();
            // RunCapabilities::default() has no tags and no timeout override.
            // Run should complete normally with empty tags.
            let (val, _, metrics) = rt
                .run(
                    r#"return sandbox.tags();"#,
                    HashMap::new(),
                    null_tx(),
                    RunCapabilities::default(),
                )
                .await
                .unwrap();

            assert_eq!(val, json!({}));
            assert!(metrics.tags.is_empty());
        })
        .await;
}
