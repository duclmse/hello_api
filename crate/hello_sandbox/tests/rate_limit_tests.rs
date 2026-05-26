//! Phase 13 — Per-Run Rate Limiting integration tests.
//!
//! Verifies:
//! - `emit_calls_per_run` limit fires on the (limit+1)th call.
//! - `kv_ops_per_run` limit fires on the (limit+1)th KV operation.
//! - `http_calls_per_run` limit fires before the allowlist check.
//! - No limit = unlimited (control tests pass with `None`).
//! - `RunMetrics` includes consumed call counters.
//! - The exact `SandboxError::RateLimitExceeded { resource, limit }` is returned.
//!
//! All tests use `pool_size = 1` (V8 single-thread constraint).

use std::collections::HashMap;

use hello_sandbox::sdk::core_sdk::CorePack;
use hello_sandbox::sdk::http_sdk::{HttpConfig, HttpPack};
use hello_sandbox::sdk::kv_sdk::KvPack;
use hello_sandbox::sdk::SdkRegistry;
use hello_sandbox::{PoolConfig, RateLimitConfig, RuntimePool, SandboxConfig, SandboxError};
use serde_json::json;
use tokio::task::LocalSet;

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn one_slot() -> PoolConfig {
    PoolConfig {
        pool_size: 1,
        ..PoolConfig::default()
    }
}

fn run_sync<F, T>(f: F) -> T
where
    F: std::future::Future<Output = T>,
{
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let local = LocalSet::new();
    local.block_on(&rt, f)
}

// ─── Emit rate limit ──────────────────────────────────────────────────────────

