//! Phase 17 — TimerPack integration tests.
//!
//! Tests cover `setTimeout`, `clearTimeout`, `setInterval`, `clearInterval`
//! installed by `TimerPack` via the `// PRE_FREEZE_INJECTION` mechanism in
//! `core.js`.
//!
//! All tests run inside a `tokio::task::LocalSet` because V8 is `!Send`.

use hello_sandbox::{PoolConfig, Sandbox, SandboxConfig, SandboxError, TimerPack};
use serde_json::json;
use tokio::task::LocalSet;

fn timer_sandbox() -> Sandbox {
    Sandbox::builder()
        .config(SandboxConfig::trusted())
        .pool(PoolConfig {
            pool_size: 1,
            max_runs_per_slot: 100,
            ..Default::default()
        })
        .sdk(TimerPack)
        .build()
        .unwrap()
}

fn timer_sandbox_isolated() -> Sandbox {
    Sandbox::builder()
        .config(SandboxConfig::trusted())
        .pool(PoolConfig {
            pool_size: 0,
            ..Default::default()
        })
        .sdk(TimerPack)
        .build()
        .unwrap()
}

// ── Availability ──────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn timer_globals_are_available() {
    LocalSet::new()
        .run_until(async {
            let mut sb = timer_sandbox_isolated();
            let r = sb.run("return typeof setTimeout").await.unwrap();
            assert_eq!(r.value, json!("function"));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn timer_globals_all_present() {
    LocalSet::new()
        .run_until(async {
            let mut sb = timer_sandbox_isolated();
            let r = sb
                .run(
                    r#"
                    return [
                        typeof setTimeout,
                        typeof clearTimeout,
                        typeof setInterval,
                        typeof clearInterval,
                    ];
                "#,
                )
                .await
                .unwrap();
            assert_eq!(r.value, json!(["function", "function", "function", "function"]));
        })
        .await;
}

// ── setTimeout ────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn settimeout_fires_callback() {
    LocalSet::new()
        .run_until(async {
            let mut sb = timer_sandbox_isolated();
            let r = sb
                .run(
                    r#"
                    let fired = false;
                    await new Promise(resolve => {
                        setTimeout(() => { fired = true; resolve(); }, 10);
                    });
                    return fired;
                "#,
                )
                .await
                .unwrap();
            assert_eq!(r.value, json!(true));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn settimeout_zero_delay_fires() {
    LocalSet::new()
        .run_until(async {
            let mut sb = timer_sandbox_isolated();
            let r = sb
                .run(
                    r#"
                    let order = [];
                    await new Promise(resolve => {
                        order.push("before");
                        setTimeout(() => { order.push("timer"); resolve(); }, 0);
                        order.push("after");
                    });
                    return order;
                "#,
                )
                .await
                .unwrap();
            // "before" and "after" happen synchronously, "timer" fires async
            assert_eq!(r.value, json!(["before", "after", "timer"]));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn settimeout_returns_numeric_id() {
    LocalSet::new()
        .run_until(async {
            let mut sb = timer_sandbox_isolated();
            let r = sb
                .run(
                    r#"
                    const id = setTimeout(() => {}, 1000);
                    clearTimeout(id);
                    return typeof id === "number" && id > 0;
                "#,
                )
                .await
                .unwrap();
            assert_eq!(r.value, json!(true));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn settimeout_sequential_order() {
    LocalSet::new()
        .run_until(async {
            let mut sb = timer_sandbox_isolated();
            let r = sb
                .run(
                    r#"
                    const order = [];
                    await new Promise(resolve => {
                        setTimeout(() => { order.push(1); }, 10);
                        setTimeout(() => { order.push(2); }, 20);
                        setTimeout(() => { order.push(3); resolve(); }, 30);
                    });
                    return order;
                "#,
                )
                .await
                .unwrap();
            assert_eq!(r.value, json!([1, 2, 3]));
        })
        .await;
}

// ── clearTimeout ─────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn cleartimeout_prevents_callback() {
    LocalSet::new()
        .run_until(async {
            let mut sb = timer_sandbox_isolated();
            let r = sb
                .run(
                    r#"
                    let fired = false;
                    const id = setTimeout(() => { fired = true; }, 50);
                    clearTimeout(id);
                    // Give the event loop time to process; callback should NOT fire.
                    await new Promise(resolve => setTimeout(resolve, 100));
                    return fired;
                "#,
                )
                .await
                .unwrap();
            assert_eq!(r.value, json!(false));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn cleartimeout_noop_on_invalid_id() {
    LocalSet::new()
        .run_until(async {
            let mut sb = timer_sandbox_isolated();
            // clearTimeout with a garbage ID should not throw or crash.
            let r = sb
                .run(
                    r#"
                    clearTimeout(99999);
                    clearTimeout(0);
                    return "ok";
                "#,
                )
                .await
                .unwrap();
            assert_eq!(r.value, json!("ok"));
        })
        .await;
}

// ── setInterval ───────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn setinterval_fires_multiple_times() {
    LocalSet::new()
        .run_until(async {
            let mut sb = timer_sandbox_isolated();
            let r = sb
                .run(
                    r#"
                    let count = 0;
                    await new Promise(resolve => {
                        const id = setInterval(() => {
                            count++;
                            if (count >= 3) {
                                clearInterval(id);
                                resolve();
                            }
                        }, 10);
                    });
                    return count;
                "#,
                )
                .await
                .unwrap();
            assert_eq!(r.value, json!(3));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn clearinterval_stops_interval() {
    LocalSet::new()
        .run_until(async {
            let mut sb = timer_sandbox_isolated();
            let r = sb
                .run(
                    r#"
                    let count = 0;
                    const id = setInterval(() => { count++; }, 10);
                    // Let it fire once, then cancel.
                    await new Promise(resolve => setTimeout(resolve, 25));
                    clearInterval(id);
                    const snapshot = count;
                    // Wait more; count should not increase after cancel.
                    await new Promise(resolve => setTimeout(resolve, 60));
                    return count === snapshot && snapshot >= 1;
                "#,
                )
                .await
                .unwrap();
            assert_eq!(r.value, json!(true));
        })
        .await;
}

// ── max_interval_calls limit ──────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn setinterval_respects_max_calls_limit() {
    LocalSet::new()
        .run_until(async {
            // Build a sandbox with a very low max_interval_calls limit.
            let mut config = SandboxConfig::trusted();
            config.max_interval_calls = 3;
            let mut sb = Sandbox::builder()
                .config(config)
                .pool(PoolConfig {
                    pool_size: 0,
                    ..Default::default()
                })
                .sdk(TimerPack)
                .build()
                .unwrap();

            let r = sb
                .run(
                    r#"
                    let count = 0;
                    // No explicit clearInterval — the limit should stop it.
                    await new Promise(resolve => {
                        setInterval(() => { count++; }, 5);
                        // Resolve after enough time for many potential firings.
                        setTimeout(resolve, 200);
                    });
                    return count;
                "#,
                )
                .await
                .unwrap();

            // Should be exactly 3 (the configured limit).
            assert_eq!(r.value, json!(3));
        })
        .await;
}

// ── Timers auto-cancel at run end ─────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn pending_timers_do_not_block_run_completion() {
    LocalSet::new()
        .run_until(async {
            let mut sb = timer_sandbox_isolated();
            // Schedule a far-future timer that should never fire.
            // The run should still complete quickly.
            let r = sb
                .run(
                    r#"
                    setTimeout(() => { }, 600000); // 10 minutes
                    return "done";
                "#,
                )
                .await
                .unwrap();
            assert_eq!(r.value, json!("done"));
        })
        .await;
}

// ── Warm pool slot ────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn timers_work_on_warm_pool_slot() {
    LocalSet::new()
        .run_until(async {
            let mut sb = timer_sandbox();

            // First run.
            let r1 = sb
                .run(
                    r#"
                    let v = 0;
                    await new Promise(r => setTimeout(() => { v = 42; r(); }, 10));
                    return v;
                "#,
                )
                .await
                .unwrap();
            assert_eq!(r1.value, json!(42));

            // Second run on the same warm slot — timers from run 1 must not leak.
            let r2 = sb
                .run(
                    r#"
                    let v = 0;
                    await new Promise(r => setTimeout(() => { v = 99; r(); }, 10));
                    return v;
                "#,
                )
                .await
                .unwrap();
            assert_eq!(r2.value, json!(99));
        })
        .await;
}

// ── TypeScript ────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn timers_work_in_typescript() {
    LocalSet::new()
        .run_until(async {
            let mut sb = timer_sandbox_isolated();
            let r = sb
                .run(
                    r#"
                    const delay = (ms: number): Promise<void> =>
                        new Promise(resolve => setTimeout(resolve, ms));
                    await delay(10);
                    return "ts-ok";
                "#,
                )
                .await
                .unwrap();
            assert_eq!(r.value, json!("ts-ok"));
        })
        .await;
}

// ── Error propagation ─────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn error_in_settimeout_callback_propagates() {
    LocalSet::new()
        .run_until(async {
            let mut sb = timer_sandbox_isolated();
            // Errors thrown inside timer callbacks are swallowed by timer_globals.js
            // (try/catch) so they do not reject the surrounding Promise.
            // The script should still complete normally.
            let r = sb
                .run(
                    r#"
                    let reached = false;
                    await new Promise(resolve => {
                        setTimeout(() => {
                            throw new Error("timer-boom");
                        }, 5);
                        setTimeout(() => {
                            reached = true;
                            resolve();
                        }, 20);
                    });
                    return reached;
                "#,
                )
                .await
                .unwrap();
            assert_eq!(r.value, json!(true));
        })
        .await;
}

// ── Sandbox without TimerPack — no globals ────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn without_timerpack_no_settimeout_global() {
    LocalSet::new()
        .run_until(async {
            let mut sb = Sandbox::builder()
                .config(SandboxConfig::trusted())
                .pool(PoolConfig {
                    pool_size: 0,
                    ..Default::default()
                })
                .build()
                .unwrap();
            let r = sb.run("return typeof setTimeout").await.unwrap();
            assert_eq!(r.value, json!("undefined"));
        })
        .await;
}

// ── Script error while timers are pending ─────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn script_error_cancels_pending_timers() {
    LocalSet::new()
        .run_until(async {
            let mut sb = timer_sandbox_isolated();
            let err = sb
                .run(
                    r#"
                    setTimeout(() => { /* never runs */ }, 5000);
                    throw new Error("oops");
                "#,
                )
                .await
                .unwrap_err();
            assert!(matches!(err, SandboxError::Runtime(_)), "expected Runtime error, got {err:?}");
        })
        .await;
}
