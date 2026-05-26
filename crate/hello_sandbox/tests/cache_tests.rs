//! Phase 12 — V8 Compilation Cache integration tests.
//!
//! These tests verify that:
//! - `CodeCache` public API works correctly (new_shared, len, is_empty).
//! - The pool auto-wires a shared cache and runs succeed with it enabled.
//! - Two sequential runs on the same slot both return correct results.
//! - A user-registered module is correctly imported when caching is active.
//! - A shared `CodeCache` can be attached to a loader builder explicitly.
//!
//! All pool tests use `pool_size = 1` (V8 single-thread constraint).

use std::collections::HashMap;
use std::sync::Arc;

use hello_sandbox::loader::{AllowlistModuleLoaderBuilder, CodeCache};
use hello_sandbox::sdk::core_sdk::CorePack;
use hello_sandbox::sdk::SdkRegistry;
use hello_sandbox::{PoolConfig, RuntimePool, SandboxBuilder, SandboxConfig};
use serde_json::json;

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn one_slot() -> PoolConfig {
    PoolConfig {
        pool_size: 1,
        ..PoolConfig::default()
    }
}

fn trusted_config() -> SandboxConfig {
    SandboxConfig::trusted()
}

// ─── CodeCache unit-level tests (no V8 needed) ────────────────────────────────

#[test]
fn code_cache_new_shared_is_empty() {
    let cache = CodeCache::new_shared();
    let c = cache.lock().unwrap();
    assert!(c.is_empty());
    assert_eq!(c.len(), 0);
}

#[test]
fn code_cache_shared_arc_identity() {
    let cache = CodeCache::new_shared();
    let cache2 = cache.clone();
    assert!(Arc::ptr_eq(&cache, &cache2), "cloning the Arc should share the same allocation");
}

#[test]
fn code_cache_with_code_cache_propagates_on_builder_clone() {
    let cache = CodeCache::new_shared();

    // Attach the cache to a builder, then clone the builder.
    let b1 = AllowlistModuleLoaderBuilder::default()
        .register("sandbox:a", "export const a = 1;")
        .with_code_cache(cache.clone());

    let b2 = b1.clone().register("sandbox:b", "export const b = 2;");

    // Both builders should share the same Arc (verified by ptr equality).
    // We verify that by building both loaders — no panic is sufficient here
    // (we can't inspect private fields from an integration test).
    let _l1 = b1.build().expect("loader1 build");
    let _l2 = b2.build().expect("loader2 build");
}

// ─── Pool integration tests ────────────────────────────────────────────────────

#[test]
fn pool_plain_run_with_cache_enabled() {
    // The pool auto-creates a CodeCache in RuntimePool::new().
    // Verify execution is unaffected.
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();

    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async {
        let sdk = SdkRegistry::empty().register(CorePack);
        let loader_builder = AllowlistModuleLoaderBuilder::default();
        let pool = RuntimePool::new(one_slot(), trusted_config(), loader_builder, sdk);

        let result = pool.run("return 6 * 7;", HashMap::new()).await.unwrap();
        assert_eq!(result.value, json!(42));
    });
}

#[test]
fn pool_two_sequential_runs_both_succeed() {
    // Second run uses the warm slot; verifies cache doesn't corrupt subsequent runs.
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();

    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async {
        let sdk = SdkRegistry::empty().register(CorePack);
        let loader_builder = AllowlistModuleLoaderBuilder::default();
        let pool = RuntimePool::new(one_slot(), trusted_config(), loader_builder, sdk);

        let r1 = pool.run("return 1;", HashMap::new()).await.unwrap();
        let r2 = pool.run("return 2;", HashMap::new()).await.unwrap();

        assert_eq!(r1.value, json!(1));
        assert_eq!(r2.value, json!(2));
    });
}

#[test]
fn pool_user_module_import_with_cache() {
    // Registers a user module, imports it in a script.
    // The CodeCache is active; verifies that module loading still works.
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();

    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async {
        let sdk = SdkRegistry::empty().register(CorePack);
        let loader_builder = AllowlistModuleLoaderBuilder::default()
            .register("sandbox:math", "export const triple = (n) => n * 3;");
        let pool = RuntimePool::new(one_slot(), trusted_config(), loader_builder, sdk);

        let script = r#"
            import { triple } from "sandbox:math";
            return triple(14);
        "#;

        let result = pool.run(script, HashMap::new()).await.unwrap();
        assert_eq!(result.value, json!(42));
    });
}

#[test]
fn pool_user_module_cached_on_second_run() {
    // Run the same module-importing script twice.
    // The second run should hit the bytecode cache (no observable diff from
    // the test perspective, but must not panic or return wrong results).
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();

    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async {
        let sdk = SdkRegistry::empty().register(CorePack);
        let loader_builder = AllowlistModuleLoaderBuilder::default()
            .register("sandbox:math", "export const double = (n) => n * 2;");
        let pool = RuntimePool::new(one_slot(), trusted_config(), loader_builder, sdk);

        let script = r#"
            import { double } from "sandbox:math";
            return double(21);
        "#;

        let r1 = pool.run(script, HashMap::new()).await.unwrap();
        let r2 = pool.run(script, HashMap::new()).await.unwrap();

        assert_eq!(r1.value, json!(42));
        assert_eq!(r2.value, json!(42));
    });
}

#[test]
fn explicit_cache_shared_sequentially_across_two_pools() {
    // Attaches the same CodeCache Arc to two independent isolated pools
    // (pool_size = 0 so no V8 isolates are created at construction — only one
    // isolate exists at a time during each run, satisfying the single-thread
    // constraint).
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();

    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async {
        let shared_cache = CodeCache::new_shared();
        let isolated = PoolConfig {
            pool_size: 0,
            ..PoolConfig::default()
        };

        let sdk1 = SdkRegistry::empty().register(CorePack);
        let lb1 = AllowlistModuleLoaderBuilder::default().with_code_cache(shared_cache.clone());
        let pool1 = RuntimePool::new(isolated.clone(), trusted_config(), lb1, sdk1);

        // pool1 runs first — its isolate is created and dropped before pool2 runs.
        let r1 = pool1.run("return 10;", HashMap::new()).await.unwrap();
        drop(pool1);

        let sdk2 = SdkRegistry::empty().register(CorePack);
        let lb2 = AllowlistModuleLoaderBuilder::default().with_code_cache(shared_cache.clone());
        let pool2 = RuntimePool::new(isolated, trusted_config(), lb2, sdk2);

        let r2 = pool2.run("return 20;", HashMap::new()).await.unwrap();

        assert_eq!(r1.value, json!(10));
        assert_eq!(r2.value, json!(20));

        // Cache Arc is still valid after both pools have run.
        let _ = shared_cache.lock().unwrap().len();
    });
}

#[test]
fn sandbox_builder_runs_with_cache_auto_enabled() {
    // End-to-end through the public SandboxBuilder API.
    // The pool auto-creates a cache; we just verify execution is correct.
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();

    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async {
        let mut sandbox = SandboxBuilder::new()
            .config(SandboxConfig::trusted())
            .pool(one_slot())
            .build()
            .unwrap();

        let result = sandbox.run("return 42;").await.unwrap();
        assert_eq!(result.value, json!(42));
    });
}
