//! Phase 17 — Per-Run Capability Constraints integration tests.
//!
//! Verifies that `RunCapabilities` fields correctly narrow what a single script
//! execution may do, independently of the sandbox-level `SandboxConfig`:
//!
//! - KV namespace prefix transparently namespaces keys.
//! - `kv_enabled: Some(false)` blocks all KV operations.
//! - `kv_ops_limit` overrides the pool-level KV rate limit.
//! - `http_enabled: Some(false)` blocks all HTTP fetches.
//! - `http_allowed_prefixes` replaces the pool-level allowlist per-run.
//! - `http_allowed_methods` restricts the HTTP verb.
//! - `http_calls_limit` overrides the pool-level HTTP rate limit.
//! - `emit_enabled: Some(false)` silently drops all events.
//! - `emit_allowed_names` filters events by name (others silently dropped).
//! - `emit_calls_limit` overrides the pool-level emit rate limit.
//! - `RunCapabilities::default()` is a complete no-op (backward compatibility).
//! - Capabilities work correctly through `run_streaming_with_caps`.
//!
//! All tests use `pool_size = 1` (V8 single-thread constraint).

use std::collections::HashMap;

use hello_sandbox::sdk::core_sdk::CorePack;
use hello_sandbox::sdk::http_sdk::{HttpConfig, HttpPack};
use hello_sandbox::sdk::kv_sdk::KvPack;
use hello_sandbox::sdk::SdkRegistry;
use hello_sandbox::{PoolConfig, RunCapabilities, RuntimePool, SandboxConfig, SandboxError};
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

// ─── KV namespace prefix ──────────────────────────────────────────────────────

