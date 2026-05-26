//! cargo run --example demo
//!
//! Demonstrates the three runtime paths the pool provides:
//!   1. Warm slot reuse        (most runs)
//!   2. Slot recycle           (after max_runs_per_slot)
//!   3. Isolated fallback      (when all slots busy simultaneously)

use deno_sandbox::{Sandbox, SandboxConfig, SandboxError};
use deno_sandbox::pool::PoolConfig;
use serde_json::json;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("deno_sandbox=info")
        .init();

    let local = tokio::task::LocalSet::new();
    local.run_until(async {
        demo_warm_reuse().await;
        demo_slot_recycle().await;
        demo_isolated_fallback().await;
        demo_error_discards_slot().await;
        demo_concurrent_burst().await;
    }).await;

    Ok(())
}

// ── 1. Warm slot reuse ────────────────────────────────────────────────────────
// All runs fit in one slot; we confirm the same slot is reused each time.

async fn demo_warm_reuse() {
    println!("\n=== 1. Warm slot reuse ===");

    let mut sb = Sandbox::builder()
        .config(SandboxConfig::power_user())
        .pool(PoolConfig { pool_size: 2, max_runs_per_slot: 50, ..Default::default() })
        .build()
        .unwrap();

    for i in 0..4 {
        sb.set_input("i", json!(i));
        let r = sb.run("return sandbox.readInput('i') * 10;").await.unwrap();
        println!("  run {i}: value={} via {:?}", r.value, r.runtime_kind);
    }

    let stats = sb.pool_stats().await;
    println!("  pool: {:?}", stats);
}

// ── 2. Slot recycle ───────────────────────────────────────────────────────────
// max_runs_per_slot=3 forces a new runtime every 3 runs.
// Watch the slot index reset between batches.

async fn demo_slot_recycle() {
    println!("\n=== 2. Slot recycle (max_runs=3) ===");

    let mut sb = Sandbox::builder()
        .config(SandboxConfig::power_user())
        .pool(PoolConfig {
            pool_size: 1,
            max_runs_per_slot: 3,
            ..Default::default()
        })
        .build()
        .unwrap();

    for i in 0..9 {
        let r = sb.run("return 1;").await.unwrap();
        // Slot 0 is recycled every 3 runs — new runtime, same slot index.
        println!("  run {i}: {:?}", r.runtime_kind);
    }
}

// ── 3. Isolated fallback ──────────────────────────────────────────────────────
// Pool size=1. Fire two concurrent runs — the second overflows to isolated.

async fn demo_isolated_fallback() {
    println!("\n=== 3. Isolated fallback (pool_size=1) ===");

    // Build two Sandbox handles pointing at the SAME underlying pool.
    // (In a real service you'd pass Arc<RuntimePool> around.)
    // Here we simulate by running two sequential runs where the first
    // is designed to exhaust the pool (we can't do true concurrency easily
    // in a demo without shared state, so we just show the path via PoolConfig).

    let mut sb = Sandbox::builder()
        .config(SandboxConfig::power_user())
        .pool(PoolConfig {
            pool_size: 0,                   // no warm slots at all
            fallback_to_isolated: true,
            ..Default::default()
        })
        .build()
        .unwrap();

    for i in 0..3 {
        let r = sb.run("return 42;").await.unwrap();
        println!("  run {i}: {:?} (should be Isolated)", r.runtime_kind);
    }
}

// ── 4. Error discards slot ────────────────────────────────────────────────────
// A script runtime error marks the slot Stale so a fresh runtime
// is created for the next run — no tainted state leaks.

async fn demo_error_discards_slot() {
    println!("\n=== 4. Error discards slot ===");

    let mut sb = Sandbox::builder()
        .config(SandboxConfig::power_user())
        .pool(PoolConfig { pool_size: 1, max_runs_per_slot: 100, ..Default::default() })
        .build()
        .unwrap();

    // First run — healthy, warms slot 0.
    let r = sb.run("return 'before error';").await.unwrap();
    println!("  before: {} via {:?}", r.value, r.runtime_kind);

    // Second run — throws, slot is discarded.
    let err = sb.run("throw new Error('boom');").await;
    println!("  error run: {:?}", err.map(|_| ()));

    // Third run — slot 0 is fresh again (recycled after the error).
    let r = sb.run("return 'after error — fresh slot';").await.unwrap();
    println!("  after:  {} via {:?}", r.value, r.runtime_kind);
}

// ── 5. Concurrent burst ───────────────────────────────────────────────────────
// Spawn N futures concurrently. The pool serves up to pool_size with warm slots;
// the rest fall back to isolated runtimes.

async fn demo_concurrent_burst() {
    println!("\n=== 5. Concurrent burst (pool_size=2, burst=6) ===");
    use std::sync::Arc;
    use tokio::sync::Mutex;

    // We need a shared sandbox for this. Wrap in Arc<Mutex<>> for the demo.
    let sb = Arc::new(Mutex::new(
        Sandbox::builder()
            .config(SandboxConfig::power_user())
            .pool(PoolConfig {
                pool_size: 2,
                max_runs_per_slot: 100,
                fallback_to_isolated: true,
                ..Default::default()
            })
            .build()
            .unwrap(),
    ));

    let handles: Vec<_> = (0..6)
        .map(|i| {
            let sb = Arc::clone(&sb);
            tokio::task::spawn_local(async move {
                let mut guard = sb.lock().await;
                guard.set_input("i", json!(i));
                let r = guard.run("return sandbox.readInput('i');").await.unwrap();
                println!("  burst run {i}: {:?}", r.runtime_kind);
            })
        })
        .collect();

    for h in handles {
        h.await.unwrap();
    }

    let stats = sb.lock().await.pool_stats().await;
    println!("  final pool stats: {:?}", stats);
}
