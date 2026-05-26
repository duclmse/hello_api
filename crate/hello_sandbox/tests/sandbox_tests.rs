//! Integration tests for `Sandbox` and `SandboxBuilder` — Phase 7.
//!
//! All tests must run inside a `tokio::task::LocalSet` because V8 is `!Send`.
//!
//! **Important:** Every test uses `pool_size=1` (or 0). Creating more than one
//! V8 isolate on the same OS thread causes a fatal V8 handle-scope crash.

use hello_sandbox::{PoolConfig, Sandbox, SandboxConfig, SandboxError};
use serde_json::json;
use tokio::task::LocalSet;

// ── Single-isolate pool config for all tests ─────────────────────────────────

fn single_slot_pool() -> PoolConfig {
    PoolConfig {
        pool_size: 1,
        max_runs_per_slot: 100,
        ..Default::default()
    }
}

/// Helper: `Sandbox::new()` but forced to pool_size=1 for test safety.
fn sandbox_new(config: SandboxConfig) -> Sandbox {
    Sandbox::builder().config(config).pool(single_slot_pool()).build().unwrap()
}

// ── Basic Sandbox::new() ─────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn sandbox_runs_script() {
    LocalSet::new()
        .run_until(async {
            let mut sb = sandbox_new(SandboxConfig::trusted());
            let result = sb.run("return 1 + 2").await.unwrap();
            assert_eq!(result.value, json!(3));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn sandbox_set_input_visible_to_script() {
    LocalSet::new()
        .run_until(async {
            let mut sb = sandbox_new(SandboxConfig::trusted());
            sb.set_input("x", json!(42));
            let result = sb.run("return sandbox.readInput('x') * 2").await.unwrap();
            assert_eq!(result.value, json!(84));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn sandbox_set_input_updated_between_runs() {
    LocalSet::new()
        .run_until(async {
            let mut sb = sandbox_new(SandboxConfig::trusted());
            sb.set_input("n", json!(1));
            let r1 = sb.run("return sandbox.readInput('n')").await.unwrap();
            assert_eq!(r1.value, json!(1));

            sb.set_input("n", json!(99));
            let r2 = sb.run("return sandbox.readInput('n')").await.unwrap();
            assert_eq!(r2.value, json!(99));
        })
        .await;
}

// ── register_module() ────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn sandbox_register_module_importable_from_script() {
    LocalSet::new()
        .run_until(async {
            let mut sb = sandbox_new(SandboxConfig::trusted());
            sb.register_module("sandbox:math", "export const double = x => x * 2;");

            let result = sb
                .run(
                    r#"
                    import { double } from "sandbox:math";
                    return double(21);
                "#,
                )
                .await
                .unwrap();
            assert_eq!(result.value, json!(42));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn sandbox_register_typescript_module() {
    LocalSet::new()
        .run_until(async {
            let mut sb = sandbox_new(SandboxConfig::trusted());
            sb.register_module(
                "sandbox:greet",
                "export const greet = (name: string): string => `Hello, ${name}!`;",
            );

            let result = sb
                .run(
                    r#"
                    import { greet } from "sandbox:greet";
                    return greet("World");
                "#,
                )
                .await
                .unwrap();
            assert_eq!(result.value, json!("Hello, World!"));
        })
        .await;
}

// ── Logs and events ──────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn sandbox_logs_captured() {
    LocalSet::new()
        .run_until(async {
            let mut sb = sandbox_new(SandboxConfig::trusted());
            let result = sb
                .run(
                    r#"
                    console.log("hello");
                    console.log("world");
                    return null;
                "#,
                )
                .await
                .unwrap();
            assert_eq!(result.logs, vec!["hello".to_string(), "world".to_string()]);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn sandbox_events_collected() {
    LocalSet::new()
        .run_until(async {
            let mut sb = sandbox_new(SandboxConfig::trusted());
            let result = sb
                .run(
                    r#"
                    sandbox.emit("ping", { ok: true });
                    return null;
                "#,
                )
                .await
                .unwrap();
            assert_eq!(result.events.len(), 1);
            assert_eq!(result.events[0].name, "ping");
        })
        .await;
}

// ── TypeScript ───────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn sandbox_typescript_transpiled() {
    LocalSet::new()
        .run_until(async {
            let mut sb = sandbox_new(SandboxConfig::trusted());
            let result = sb
                .run(
                    r#"
                    const greet = (name: string): string => `Hi, ${name}`;
                    return greet("TS");
                "#,
                )
                .await
                .unwrap();
            assert_eq!(result.value, json!("Hi, TS"));
        })
        .await;
}

// ── pool_stats() ─────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn pool_stats_zero_before_first_run() {
    LocalSet::new()
        .run_until(async {
            let sb = sandbox_new(SandboxConfig::trusted());
            let stats = sb.pool_stats();
            assert_eq!(stats.total_runs, 0);
            assert_eq!(stats.idle, 0);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn pool_stats_after_runs() {
    LocalSet::new()
        .run_until(async {
            let mut sb = Sandbox::builder()
                .config(SandboxConfig::trusted())
                .pool(single_slot_pool())
                .build()
                .unwrap();

            sb.run("return 1").await.unwrap();
            sb.run("return 2").await.unwrap();

            let stats = sb.pool_stats();
            assert_eq!(stats.total_runs, 2);
            assert_eq!(stats.idle, 1);
            assert_eq!(stats.checked_out, 0);
        })
        .await;
}

// ── SandboxBuilder ───────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn builder_with_pool_config() {
    LocalSet::new()
        .run_until(async {
            let mut sb = Sandbox::builder()
                .config(SandboxConfig::trusted())
                .pool(PoolConfig {
                    pool_size: 1,
                    max_runs_per_slot: 5,
                    ..Default::default()
                })
                .build()
                .unwrap();

            let result = sb.run("return 'ok'").await.unwrap();
            assert_eq!(result.value, json!("ok"));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn builder_module_and_input() {
    LocalSet::new()
        .run_until(async {
            let mut sb = Sandbox::builder()
                .config(SandboxConfig::trusted())
                .pool(single_slot_pool())
                .module("sandbox:util", "export const inc = n => n + 1;")
                .input("start", json!(10))
                .build()
                .unwrap();

            let result = sb
                .run(
                    r#"
                    import { inc } from "sandbox:util";
                    return inc(sandbox.readInput("start"));
                "#,
                )
                .await
                .unwrap();
            assert_eq!(result.value, json!(11));
        })
        .await;
}

// ── Error handling ───────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn sandbox_script_error_propagated() {
    LocalSet::new()
        .run_until(async {
            let mut sb = sandbox_new(SandboxConfig::trusted());
            let err = sb.run("throw new Error('boom')").await.unwrap_err();
            assert!(matches!(err, SandboxError::Runtime(_)), "expected Runtime error, got {err:?}");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn sandbox_timeout_fires() {
    LocalSet::new()
        .run_until(async {
            use std::time::Duration;

            let mut config = SandboxConfig::power_user();
            config.timeout = Duration::from_millis(200);

            let mut sb =
                Sandbox::builder().config(config).pool(single_slot_pool()).build().unwrap();

            let err = sb.run("while (true) {}").await.unwrap_err();
            assert!(matches!(err, SandboxError::Timeout(_)), "expected Timeout, got {err:?}");
        })
        .await;
}

// ── Multi-run scope isolation ─────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn sandbox_runs_have_isolated_scopes() {
    LocalSet::new()
        .run_until(async {
            let mut sb = Sandbox::builder()
                .config(SandboxConfig::trusted())
                .pool(single_slot_pool())
                .build()
                .unwrap();

            // Run 1 sets a module-level variable.
            sb.run("const MY_STATE = 'run_one'; return MY_STATE").await.unwrap();

            // Run 2 should NOT see MY_STATE — each run is a new module evaluation.
            let r = sb
                .run("return typeof MY_STATE === 'undefined' ? 'clean' : 'polluted'")
                .await
                .unwrap();
            assert_eq!(
                r.value,
                json!("clean"),
                "run 2 should not see module-level vars from run 1"
            );
        })
        .await;
}

// ── Pool size 0 (always isolated) ─────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn sandbox_pool_size_zero_always_isolated() {
    LocalSet::new()
        .run_until(async {
            let mut sb = Sandbox::builder()
                .config(SandboxConfig::trusted())
                .pool(PoolConfig::high_isolation())
                .build()
                .unwrap();

            let result = sb.run("return 'isolated'").await.unwrap();
            assert_eq!(result.value, json!("isolated"));

            let stats = sb.pool_stats();
            assert_eq!(stats.idle, 0);
            assert_eq!(stats.total_runs, 1);
        })
        .await;
}
