//! Phase 8 — V8 heap-limit integration tests.
//!
//! All tests use `pool_size=1` (or 0) to stay within the V8 single-isolate
//! per-thread constraint.

use std::time::Duration;

use hello_sandbox::{IsolationLevel, PoolConfig, Sandbox, SandboxConfig, SandboxError};
use tokio::task::LocalSet;

// ── Config helpers ────────────────────────────────────────────────────────────

/// A config with a tight 16 MiB heap limit (same as `SandboxConfig::untrusted()`).
fn tight_config() -> SandboxConfig {
    SandboxConfig {
        isolation: IsolationLevel::PowerUser,
        timeout: Duration::from_secs(10),
        heap_initial_bytes: 4 * 1024 * 1024, // 4 MiB
        heap_max_bytes: 16 * 1024 * 1024,    // 16 MiB
        max_log_lines: 100,
        allow_modules: false,
        allow_typescript: false,
        allow_events: false,
        ..SandboxConfig::power_user()
    }
}

/// A config with a generous 256 MiB heap limit (same as `SandboxConfig::trusted()`).
fn large_config() -> SandboxConfig {
    SandboxConfig {
        isolation: IsolationLevel::Trusted,
        timeout: Duration::from_secs(30),
        heap_initial_bytes: 8 * 1024 * 1024, //   8 MiB
        heap_max_bytes: 256 * 1024 * 1024,   // 256 MiB
        max_log_lines: 1000,
        allow_modules: false,
        allow_typescript: false,
        allow_events: false,
        ..SandboxConfig::trusted()
    }
}

fn single_slot(config: SandboxConfig) -> Sandbox {
    Sandbox::builder()
        .config(config)
        .pool(PoolConfig {
            pool_size: 1,
            max_runs_per_slot: 10,
            ..Default::default()
        })
        .build()
        .unwrap()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Allocating far beyond the heap limit must return `SandboxError::OutOfMemory`.
#[tokio::test(flavor = "current_thread")]
async fn oom_large_allocation_returns_out_of_memory() {
    LocalSet::new()
        .run_until(async {
            let mut sb = single_slot(tight_config());

            // Allocate ~80 MiB of JS objects — well above the 16 MiB limit.
            let err = sb
                .run(
                    r#"
                    const chunks = [];
                    for (let i = 0; i < 1000; i++) {
                        chunks.push(new Array(10000).fill(i));
                    }
                    return chunks.length;
                "#,
                )
                .await
                .unwrap_err();

            assert!(matches!(err, SandboxError::OutOfMemory), "expected OutOfMemory, got {err:?}");
        })
        .await;
}

/// A script that allocates only a small amount must succeed with the tight config.
#[tokio::test(flavor = "current_thread")]
async fn oom_small_allocation_succeeds() {
    LocalSet::new()
        .run_until(async {
            let mut sb = single_slot(tight_config());

            let result = sb
                .run(
                    r#"
                    const arr = new Array(100).fill(42);
                    return arr.length;
                "#,
                )
                .await
                .unwrap();

            assert_eq!(result.value, serde_json::json!(100));
        })
        .await;
}

/// The same allocation that OOMs with a tight limit succeeds with a generous one,
/// confirming that configs produce distinct heap limits.
#[tokio::test(flavor = "current_thread")]
async fn oom_tight_limit_fails_where_large_limit_succeeds() {
    LocalSet::new()
        .run_until(async {
            // Script allocates ~20 MiB of JS objects.
            let script = r#"
                const chunks = [];
                for (let i = 0; i < 200; i++) {
                    chunks.push(new Array(10000).fill(i));
                }
                return chunks.length;
            "#;

            // With the tight 16 MiB limit this must OOM.
            let mut sb_tight = single_slot(tight_config());
            let err = sb_tight.run(script).await.unwrap_err();
            assert!(
                matches!(err, SandboxError::OutOfMemory),
                "expected OutOfMemory with tight limit, got {err:?}"
            );

            // With the generous 256 MiB limit the same script succeeds.
            let mut sb_large = single_slot(large_config());
            let result = sb_large.run(script).await.unwrap();
            assert_eq!(result.value, serde_json::json!(200));
        })
        .await;
}

/// After an OOM, the pool marks the slot stale. The next run must succeed on a
/// fresh slot (confirming recovery).
#[tokio::test(flavor = "current_thread")]
async fn oom_next_run_succeeds_on_fresh_slot() {
    LocalSet::new()
        .run_until(async {
            let mut sb = single_slot(tight_config());

            // First run: OOM.
            let err = sb
                .run("const c = []; while (true) { c.push(new Array(10000).fill(0)); }")
                .await
                .unwrap_err();
            assert!(matches!(err, SandboxError::OutOfMemory), "expected OOM, got {err:?}");

            // Second run: should succeed on a recycled (fresh) slot.
            let result = sb.run("return 42").await.unwrap();
            assert_eq!(result.value, serde_json::json!(42));
        })
        .await;
}

/// `SandboxConfig::untrusted()` specifies a 16 MiB max heap. Verify OOM fires.
#[tokio::test(flavor = "current_thread")]
async fn oom_untrusted_config_enforces_16mib_limit() {
    LocalSet::new()
        .run_until(async {
            let config = SandboxConfig {
                // Use PowerUser isolation to avoid the child-process path
                // (Phase 9 is not implemented yet), but keep the Untrusted heap limits.
                isolation: IsolationLevel::PowerUser,
                ..SandboxConfig::untrusted()
            };
            let mut sb = Sandbox::builder()
                .config(config)
                .pool(PoolConfig {
                    pool_size: 1,
                    ..Default::default()
                })
                .build()
                .unwrap();

            let err = sb
                .run(
                    r#"
                    const c = [];
                    for (let i = 0; i < 1000; i++) {
                        c.push(new Array(10000).fill(i));
                    }
                    return c.length;
                "#,
                )
                .await
                .unwrap_err();

            assert!(
                matches!(err, SandboxError::OutOfMemory),
                "expected OutOfMemory with untrusted heap limits, got {err:?}"
            );
        })
        .await;
}