#[test]
fn kv_prefix_namespaces_stored_keys() {
    // Write key "x" under namespace prefix "user:1:", read it back under the
    // same namespace. The stored key in the backend is "user:1:x" but the
    // script only ever sees "x".
    // With pool_size=1 the same InMemoryKvBackend slot is reused across runs.
    run_sync(async {
        let sdk = SdkRegistry::empty().register(CorePack).register(KvPack::new());
        let pool = RuntimePool::new(
            one_slot(),
            SandboxConfig::trusted(),
            hello_sandbox::loader::AllowlistModuleLoaderBuilder::default(),
            sdk,
        );

        // Write "x" under namespace "user:1:".
        let r = pool
            .run_with_caps(
                r#"
                import { kv } from "sandbox:kv";
                await kv.set("x", 42);
                return "ok";
                "#,
                HashMap::new(),
                RunCapabilities {
                    kv_key_prefix: Some("user:1:".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(r.value, json!("ok"));

        // Read "x" under the same namespace — should see 42.
        let r2 = pool
            .run_with_caps(
                r#"
                import { kv } from "sandbox:kv";
                const v = await kv.get("x");
                return v;
                "#,
                HashMap::new(),
                RunCapabilities {
                    kv_key_prefix: Some("user:1:".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(r2.value, json!(42));

        // Read "x" under a DIFFERENT namespace — should see null (different key).
        let r3 = pool
            .run_with_caps(
                r#"
                import { kv } from "sandbox:kv";
                const v = await kv.get("x");
                return v;
                "#,
                HashMap::new(),
                RunCapabilities {
                    kv_key_prefix: Some("user:2:".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(r3.value, json!(null));
    });
}

#[test]
fn kv_prefix_strips_from_list_results() {
    // kv.list("") should return keys without the namespace prefix.
    // With pool_size=1 the same backend is reused across runs on the slot.
    run_sync(async {
        let sdk = SdkRegistry::empty().register(CorePack).register(KvPack::new());
        let pool = RuntimePool::new(
            one_slot(),
            SandboxConfig::trusted(),
            hello_sandbox::loader::AllowlistModuleLoaderBuilder::default(),
            sdk,
        );

        // Write two keys under the same namespace.
        pool.run_with_caps(
            r#"
            import { kv } from "sandbox:kv";
            await kv.set("a", 1);
            await kv.set("b", 2);
            return "ok";
            "#,
            HashMap::new(),
            RunCapabilities {
                kv_key_prefix: Some("ns:".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        // List should return the un-prefixed keys.
        let r = pool
            .run_with_caps(
                r#"
                import { kv } from "sandbox:kv";
                const keys = await kv.list("");
                keys.sort();
                return keys;
                "#,
                HashMap::new(),
                RunCapabilities {
                    kv_key_prefix: Some("ns:".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(r.value, json!(["a", "b"]));
    });
}

// ─── kv_enabled ───────────────────────────────────────────────────────────────

#[test]
fn kv_disabled_blocks_all_ops() {
    run_sync(async {
        let sdk = SdkRegistry::empty().register(CorePack).register(KvPack::new());
        let pool = RuntimePool::new(
            one_slot(),
            SandboxConfig::trusted(),
            hello_sandbox::loader::AllowlistModuleLoaderBuilder::default(),
            sdk,
        );

        let err = pool
            .run_with_caps(
                r#"
                import { kv } from "sandbox:kv";
                await kv.set("x", 1);
                return "ok";
                "#,
                HashMap::new(),
                RunCapabilities {
                    kv_enabled: Some(false),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();

        match err {
            SandboxError::Runtime(_) => {}, // "capability denied: kv" JS exception
            other => panic!("expected Runtime error, got: {other:?}"),
        }
    });
}

// ─── kv_ops_limit override ────────────────────────────────────────────────────

#[test]
fn kv_ops_limit_overrides_pool_level() {
    // Pool has no KV limit; run cap sets limit=1 → second op should fail.
    run_sync(async {
        let sdk = SdkRegistry::empty().register(CorePack).register(KvPack::new());
        let pool = RuntimePool::new(
            one_slot(),
            SandboxConfig::trusted(), // no pool-level kv limit
            hello_sandbox::loader::AllowlistModuleLoaderBuilder::default(),
            sdk,
        );

        let err = pool
            .run_with_caps(
                r#"
                import { kv } from "sandbox:kv";
                await kv.set("a", 1);
                await kv.set("b", 2);
                return "ok";
                "#,
                HashMap::new(),
                RunCapabilities {
                    kv_ops_limit: Some(1),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();

        match err {
            SandboxError::RateLimitExceeded { resource, limit } => {
                assert_eq!(resource, "kv");
                assert_eq!(limit, 1);
            },
            other => panic!("expected RateLimitExceeded, got: {other:?}"),
        }
    });
}

// ─── http_enabled ─────────────────────────────────────────────────────────────

#[test]
fn http_disabled_blocks_all_fetches() {
    run_sync(async {
        let http_pack = HttpPack::new(HttpConfig {
            allowed_prefixes: vec!["https://".into()],
            ..HttpConfig::default()
        });
        let sdk = SdkRegistry::empty().register(CorePack).register(http_pack);
        let pool = RuntimePool::new(
            one_slot(),
            SandboxConfig::trusted(),
            hello_sandbox::loader::AllowlistModuleLoaderBuilder::default(),
            sdk,
        );

        let err = pool
            .run_with_caps(
                r#"
                import { fetch } from "sandbox:http";
                await fetch("https://example.com");
                return "ok";
                "#,
                HashMap::new(),
                RunCapabilities {
                    http_enabled: Some(false),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();

        match err {
            SandboxError::Runtime(_) => {}, // "capability denied: http" JS exception
            other => panic!("expected Runtime error, got: {other:?}"),
        }
    });
}

// ─── http_allowed_prefixes ────────────────────────────────────────────────────

#[test]
fn http_per_run_allowlist_blocks_pool_allowed_url() {
    // Pool allows "https://allowed.com". Run cap restricts to "https://other.com".
    // Fetching "https://allowed.com" should fail because the run-level cap overrides.
    run_sync(async {
        let http_pack = HttpPack::new(HttpConfig {
            allowed_prefixes: vec!["https://allowed.com".into()],
            ..HttpConfig::default()
        });
        let sdk = SdkRegistry::empty().register(CorePack).register(http_pack);
        let pool = RuntimePool::new(
            one_slot(),
            SandboxConfig::trusted(),
            hello_sandbox::loader::AllowlistModuleLoaderBuilder::default(),
            sdk,
        );

        let err = pool
            .run_with_caps(
                r#"
                import { fetch } from "sandbox:http";
                await fetch("https://allowed.com/api");
                return "ok";
                "#,
                HashMap::new(),
                RunCapabilities {
                    // Replace pool allowlist — "allowed.com" is now blocked.
                    http_allowed_prefixes: Some(vec!["https://other.com".into()]),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();

        match err {
            SandboxError::Runtime(e) => {
                assert!(e.to_string().contains("allowlist"), "expected allowlist error, got: {e}");
            },
            other => panic!("expected Runtime error, got: {other:?}"),
        }
    });
}

#[test]
fn http_empty_allowlist_blocks_all_urls() {
    run_sync(async {
        let http_pack = HttpPack::new(HttpConfig {
            allowed_prefixes: vec!["https://".into()],
            ..HttpConfig::default()
        });
        let sdk = SdkRegistry::empty().register(CorePack).register(http_pack);
        let pool = RuntimePool::new(
            one_slot(),
            SandboxConfig::trusted(),
            hello_sandbox::loader::AllowlistModuleLoaderBuilder::default(),
            sdk,
        );

        let err = pool
            .run_with_caps(
                r#"
                import { fetch } from "sandbox:http";
                await fetch("https://example.com");
                return "ok";
                "#,
                HashMap::new(),
                RunCapabilities {
                    http_allowed_prefixes: Some(vec![]), // empty = block all
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();

        match err {
            SandboxError::Runtime(e) => {
                assert!(e.to_string().contains("allowlist"));
            },
            other => panic!("expected Runtime error, got: {other:?}"),
        }
    });
}

// ─── http_allowed_methods ─────────────────────────────────────────────────────

#[test]
fn http_method_restriction_blocks_disallowed_verb() {
    run_sync(async {
        let http_pack = HttpPack::new(HttpConfig {
            allowed_prefixes: vec!["https://".into()],
            ..HttpConfig::default()
        });
        let sdk = SdkRegistry::empty().register(CorePack).register(http_pack);
        let pool = RuntimePool::new(
            one_slot(),
            SandboxConfig::trusted(),
            hello_sandbox::loader::AllowlistModuleLoaderBuilder::default(),
            sdk,
        );

        let err = pool
            .run_with_caps(
                r#"
                import { fetch } from "sandbox:http";
                await fetch("https://example.com", { method: "POST" });
                return "ok";
                "#,
                HashMap::new(),
                RunCapabilities {
                    http_allowed_methods: Some(vec!["GET".into()]),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();

        match err {
            SandboxError::Runtime(e) => {
                assert!(
                    e.to_string().contains("capability denied"),
                    "expected capability denied, got: {e}"
                );
            },
            other => panic!("expected Runtime error, got: {other:?}"),
        }
    });
}

// ─── http_calls_limit ────────────────────────────────────────────────────────

#[test]
fn http_calls_limit_overrides_pool_level() {
    run_sync(async {
        let http_pack = HttpPack::new(HttpConfig {
            allowed_prefixes: vec!["https://".into()],
            ..HttpConfig::default()
        });
        let sdk = SdkRegistry::empty().register(CorePack).register(http_pack);
        let pool = RuntimePool::new(
            one_slot(),
            SandboxConfig::trusted(), // no pool-level http limit
            hello_sandbox::loader::AllowlistModuleLoaderBuilder::default(),
            sdk,
        );

        let err = pool
            .run_with_caps(
                r#"
                import { fetch } from "sandbox:http";
                await fetch("https://example.com");
                return "ok";
                "#,
                HashMap::new(),
                RunCapabilities {
                    http_calls_limit: Some(0), // block all HTTP for this run
                    ..Default::default()
                },
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

// ─── emit_enabled ────────────────────────────────────────────────────────────

#[test]
fn emit_disabled_silently_drops_all_events() {
    run_sync(async {
        let sdk = SdkRegistry::empty().register(CorePack);
        let pool = RuntimePool::new(
            one_slot(),
            SandboxConfig::trusted(),
            hello_sandbox::loader::AllowlistModuleLoaderBuilder::default(),
            sdk,
        );

        // Script emits two events — should succeed (no error), but events are dropped.
        let r = pool
            .run_with_caps(
                r#"
                sandbox.emit("a", {});
                sandbox.emit("b", {});
                return "ok";
                "#,
                HashMap::new(),
                RunCapabilities {
                    emit_enabled: Some(false),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        assert_eq!(r.value, json!("ok"));
        assert!(r.events.is_empty(), "events should be silently dropped");
        // emit_calls counter is NOT incremented for dropped events.
        assert_eq!(r.metrics.emit_calls, 0);
    });
}

// ─── emit_allowed_names ───────────────────────────────────────────────────────

#[test]
fn emit_allowed_names_filters_events() {
    run_sync(async {
        let sdk = SdkRegistry::empty().register(CorePack);
        let pool = RuntimePool::new(
            one_slot(),
            SandboxConfig::trusted(),
            hello_sandbox::loader::AllowlistModuleLoaderBuilder::default(),
            sdk,
        );

        // Emit "ok" (allowed) and "blocked" (not allowed).
        let r = pool
            .run_with_caps(
                r#"
                sandbox.emit("ok", { v: 1 });
                sandbox.emit("blocked", { v: 2 });
                return "done";
                "#,
                HashMap::new(),
                RunCapabilities {
                    emit_allowed_names: Some(vec!["ok".into()]),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        assert_eq!(r.value, json!("done"));
        // Only the "ok" event should be in the result.
        assert_eq!(r.events.len(), 1);
        assert_eq!(r.events[0].name, "ok");
        // emit_calls only counts events that passed through (not the dropped one).
        assert_eq!(r.metrics.emit_calls, 1);
    });
}

// ─── emit_calls_limit ────────────────────────────────────────────────────────

#[test]
fn emit_calls_limit_overrides_pool_level() {
    run_sync(async {
        let sdk = SdkRegistry::empty().register(CorePack);
        let pool = RuntimePool::new(
            one_slot(),
            SandboxConfig::trusted(), // no pool-level emit limit
            hello_sandbox::loader::AllowlistModuleLoaderBuilder::default(),
            sdk,
        );

        let err = pool
            .run_with_caps(
                r#"
                sandbox.emit("a", {});
                sandbox.emit("b", {});
                return "ok";
                "#,
                HashMap::new(),
                RunCapabilities {
                    emit_calls_limit: Some(1), // allow only 1 emit
                    ..Default::default()
                },
            )
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

// ─── Backward compatibility ───────────────────────────────────────────────────

#[test]
fn default_capabilities_are_noop() {
    // RunCapabilities::default() must be a complete no-op — existing run()
    // behaviour is unchanged.
    run_sync(async {
        let sdk = SdkRegistry::empty().register(CorePack).register(KvPack::new());
        let pool = RuntimePool::new(
            one_slot(),
            SandboxConfig::trusted(),
            hello_sandbox::loader::AllowlistModuleLoaderBuilder::default(),
            sdk,
        );

        let r = pool
            .run_with_caps(
                r#"
                import { kv } from "sandbox:kv";
                await kv.set("key", "value");
                const v = await kv.get("key");
                sandbox.emit("done", { v });
                return v;
                "#,
                HashMap::new(),
                RunCapabilities::default(),
            )
            .await
            .unwrap();

        assert_eq!(r.value, json!("value"));
        assert_eq!(r.events.len(), 1);
        assert_eq!(r.metrics.kv_ops, 2);
        assert_eq!(r.metrics.emit_calls, 1);
    });
}

// ─── Streaming + capabilities ─────────────────────────────────────────────────

#[test]
fn run_streaming_with_caps_filters_emit_names() {
    run_sync(async {
        let sdk = SdkRegistry::empty().register(CorePack);
        let pool = RuntimePool::new(
            one_slot(),
            SandboxConfig::trusted(),
            hello_sandbox::loader::AllowlistModuleLoaderBuilder::default(),
            sdk,
        );

        let (fut, mut rx) = pool.run_streaming_with_caps(
            r#"
            sandbox.emit("allowed", { n: 1 });
            sandbox.emit("blocked", { n: 2 });
            sandbox.emit("allowed", { n: 3 });
            return "done";
            "#,
            HashMap::new(),
            RunCapabilities {
                emit_allowed_names: Some(vec!["allowed".into()]),
                ..Default::default()
            },
        );

        let result = fut.await.unwrap();
        assert_eq!(result.value, json!("done"));

        // Drain the streaming receiver.
        let mut events = vec![];
        while let Ok(e) = rx.try_recv() {
            events.push(e);
        }
        // Only the two "allowed" events should have been forwarded.
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|e| e.name == "allowed"));
    });
}

// ─── Multiple capabilities combined ──────────────────────────────────────────

#[test]
fn combined_kv_and_emit_caps() {
    // Verify that multiple capability fields work together in a single run.
    run_sync(async {
        let sdk = SdkRegistry::empty().register(CorePack).register(KvPack::new());
        let pool = RuntimePool::new(
            one_slot(),
            SandboxConfig::trusted(),
            hello_sandbox::loader::AllowlistModuleLoaderBuilder::default(),
            sdk,
        );

        let r = pool
            .run_with_caps(
                r#"
                import { kv } from "sandbox:kv";
                await kv.set("msg", "hello");
                sandbox.emit("data", { key: "msg" });    // allowed
                sandbox.emit("internal", { secret: 1 }); // filtered out
                return "ok";
                "#,
                HashMap::new(),
                RunCapabilities {
                    kv_key_prefix: Some("tenant:A:".into()),
                    emit_allowed_names: Some(vec!["data".into()]),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        assert_eq!(r.value, json!("ok"));
        assert_eq!(r.events.len(), 1);
        assert_eq!(r.events[0].name, "data");
        assert_eq!(r.metrics.kv_ops, 1);
        assert_eq!(r.metrics.emit_calls, 1);
    });
}
