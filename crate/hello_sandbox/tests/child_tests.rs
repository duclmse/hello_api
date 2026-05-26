//! Phase 9 — child-process isolation tests.
//!
//! On Linux these tests exercise the full seccomp + child-process path.
//! On non-Linux platforms the `Untrusted` level falls back to `PowerUser`
//! isolation (with a warning), so the value-correctness tests still pass.

use std::time::Duration;

use hello_sandbox::{IsolationLevel, PoolConfig, Sandbox, SandboxConfig, SandboxError};
use tokio::task::LocalSet;

// ── Config helpers ─────────────────────────────────────────────────────────────

fn untrusted_config() -> SandboxConfig {
    SandboxConfig {
        isolation: IsolationLevel::Untrusted,
        timeout: Duration::from_secs(10),
        heap_initial_bytes: 4 * 1024 * 1024,
        heap_max_bytes: 64 * 1024 * 1024,
        max_log_lines: 100,
        allow_modules: false,
        allow_typescript: false,
        allow_events: false,
        ..SandboxConfig::untrusted()
    }
}

fn sandbox_with(config: SandboxConfig) -> Sandbox {
    Sandbox::builder()
        .config(config)
        .pool(PoolConfig {
            pool_size: 1,
            ..Default::default()
        })
        .build()
        .unwrap()
}

// ── Tests ──────────────────────────────────────────────────────────────────────

/// Non-Linux: `Untrusted` falls back to `PowerUser` (warns) — basic value OK.
#[tokio::test(flavor = "current_thread")]
async fn untrusted_basic_script_returns_value() {
    LocalSet::new()
        .run_until(async {
            let mut sb = sandbox_with(untrusted_config());
            let result = sb.run("return 1 + 2").await.unwrap();
            assert_eq!(result.value, serde_json::json!(3));
        })
        .await;
}

/// Logs captured through the child-process (or fallback) path.
#[tokio::test(flavor = "current_thread")]
async fn untrusted_logs_captured() {
    LocalSet::new()
        .run_until(async {
            let mut sb = sandbox_with(untrusted_config());
            let result = sb
                .run(
                    r#"
                    console.log("hello from child");
                    return true;
                "#,
                )
                .await
                .unwrap();
            assert!(result.logs.iter().any(|l| l.contains("hello from child")));
        })
        .await;
}

/// Script error propagated back correctly through the protocol.
#[tokio::test(flavor = "current_thread")]
async fn untrusted_script_error_propagated() {
    LocalSet::new()
        .run_until(async {
            let mut sb = sandbox_with(untrusted_config());
            let err = sb.run("throw new Error('boom')").await.unwrap_err();
            match err {
                SandboxError::Runtime(_) | SandboxError::ChildProcess(_) => {},
                other => panic!("unexpected error: {other:?}"),
            }
        })
        .await;
}

/// Timeout enforced in the child process (or PowerUser fallback).
#[tokio::test(flavor = "current_thread")]
async fn untrusted_timeout_enforced() {
    LocalSet::new()
        .run_until(async {
            let mut config = untrusted_config();
            config.timeout = Duration::from_millis(300);
            let mut sb = sandbox_with(config);

            let err = sb.run("while (true) {}").await.unwrap_err();
            assert!(
                matches!(err, SandboxError::Timeout(_) | SandboxError::ChildProcess(_)),
                "expected Timeout or ChildProcess, got {err:?}"
            );
        })
        .await;
}

// ── Linux-only seccomp tests ───────────────────────────────────────────────────

/// On Linux, verify the child-process path (not just the fallback) is used by
/// checking that the worker binary resolves. If the binary isn't built yet
/// (CI scenario), the test is skipped gracefully.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "current_thread")]
async fn linux_child_process_path_used() {
    use hello_sandbox::child::find_worker_binary;

    let worker = find_worker_binary();
    if !worker.exists() {
        // Worker binary not built yet — skip rather than fail.
        eprintln!("SKIP: sandbox-worker binary not found at {}", worker.display());
        return;
    }

    LocalSet::new()
        .run_until(async {
            let mut sb = Sandbox::builder()
                .config(untrusted_config())
                .pool(PoolConfig {
                    pool_size: 1,
                    ..Default::default()
                })
                .worker_binary(&worker)
                .build()
                .unwrap();

            let result = sb.run("return 42").await.unwrap();
            assert_eq!(result.value, serde_json::json!(42));
        })
        .await;
}

/// On Linux with the worker binary, OOM in the child must surface as
/// `SandboxError::OutOfMemory` (worker marshals it in the response).
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "current_thread")]
async fn linux_child_oom_returns_out_of_memory() {
    use hello_sandbox::child::find_worker_binary;

    let worker = find_worker_binary();
    if !worker.exists() {
        eprintln!("SKIP: sandbox-worker binary not found at {}", worker.display());
        return;
    }

    LocalSet::new()
        .run_until(async {
            let config = SandboxConfig {
                isolation: IsolationLevel::Untrusted,
                heap_initial_bytes: 4 * 1024 * 1024,
                heap_max_bytes: 16 * 1024 * 1024, // tight 16 MiB
                ..untrusted_config()
            };

            let mut sb = Sandbox::builder()
                .config(config)
                .pool(PoolConfig {
                    pool_size: 1,
                    ..Default::default()
                })
                .worker_binary(&worker)
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
                "expected OutOfMemory from child, got {err:?}"
            );
        })
        .await;
}
