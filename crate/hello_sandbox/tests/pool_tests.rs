//! Phase 6 integration tests for `RuntimePool`.
//!
//! All tests run inside a `tokio::task::LocalSet` because `RuntimePool`
//! (and the `SharedRuntime` slots it holds) are `!Send`.

use std::collections::HashMap;
use std::time::Duration;

use hello_sandbox::loader::AllowlistModuleLoader;
use hello_sandbox::pool::{PoolConfig, RuntimePool};
use hello_sandbox::sdk::core_sdk::CorePack;
use hello_sandbox::sdk::kv_sdk::KvPack;
use hello_sandbox::sdk::SdkRegistry;
use hello_sandbox::SandboxConfig;
use serde_json::{json, Value};
use tokio::task::LocalSet;

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn make_pool(pool_config: PoolConfig) -> RuntimePool {
    RuntimePool::new(
        pool_config,
        SandboxConfig::trusted(),
        AllowlistModuleLoader::new(),
        SdkRegistry::empty().register(CorePack),
    )
}

fn make_kv_pool(pool_config: PoolConfig) -> RuntimePool {
    RuntimePool::new(
        pool_config,
        SandboxConfig::trusted(),
        AllowlistModuleLoader::new(),
        SdkRegistry::empty().register(CorePack).register(KvPack::default()),
    )
}

// ─── 1. Sequential runs reuse the same slot (KV persists) ─────────────────────

#[tokio::test(flavor = "current_thread")]
async fn pool_single_slot_sequential_reuse() {
    LocalSet::new()
        .run_until(async {
            let pool = make_kv_pool(PoolConfig {
                pool_size: 1,
                max_runs_per_slot: 100,
                max_idle_duration: Duration::from_secs(60),
                fallback_to_isolated: true,
            });

            // Run 1: write to KV
            pool.run(
                r#"
                import { kv } from "sandbox:kv";
                await kv.set("x", 42);
            "#,
                HashMap::new(),
            )
            .await
            .unwrap();

            // Run 2: same slot → KV persists
            let r = pool
                .run(
                    r#"
                import { kv } from "sandbox:kv";
                return await kv.get("x");
            "#,
                    HashMap::new(),
                )
                .await
                .unwrap();
            assert_eq!(r.value, json!(42), "KV should persist within same slot");

            // Run 3: still same slot
            let r = pool
                .run(
                    r#"
                import { kv } from "sandbox:kv";
                return await kv.get("x");
            "#,
                    HashMap::new(),
                )
                .await
                .unwrap();
            assert_eq!(r.value, json!(42));

            let stats = pool.pool_stats();
            assert_eq!(stats.idle, 1);
            assert_eq!(stats.total_runs, 3);
        })
        .await;
}

// ─── 2. Slot is recycled once max_runs_per_slot is reached ────────────────────

#[tokio::test(flavor = "current_thread")]
async fn pool_recycles_after_max_runs() {
    LocalSet::new()
        .run_until(async {
            let pool = make_kv_pool(PoolConfig {
                pool_size: 1,
                max_runs_per_slot: 2,
                max_idle_duration: Duration::from_secs(60),
                fallback_to_isolated: true,
            });

            // Run 1: write KV (run_count becomes 1)
            pool.run(
                r#"
                import { kv } from "sandbox:kv";
                await kv.set("counter", 99);
            "#,
                HashMap::new(),
            )
            .await
            .unwrap();

            // Run 2: same slot (run_count = 2 == max → slot marked Stale after checkin)
            let r = pool
                .run(
                    r#"
                import { kv } from "sandbox:kv";
                return await kv.get("counter");
            "#,
                    HashMap::new(),
                )
                .await
                .unwrap();
            assert_eq!(r.value, json!(99), "Should see value from run 1");

            // Run 3: stale slot is recycled → fresh KV store
            let r = pool
                .run(
                    r#"
                import { kv } from "sandbox:kv";
                return await kv.get("counter");
            "#,
                    HashMap::new(),
                )
                .await
                .unwrap();
            assert_eq!(r.value, Value::Null, "New slot after recycle: KV must be empty");

            assert_eq!(pool.pool_stats().total_runs, 3);
        })
        .await;
}

