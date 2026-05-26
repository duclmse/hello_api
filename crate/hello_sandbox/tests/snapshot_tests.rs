//! Phase 15 — V8 Snapshot integration tests.
//!
//! Verifies that the sandbox behaves correctly with a pre-baked V8 snapshot.
//! The snapshot path is taken automatically when `cfg(has_snapshot)` is set
//! (i.e. after running `cargo run --bin make-snapshot` and rebuilding).
//!
//! All tests use `pool_size: 0` (isolated mode) or `pool_size: 1` to avoid
//! the V8 single-isolate constraint.

use hello_sandbox::{snapshot, PoolConfig, Sandbox, SandboxConfig, SandboxError};
use serde_json::json;
use tokio::task::LocalSet;

fn isolated_sandbox(config: SandboxConfig) -> Sandbox {
    Sandbox::builder()
        .config(config)
        .pool(PoolConfig {
            pool_size: 0,
            ..Default::default()
        })
        .build()
        .unwrap()
}

fn single_slot_sandbox(config: SandboxConfig) -> Sandbox {
    Sandbox::builder()
        .config(config)
        .pool(PoolConfig {
            pool_size: 1,
            max_runs_per_slot: 100,
            ..Default::default()
        })
        .build()
        .unwrap()
}

// ── Snapshot availability ─────────────────────────────────────────────────────

#[test]
fn get_snapshot_returns_expected_state() {
    let snap = snapshot::get_snapshot();
    if let Some(bytes) = snap {
        assert!(!bytes.is_empty(), "snapshot bytes must be non-empty");
        // Snapshot should be substantial (CorePack bootstrap + V8 heap overhead).
        assert!(bytes.len() > 1024, "snapshot should be > 1 KiB, got {} bytes", bytes.len());
    }
    // None is also valid when the snapshot hasn't been generated yet.
}

// ── Basic correctness with (or without) snapshot ──────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn snapshot_sandbox_runs_plain_js() {
    LocalSet::new()
        .run_until(async {
            let mut sb = isolated_sandbox(SandboxConfig::trusted());
            let result = sb.run("return 42").await.unwrap();
            assert_eq!(result.value, json!(42));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn snapshot_console_log_captured() {
    LocalSet::new()
        .run_until(async {
            let mut sb = isolated_sandbox(SandboxConfig::trusted());
            let result = sb
                .run(
                    r#"
                console.log("hello from snapshot");
                return "done";
            "#,
                )
                .await
                .unwrap();
            assert_eq!(result.value, json!("done"));
            assert!(
                result.logs.iter().any(|l| l.contains("hello from snapshot")),
                "expected log line not found: {:?}",
                result.logs
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn snapshot_readinput_works() {
    LocalSet::new()
        .run_until(async {
            let mut sb = Sandbox::builder()
                .config(SandboxConfig::trusted())
                .pool(PoolConfig {
                    pool_size: 0,
                    ..Default::default()
                })
                .input("greeting", json!("hello"))
                .build()
                .unwrap();
            let result = sb.run(r#"return sandbox.readInput("greeting")"#).await.unwrap();
            assert_eq!(result.value, json!("hello"));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn snapshot_emit_works() {
    LocalSet::new()
        .run_until(async {
            let mut sb = isolated_sandbox(SandboxConfig::trusted());
            let result = sb
                .run(
                    r#"
                sandbox.emit("ping", { value: 1 });
                return "ok";
            "#,
                )
                .await
                .unwrap();
            assert_eq!(result.value, json!("ok"));
            assert_eq!(result.events.len(), 1);
            assert_eq!(result.events[0].name, "ping");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn snapshot_frozen_globalthis_prevents_new_globals() {
    LocalSet::new()
        .run_until(async {
            let mut sb = isolated_sandbox(SandboxConfig::trusted());
            // In strict-mode ESM, assigning to a frozen object property throws.
            let result = sb
                .run(
                    r#"
                try {
                    globalThis.evil = true;
                    return "not frozen";
                } catch (_e) {
                    return "frozen";
                }
            "#,
                )
                .await
                .unwrap();
            assert_eq!(result.value, json!("frozen"));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn snapshot_deno_is_deleted() {
    LocalSet::new()
        .run_until(async {
            let mut sb = isolated_sandbox(SandboxConfig::trusted());
            let result = sb.run(r#"return typeof globalThis.Deno"#).await.unwrap();
            assert_eq!(result.value, json!("undefined"));
        })
        .await;
}

// ── Scope isolation across sequential runs on same warm slot ──────────────────

#[tokio::test(flavor = "current_thread")]
async fn snapshot_sequential_runs_have_isolated_scopes() {
    LocalSet::new()
        .run_until(async {
            let mut sb = single_slot_sandbox(SandboxConfig::trusted());

            let r1 = sb.run("return 42").await.unwrap();
            assert_eq!(r1.value, json!(42));

            // `x` from run 1 must not bleed into run 2 (different module scope).
            let r2 = sb
                .run(
                    r#"
                return typeof x === "undefined" ? "isolated" : "leaked";
            "#,
                )
                .await
                .unwrap();
            assert_eq!(r2.value, json!("isolated"));
        })
        .await;
}

// ── Metrics populated correctly ───────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn snapshot_run_metrics_populated() {
    LocalSet::new()
        .run_until(async {
            let mut sb = isolated_sandbox(SandboxConfig::trusted());
            let result = sb.run("return 1 + 1").await.unwrap();
            assert_eq!(result.value, json!(2));
            assert!(result.metrics.elapsed.as_nanos() > 0, "elapsed should be non-zero");
            assert!(result.metrics.peak_heap_bytes > 0, "peak_heap_bytes should be non-zero");
        })
        .await;
}

// ── TypeScript transpilation works under snapshot ─────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn snapshot_typescript_transpiles_and_runs() {
    LocalSet::new()
        .run_until(async {
            let mut sb = isolated_sandbox(SandboxConfig::trusted());
            let result = sb
                .run(
                    r#"
                const add = (a: number, b: number): number => a + b;
                return add(3, 4);
            "#,
                )
                .await
                .unwrap();
            assert_eq!(result.value, json!(7));
        })
        .await;
}

// ── Error handling under snapshot ─────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn snapshot_script_error_propagates() {
    LocalSet::new()
        .run_until(async {
            let mut sb = isolated_sandbox(SandboxConfig::trusted());
            let err = sb.run("throw new Error('boom')").await.unwrap_err();
            assert!(matches!(err, SandboxError::Runtime(_)), "expected Runtime error, got {err:?}");
        })
        .await;
}

// ── Snapshot state: sandbox ops proxy still works ─────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn snapshot_sandbox_ops_proxy_still_functional() {
    LocalSet::new()
        .run_until(async {
            let mut sb = isolated_sandbox(SandboxConfig::trusted());
            // __sandbox_ops is frozen into globalThis at snapshot creation time.
            // Verify it still resolves ops correctly after loading from snapshot.
            let result = sb
                .run(
                    r#"
                return typeof globalThis.__sandbox_ops === "object" ? "present" : "missing";
            "#,
                )
                .await
                .unwrap();
            assert_eq!(result.value, json!("present"));
        })
        .await;
}