#[test]
fn emit_limit_zero_blocks_first_call() {
    run_sync(async {
        let config = SandboxConfig {
            rate_limits: RateLimitConfig {
                emit_calls_per_run: Some(0),
                ..Default::default()
            },
            ..SandboxConfig::trusted()
        };
        let sdk = SdkRegistry::empty().register(CorePack);
        let pool = RuntimePool::new(
            one_slot(),
            config,
            hello_sandbox::loader::AllowlistModuleLoaderBuilder::default(),
            sdk,
        );

        let err =
            pool.run(r#"sandbox.emit("e", {}); return "ok";"#, HashMap::new()).await.unwrap_err();

        match err {
            SandboxError::RateLimitExceeded { resource, limit } => {
                assert_eq!(resource, "emit");
                assert_eq!(limit, 0);
            },
            other => panic!("expected RateLimitExceeded, got: {other:?}"),
        }
    });
}

#[test]
fn emit_limit_one_allows_first_blocks_second() {
    run_sync(async {
        let config = SandboxConfig {
            rate_limits: RateLimitConfig {
                emit_calls_per_run: Some(1),
                ..Default::default()
            },
            ..SandboxConfig::trusted()
        };
        let sdk = SdkRegistry::empty().register(CorePack);
        let pool = RuntimePool::new(
            one_slot(),
            config,
            hello_sandbox::loader::AllowlistModuleLoaderBuilder::default(),
            sdk,
        );

        // First call succeeds (emit #1 is within limit).
        let ok = pool.run(r#"sandbox.emit("e", {}); return 1;"#, HashMap::new()).await.unwrap();
        assert_eq!(ok.value, json!(1));

        // Second run: two emits — second should exceed the limit.
        let err = pool
            .run(r#"sandbox.emit("a", {}); sandbox.emit("b", {}); return 2;"#, HashMap::new())
            .await
            .unwrap_err();

        match err {
            SandboxError::RateLimitExceeded { resource, limit } => {
                assert_eq!(resource, "emit");
                assert_eq!(limit, 1);
            },
            other => panic!("expected RateLimitExceeded, got: {other:?}"),
        }
    });
}

#[test]
fn emit_no_limit_allows_many_calls() {
    run_sync(async {
        // Default: no rate limit (None).
        let sdk = SdkRegistry::empty().register(CorePack);
        let pool = RuntimePool::new(
            one_slot(),
            SandboxConfig::trusted(),
            hello_sandbox::loader::AllowlistModuleLoaderBuilder::default(),
            sdk,
        );

        // 10 emits — should all succeed.
        let result = pool
            .run(
                r#"
                for (let i = 0; i < 10; i++) sandbox.emit("e", { i });
                return "ok";
                "#,
                HashMap::new(),
            )
            .await
            .unwrap();
        assert_eq!(result.value, json!("ok"));
        assert_eq!(result.metrics.emit_calls, 10);
    });
}

// ─── KV rate limit ────────────────────────────────────────────────────────────

#[test]
fn kv_limit_zero_blocks_first_op() {
    run_sync(async {
        let config = SandboxConfig {
            rate_limits: RateLimitConfig {
                kv_ops_per_run: Some(0),
                ..Default::default()
            },
            ..SandboxConfig::trusted()
        };
        let sdk = SdkRegistry::empty().register(CorePack).register(KvPack::new());
        let pool = RuntimePool::new(
            one_slot(),
            config,
            hello_sandbox::loader::AllowlistModuleLoaderBuilder::default(),
            sdk,
        );

        let err = pool
            .run(
                r#"
                import { kv } from "sandbox:kv";
                await kv.set("x", 1);
                return "ok";
                "#,
                HashMap::new(),
            )
            .await
            .unwrap_err();

        match err {
            SandboxError::RateLimitExceeded { resource, limit } => {
                assert_eq!(resource, "kv");
                assert_eq!(limit, 0);
            },
            other => panic!("expected RateLimitExceeded, got: {other:?}"),
        }
    });
}

#[test]
fn kv_limit_allows_exactly_n_ops() {
    run_sync(async {
        let config = SandboxConfig {
            rate_limits: RateLimitConfig {
                kv_ops_per_run: Some(2),
                ..Default::default()
            },
            ..SandboxConfig::trusted()
        };
        let sdk = SdkRegistry::empty().register(CorePack).register(KvPack::new());
        let pool = RuntimePool::new(
            one_slot(),
            config,
            hello_sandbox::loader::AllowlistModuleLoaderBuilder::default(),
            sdk,
        );

        // 2 ops — exactly at the limit, should succeed.
        let ok = pool
            .run(
                r#"
                import { kv } from "sandbox:kv";
                await kv.set("a", 1);
                await kv.set("b", 2);
                return "ok";
                "#,
                HashMap::new(),
            )
            .await
            .unwrap();
        assert_eq!(ok.value, json!("ok"));
        assert_eq!(ok.metrics.kv_ops, 2);
    });
}

#[test]
fn kv_limit_blocks_on_n_plus_one() {
    run_sync(async {
        let config = SandboxConfig {
            rate_limits: RateLimitConfig {
                kv_ops_per_run: Some(2),
                ..Default::default()
            },
            ..SandboxConfig::trusted()
        };
        let sdk = SdkRegistry::empty().register(CorePack).register(KvPack::new());
        let pool = RuntimePool::new(
            one_slot(),
            config,
            hello_sandbox::loader::AllowlistModuleLoaderBuilder::default(),
            sdk,
        );

        // 3 ops — one over the limit.
        let err = pool
            .run(
                r#"
                import { kv } from "sandbox:kv";
                await kv.set("a", 1);
                await kv.set("b", 2);
                await kv.get("a");
                return "ok";
                "#,
                HashMap::new(),
            )
            .await
            .unwrap_err();

        match err {
            SandboxError::RateLimitExceeded { resource, limit } => {
                assert_eq!(resource, "kv");
                assert_eq!(limit, 2);
            },
            other => panic!("expected RateLimitExceeded, got: {other:?}"),
        }
    });
}

// ─── HTTP rate limit ──────────────────────────────────────────────────────────

#[test]
fn http_limit_zero_blocks_before_allowlist_check() {
    // With http_calls_per_run: Some(0), the rate limit fires on the first call
    // BEFORE the allowlist check — so the error is RateLimitExceeded, not an
    // allowlist error, even with an empty allowlist.
    run_sync(async {
        let config = SandboxConfig {
            rate_limits: RateLimitConfig {
                http_calls_per_run: Some(0),
                ..Default::default()
            },
            ..SandboxConfig::trusted()
        };
        let http_pack = HttpPack::new(HttpConfig::default()); // empty allowlist
        let sdk = SdkRegistry::empty().register(CorePack).register(http_pack);
        let pool = RuntimePool::new(
            one_slot(),
            config,
            hello_sandbox::loader::AllowlistModuleLoaderBuilder::default(),
            sdk,
        );

        let err = pool
            .run(
                r#"
                import { fetch } from "sandbox:http";
                await fetch("https://example.com");
                return "ok";
                "#,
                HashMap::new(),
            )
            .await
            .unwrap_err();

        match err {
            SandboxError::RateLimitExceeded { resource, limit } => {
                assert_eq!(resource, "http");
                assert_eq!(limit, 0);
            },
            other => panic!("expected RateLimitExceeded, got: {other:?}"),
        }
    });
}

// ─── Metrics counters ─────────────────────────────────────────────────────────

#[test]
fn metrics_include_call_counters() {
    run_sync(async {
        let sdk = SdkRegistry::empty().register(CorePack).register(KvPack::new());
        let pool = RuntimePool::new(
            one_slot(),
            SandboxConfig::trusted(),
            hello_sandbox::loader::AllowlistModuleLoaderBuilder::default(),
            sdk,
        );

        let result = pool
            .run(
                r#"
                import { kv } from "sandbox:kv";
                await kv.set("x", 1);
                await kv.get("x");
                sandbox.emit("e", {});
                return "ok";
                "#,
                HashMap::new(),
            )
            .await
            .unwrap();

        assert_eq!(result.metrics.kv_ops, 2);
        assert_eq!(result.metrics.emit_calls, 1);
        assert_eq!(result.metrics.http_calls, 0);
    });
}

#[test]
fn metrics_zero_when_no_ops_called() {
    run_sync(async {
        let sdk = SdkRegistry::empty().register(CorePack);
        let pool = RuntimePool::new(
            one_slot(),
            SandboxConfig::trusted(),
            hello_sandbox::loader::AllowlistModuleLoaderBuilder::default(),
            sdk,
        );

        let result = pool.run("return 42;", HashMap::new()).await.unwrap();
        assert_eq!(result.metrics.http_calls, 0);
        assert_eq!(result.metrics.kv_ops, 0);
        assert_eq!(result.metrics.emit_calls, 0);
    });
}

// ─── Rate limit resets between runs ───────────────────────────────────────────

#[test]
fn rate_limit_resets_between_runs() {
    // With a limit of 1, two sequential runs each making 1 call should both succeed.
    run_sync(async {
        let config = SandboxConfig {
            rate_limits: RateLimitConfig {
                emit_calls_per_run: Some(1),
                ..Default::default()
            },
            ..SandboxConfig::trusted()
        };
        let sdk = SdkRegistry::empty().register(CorePack);
        let pool = RuntimePool::new(
            one_slot(),
            config,
            hello_sandbox::loader::AllowlistModuleLoaderBuilder::default(),
            sdk,
        );

        // Run 1: 1 emit (within limit).
        pool.run(r#"sandbox.emit("e", {}); return 1;"#, HashMap::new()).await.unwrap();

        // Run 2: 1 emit again (counter reset, within limit).
        let r2 = pool.run(r#"sandbox.emit("e", {}); return 2;"#, HashMap::new()).await.unwrap();

        assert_eq!(r2.value, json!(2));
        assert_eq!(r2.metrics.emit_calls, 1);
    });
}