// ─── 3. Slot is recycled after idle timeout ────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn pool_recycles_after_idle_timeout() {
    LocalSet::new()
        .run_until(async {
            let pool = make_kv_pool(PoolConfig {
                pool_size: 1,
                max_runs_per_slot: 100,
                max_idle_duration: Duration::from_millis(50),
                fallback_to_isolated: true,
            });

            // Run 1: write KV
            pool.run(
                r#"
                import { kv } from "sandbox:kv";
                await kv.set("val", "hello");
            "#,
                HashMap::new(),
            )
            .await
            .unwrap();

            // Wait past the idle timeout
            tokio::time::sleep(Duration::from_millis(100)).await;

            // Run 2: slot should be recycled (idle too long) → KV cleared
            let r = pool
                .run(
                    r#"
                import { kv } from "sandbox:kv";
                return await kv.get("val");
            "#,
                    HashMap::new(),
                )
                .await
                .unwrap();
            assert_eq!(r.value, Value::Null, "KV must be empty after idle recycle");
        })
        .await;
}

// ─── 4. pool_size=0 always creates isolated runtimes ─────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn pool_size_zero_always_isolated() {
    LocalSet::new()
        .run_until(async {
            let pool = make_kv_pool(PoolConfig {
                pool_size: 0,
                ..Default::default()
            });

            // No warm slots
            let stats = pool.pool_stats();
            assert_eq!(stats.idle, 0);
            assert_eq!(stats.checked_out, 0);
            assert_eq!(stats.stale, 0);

            // Run 1: set KV in isolated runtime
            pool.run(
                r#"
                import { kv } from "sandbox:kv";
                await kv.set("key", "isolated");
            "#,
                HashMap::new(),
            )
            .await
            .unwrap();

            // Run 2: different isolated runtime → KV is empty
            let r = pool
                .run(
                    r#"
                import { kv } from "sandbox:kv";
                return await kv.get("key");
            "#,
                    HashMap::new(),
                )
                .await
                .unwrap();
            assert_eq!(r.value, Value::Null, "Isolated runtimes must not share state");

            assert_eq!(pool.pool_stats().total_runs, 2);
        })
        .await;
}

// ─── 5. Error marks slot Stale; next run gets fresh slot ─────────────────────

