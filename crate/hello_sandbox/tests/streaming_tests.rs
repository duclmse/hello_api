//! Phase 16 — Real-Time Event Streaming integration tests.
//!
//! `Sandbox::run_streaming()` returns `(future, receiver)` immediately.
//! Events emitted via `sandbox.emit()` arrive at the receiver as they fire
//! rather than being batch-collected after the script completes.
//!
//! All tests run inside a `tokio::task::LocalSet` because V8 is `!Send`.
//! The future returned by `run_streaming` is also `!Send`; only the receiver
//! is `Send`.

use hello_sandbox::{PoolConfig, Sandbox, SandboxConfig, SandboxError};
use serde_json::json;
use tokio::task::LocalSet;

fn isolated_sandbox() -> Sandbox {
    Sandbox::builder()
        .config(SandboxConfig::trusted())
        .pool(PoolConfig {
            pool_size: 0,
            ..Default::default()
        })
        .build()
        .unwrap()
}

fn single_slot_sandbox() -> Sandbox {
    Sandbox::builder()
        .config(SandboxConfig::trusted())
        .pool(PoolConfig {
            pool_size: 1,
            max_runs_per_slot: 100,
            ..Default::default()
        })
        .build()
        .unwrap()
}

// ── Basic streaming ───────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn streaming_returns_receiver_before_script_starts() {
    LocalSet::new()
        .run_until(async {
            let mut sb = isolated_sandbox();
            // run_streaming is synchronous — no await needed to get the receiver.
            let (_fut, _rx) = sb.run_streaming("return 1");
            // If we reach here, the receiver was returned without blocking.
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn streaming_events_arrive_live() {
    LocalSet::new()
        .run_until(async {
            let mut sb = isolated_sandbox();
            let (fut, mut rx) = sb.run_streaming(
                r#"
                sandbox.emit("a", 1);
                sandbox.emit("b", 2);
                sandbox.emit("c", 3);
                return "done";
            "#,
            );

            // Spawn a local task to drain events while the script runs.
            let collector = tokio::task::spawn_local(async move {
                let mut events = vec![];
                while let Some(e) = rx.recv().await {
                    events.push((e.name, e.payload));
                }
                events
            });

            let result = fut.await.unwrap();
            let events = collector.await.unwrap();

            assert_eq!(result.value, json!("done"));
            // Events are NOT in SandboxResult.events when streaming.
            assert!(result.events.is_empty(), "SandboxResult.events must be empty when streaming");

            assert_eq!(events.len(), 3);
            assert_eq!(events[0], ("a".to_string(), json!(1)));
            assert_eq!(events[1], ("b".to_string(), json!(2)));
            assert_eq!(events[2], ("c".to_string(), json!(3)));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn streaming_result_value_correct() {
    LocalSet::new()
        .run_until(async {
            let mut sb = isolated_sandbox();
            let (fut, _rx) = sb.run_streaming("return 42");
            let result = fut.await.unwrap();
            assert_eq!(result.value, json!(42));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn streaming_logs_captured() {
    LocalSet::new()
        .run_until(async {
            let mut sb = isolated_sandbox();
            let (fut, _rx) = sb.run_streaming(
                r#"
                console.log("streaming log");
                return "ok";
            "#,
            );
            let result = fut.await.unwrap();
            assert!(
                result.logs.iter().any(|l| l.contains("streaming log")),
                "expected log line not found: {:?}",
                result.logs
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn streaming_no_events_receiver_closes_immediately() {
    LocalSet::new()
        .run_until(async {
            let mut sb = isolated_sandbox();
            let (fut, mut rx) = sb.run_streaming("return 0");

            let result = fut.await.unwrap();
            assert_eq!(result.value, json!(0));

            // After the future completes, the sender (event_tx) is dropped,
            // so the receiver returns None immediately.
            assert!(rx.recv().await.is_none(), "receiver should be closed after script ends");
        })
        .await;
}

// ── Event ordering ────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn streaming_events_arrive_in_emission_order() {
    LocalSet::new()
        .run_until(async {
            let mut sb = isolated_sandbox();
            let (fut, mut rx) = sb.run_streaming(
                r#"
                for (let i = 0; i < 10; i++) {
                    sandbox.emit("tick", i);
                }
                return "done";
            "#,
            );

            let result = fut.await.unwrap();
            assert_eq!(result.value, json!("done"));
            assert!(result.events.is_empty());

            let mut payloads = vec![];
            while let Some(e) = rx.recv().await {
                payloads.push(e.payload);
            }
            let expected: Vec<_> = (0i64..10).map(|i| json!(i)).collect();
            assert_eq!(payloads, expected);
        })
        .await;
}

// ── Metrics are populated ─────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn streaming_metrics_populated() {
    LocalSet::new()
        .run_until(async {
            let mut sb = isolated_sandbox();
            let (fut, _rx) = sb.run_streaming("return 1 + 1");
            let result = fut.await.unwrap();
            assert_eq!(result.value, json!(2));
            assert!(result.metrics.elapsed.as_nanos() > 0);
            assert!(result.metrics.peak_heap_bytes > 0);
        })
        .await;
}

// ── Error propagation ─────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn streaming_script_error_propagates() {
    LocalSet::new()
        .run_until(async {
            let mut sb = isolated_sandbox();
            let (fut, _rx) = sb.run_streaming("throw new Error('streaming boom')");
            let err = fut.await.unwrap_err();
            assert!(matches!(err, SandboxError::Runtime(_)), "expected Runtime error, got {err:?}");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn streaming_receiver_closes_on_error() {
    LocalSet::new()
        .run_until(async {
            let mut sb = isolated_sandbox();
            let (fut, mut rx) = sb.run_streaming(
                r#"
                sandbox.emit("before", 1);
                throw new Error("oops");
            "#,
            );

            // Drain events concurrently.
            let collector = tokio::task::spawn_local(async move {
                let mut events = vec![];
                while let Some(e) = rx.recv().await {
                    events.push(e.name);
                }
                events
            });

            let err = fut.await.unwrap_err();
            assert!(matches!(err, SandboxError::Runtime(_)));

            // The receiver closes when the sender (event_tx) is dropped after the run.
            let events = collector.await.unwrap();
            // The "before" event may or may not have been received depending on
            // scheduling, but the receiver must always close cleanly.
            let _ = events; // just verify it doesn't hang
        })
        .await;
}

// ── Warm pool slot ────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn streaming_works_on_warm_pool_slot() {
    LocalSet::new()
        .run_until(async {
            let mut sb = single_slot_sandbox();
            let (fut, mut rx) = sb.run_streaming(
                r#"
                sandbox.emit("warm", true);
                return "warm-slot";
            "#,
            );

            let result = fut.await.unwrap();
            assert_eq!(result.value, json!("warm-slot"));
            assert!(result.events.is_empty());

            // Events should be in the receiver.
            let event = rx.recv().await.unwrap();
            assert_eq!(event.name, "warm");
            assert_eq!(event.payload, json!(true));
            assert!(rx.recv().await.is_none()); // closed
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn streaming_and_run_interleave_correctly() {
    LocalSet::new()
        .run_until(async {
            let mut sb = single_slot_sandbox();

            // First run via run() — events in SandboxResult.events.
            let r1 = sb
                .run(
                    r#"
                sandbox.emit("batch", 1);
                return "batch";
            "#,
                )
                .await
                .unwrap();
            assert_eq!(r1.value, json!("batch"));
            assert_eq!(r1.events.len(), 1);
            assert_eq!(r1.events[0].name, "batch");

            // Second run via run_streaming() — events in receiver.
            let (fut, mut rx) = sb.run_streaming(
                r#"
                sandbox.emit("stream", 2);
                return "stream";
            "#,
            );
            let r2 = fut.await.unwrap();
            assert_eq!(r2.value, json!("stream"));
            assert!(r2.events.is_empty());

            let e = rx.recv().await.unwrap();
            assert_eq!(e.name, "stream");
            assert_eq!(e.payload, json!(2));
        })
        .await;
}

// ── Receiver is Send ─────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn streaming_receiver_is_send() {
    LocalSet::new()
        .run_until(async {
            let mut sb = isolated_sandbox();
            let (fut, rx) = sb.run_streaming(
                r#"
                sandbox.emit("hello", "world");
                return "ok";
            "#,
            );

            // rx is Send — can be moved to a regular tokio::spawn task.
            let handle = tokio::spawn(async move {
                let mut collected = vec![];
                let mut rx = rx;
                while let Some(e) = rx.recv().await {
                    collected.push(e.name);
                }
                collected
            });

            let result = fut.await.unwrap();
            assert_eq!(result.value, json!("ok"));

            let names = handle.await.unwrap();
            assert_eq!(names, vec!["hello"]);
        })
        .await;
}

// ── TypeScript works in streaming mode ───────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn streaming_typescript_works() {
    LocalSet::new()
        .run_until(async {
            let mut sb = isolated_sandbox();
            let (fut, mut rx) = sb.run_streaming(
                r#"
                const greet = (name: string): string => `hello ${name}`;
                sandbox.emit("greeting", greet("world"));
                return greet("sandbox");
            "#,
            );
            let result = fut.await.unwrap();
            assert_eq!(result.value, json!("hello sandbox"));

            let event = rx.recv().await.unwrap();
            assert_eq!(event.name, "greeting");
            assert_eq!(event.payload, json!("hello world"));
        })
        .await;
}