#[tokio::test(flavor = "current_thread")]
async fn pool_error_marks_slot_stale_and_next_run_succeeds() {
    LocalSet::new()
        .run_until(async {
            let pool = make_pool(PoolConfig {
                pool_size: 1,
                max_runs_per_slot: 100,
                max_idle_duration: Duration::from_secs(60),
                fallback_to_isolated: true,
            });

            // Run 1: throws → slot becomes Stale
            let err = pool.run(r#"throw new Error("boom");"#, HashMap::new()).await;
            assert!(err.is_err(), "Should have errored");

            // Stale slot recycled on next checkout → new run must succeed
            let r = pool.run("return 42;", HashMap::new()).await.unwrap();
            assert_eq!(r.value, json!(42), "Fresh slot after stale recycle must work");
        })
        .await;
}

// ─── 6. KV is cleared when slot is recycled (max_runs_per_slot=1) ─────────────

#[tokio::test(flavor = "current_thread")]
async fn pool_kv_cleared_after_slot_recycle() {
    LocalSet::new()
        .run_until(async {
            let pool = make_kv_pool(PoolConfig {
                pool_size: 1,
                max_runs_per_slot: 1,
                max_idle_duration: Duration::from_secs(60),
                fallback_to_isolated: true,
            });

            // Run 1 (max_runs=1 → slot marked Stale after checkin): set KV
            pool.run(
                r#"
                import { kv } from "sandbox:kv";
                await kv.set("data", "present");
            "#,
                HashMap::new(),
            )
            .await
            .unwrap();

            // Run 2: fresh slot (recycled) → KV is empty
            let r = pool
                .run(
                    r#"
                import { kv } from "sandbox:kv";
                return await kv.get("data");
            "#,
                    HashMap::new(),
                )
                .await
                .unwrap();
            assert_eq!(r.value, Value::Null, "KV cleared after slot recycle");
        })
        .await;
}

// ─── 7. Pool stats track correctly across successes and errors ────────────────
//
// Note: only pool_size=1 is used here because V8 (via deno_core) maintains
// thread-local isolate state and cannot have two JsRuntime objects simultaneously
// active on the same OS thread. A pool on a LocalSet is inherently single-slot.

#[tokio::test(flavor = "current_thread")]
async fn pool_stats_track_correctly() {
    LocalSet::new()
        .run_until(async {
            let pool = make_pool(PoolConfig {
                pool_size: 1,
                max_runs_per_slot: 100,
                max_idle_duration: Duration::from_secs(60),
                fallback_to_isolated: true,
            });

            // Initially: 1 idle slot
            let stats = pool.pool_stats();
            assert_eq!(stats.idle, 1);
            assert_eq!(stats.checked_out, 0);
            assert_eq!(stats.stale, 0);
            assert_eq!(stats.total_runs, 0);

            // After 2 successful runs
            pool.run("return 1;", HashMap::new()).await.unwrap();
            pool.run("return 2;", HashMap::new()).await.unwrap();
            let stats = pool.pool_stats();
            assert_eq!(stats.idle, 1);
            assert_eq!(stats.total_runs, 2);

            // After 1 error run: slot becomes Stale (until next checkout recycles it)
            let _ = pool.run("throw new Error('x');", HashMap::new()).await;
            let stats = pool.pool_stats();
            // The 1 slot should now be stale
            assert_eq!(stats.stale, 1);
            assert_eq!(stats.idle, 0);
            assert_eq!(stats.total_runs, 3);

            // Next run recycles the stale slot → fresh idle slot
            pool.run("return 99;", HashMap::new()).await.unwrap();
            let stats = pool.pool_stats();
            assert_eq!(stats.idle, 1);
            assert_eq!(stats.stale, 0);
            assert_eq!(stats.total_runs, 4);
        })
        .await;
}

// ─── 8. Return value and logs are propagated correctly ────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn pool_return_value_and_logs_propagated() {
    LocalSet::new()
        .run_until(async {
            let pool = make_pool(PoolConfig {
                pool_size: 1,
                ..Default::default()
            });

            let r = pool
                .run(
                    r#"
                console.log("hello from pool");
                return { answer: 42 };
            "#,
                    HashMap::new(),
                )
                .await
                .unwrap();
            assert_eq!(r.value, json!({ "answer": 42 }));
            assert_eq!(r.logs, vec!["hello from pool"]);
        })
        .await;
}

// ─── 9. Inputs are forwarded to the script ────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn pool_inputs_forwarded_to_script() {
    LocalSet::new()
        .run_until(async {
            let pool = make_pool(PoolConfig {
                pool_size: 1,
                ..Default::default()
            });

            let mut inputs = HashMap::new();
            inputs.insert("name".to_string(), json!("Alice"));

            let r = pool.run(r#"return sandbox.readInput("name");"#, inputs).await.unwrap();
            assert_eq!(r.value, json!("Alice"));
        })
        .await;
}

// ─── 10. fallback_to_isolated=false yields until a slot is free ───────────────

#[tokio::test(flavor = "current_thread")]
async fn pool_no_fallback_blocks_until_slot_free() {
    // With pool_size=1, fallback_to_isolated=false, and a single-threaded
    // LocalSet, two spawned tasks interleave via await points. The second
    // task yields until the first completes and returns the slot.
    LocalSet::new()
        .run_until(async {
            let pool = std::rc::Rc::new(make_pool(PoolConfig {
                pool_size: 1,
                max_runs_per_slot: 100,
                max_idle_duration: Duration::from_secs(60),
                fallback_to_isolated: false,
            }));

            let pool2 = pool.clone();

            let t1 = tokio::task::spawn_local({
                let p = pool.clone();
                async move { p.run("return 1;", HashMap::new()).await }
            });
            let t2 = tokio::task::spawn_local({
                let p = pool2.clone();
                async move { p.run("return 2;", HashMap::new()).await }
            });

            let (r1, r2) = tokio::join!(t1, t2);
            assert_eq!(r1.unwrap().unwrap().value, json!(1));
            assert_eq!(r2.unwrap().unwrap().value, json!(2));
            assert_eq!(pool.pool_stats().total_runs, 2);
        })
        .await;
}
