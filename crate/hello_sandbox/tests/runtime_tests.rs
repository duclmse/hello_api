//! Phase 4 integration tests for `SharedRuntime`.
//!
//! All tests must run inside a `LocalSet` because `JsRuntime` is `!Send`.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use hello_sandbox::loader::AllowlistModuleLoader;
use hello_sandbox::runtime::SharedRuntime;
use hello_sandbox::sdk::core_sdk::CorePack;
use hello_sandbox::sdk::SdkRegistry;
use hello_sandbox::{RunCapabilities, SandboxConfig, SandboxError, SandboxEvent};
use serde_json::{json, Value};
use tokio::sync::mpsc;

// ─── Test helpers ─────────────────────────────────────────────────────────────

fn make_runtime() -> SharedRuntime {
    make_runtime_with_config(SandboxConfig::trusted())
}

fn make_runtime_with_config(config: SandboxConfig) -> SharedRuntime {
    let loader = AllowlistModuleLoader::new();
    let sdk = SdkRegistry::empty().register(CorePack);
    SharedRuntime::new(config, loader, &sdk)
}

fn null_tx() -> mpsc::UnboundedSender<SandboxEvent> {
    mpsc::unbounded_channel().0
}

fn inputs(pairs: &[(&str, Value)]) -> HashMap<String, Value> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
}

fn caps() -> RunCapabilities {
    RunCapabilities::default()
}

// ─── 1. Plain JS returns correct value ───────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn plain_js_returns_value() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut rt = make_runtime();
            let (val, logs, _) =
                rt.run("return 42;", HashMap::new(), null_tx(), caps()).await.unwrap();
            assert_eq!(val, json!(42));
            assert!(logs.is_empty(), "no user logs expected");
        })
        .await;
}

// ─── 2. TypeScript transpiles and runs ───────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn typescript_transpiles_and_runs() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut rt = make_runtime();
            let (val, _, _) = rt
                .run(
                    r#"
                    const greet = (name: string): string => `Hello, ${name}!`;
                    return greet("World");
                    "#,
                    HashMap::new(),
                    null_tx(),
                    caps(),
                )
                .await
                .unwrap();
            assert_eq!(val, json!("Hello, World!"), "got: {val}");
        })
        .await;
}

// ─── 3. sandbox.readInput() returns host-provided value ──────────────────────

#[tokio::test(flavor = "current_thread")]
async fn read_input_returns_host_value() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut rt = make_runtime();
            let (val, _, _) = rt
                .run(
                    r#"
                    const x = sandbox.readInput("x");
                    const y = sandbox.readInput("y");
                    const missing = sandbox.readInput("nope");
                    return { x, y, missing };
                    "#,
                    inputs(&[("x", json!(10)), ("y", json!("hello"))]),
                    null_tx(),
                    caps(),
                )
                .await
                .unwrap();
            assert_eq!(val["x"], json!(10));
            assert_eq!(val["y"], json!("hello"));
            assert_eq!(val["missing"], Value::Null);
        })
        .await;
}

// ─── 4. sandbox.emit() sends events to the host channel ──────────────────────

#[tokio::test(flavor = "current_thread")]
async fn emit_sends_events_to_host() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut rt = make_runtime();
            let (tx, mut rx) = mpsc::unbounded_channel::<SandboxEvent>();

            rt.run(
                r#"
                    sandbox.emit("alpha", { v: 1 });
                    sandbox.emit("beta",  { v: 2 });
                "#,
                HashMap::new(),
                tx,
                caps(),
            )
            .await
            .unwrap();

            let mut events = Vec::new();
            while let Ok(e) = rx.try_recv() {
                events.push(e);
            }

            assert_eq!(events.len(), 2);
            assert_eq!(events[0].name, "alpha");
            assert_eq!(events[0].payload, json!({ "v": 1 }));
            assert_eq!(events[1].name, "beta");
            assert_eq!(events[1].payload, json!({ "v": 2 }));
        })
        .await;
}

// ─── 5. console.log appears in logs, not in the return value ─────────────────

#[tokio::test(flavor = "current_thread")]
async fn console_log_in_logs_not_value() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut rt = make_runtime();
            let (val, logs, _) = rt
                .run(
                    r#"
                    console.log("captured");
                    return "result";
                    "#,
                    HashMap::new(),
                    null_tx(),
                    caps(),
                )
                .await
                .unwrap();

            // The log line appears in logs.
            assert!(logs.iter().any(|l| l.contains("captured")));
            // It does NOT appear in the return value.
            assert_eq!(val, json!("result"));
            // The sentinel (__RETURN__:...) is NOT in logs.
            assert!(!logs.iter().any(|l| l.starts_with("__RETURN__:")));
        })
        .await;
}

// ─── 6. throw propagates as SandboxError::Runtime ────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn throw_returns_runtime_error() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut rt = make_runtime();
            let err = rt
                .run("throw new Error(\"boom\");", HashMap::new(), null_tx(), caps())
                .await
                .unwrap_err();

            match err {
                SandboxError::Runtime(e) => {
                    assert!(
                        e.to_string().contains("boom"),
                        "error message should mention 'boom'; got: {e}"
                    );
                },
                other => panic!("expected Runtime error, got: {other:?}"),
            }
        })
        .await;
}

// ─── 7. Infinite loop returns SandboxError::Timeout ──────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn infinite_loop_times_out() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // Use a very short deadline so the test doesn't hang.
            let mut config = SandboxConfig::power_user();
            config.timeout = Duration::from_millis(300);

            let mut rt = make_runtime_with_config(config.clone());

            let start = Instant::now();
            let err =
                rt.run("while (true) {}", HashMap::new(), null_tx(), caps()).await.unwrap_err();
            let elapsed = start.elapsed();

            match err {
                SandboxError::Timeout(d) => {
                    assert_eq!(d, config.timeout, "timeout duration should match config");
                },
                other => panic!("expected Timeout error, got: {other:?}"),
            }

            // Should have terminated close to the deadline (±500 ms slack).
            assert!(
                elapsed < config.timeout + Duration::from_millis(500),
                "run took too long: {elapsed:?}"
            );
        })
        .await;
}

// ─── 8. Module scope isolation across sequential runs ────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn sequential_runs_have_isolated_scopes() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut rt = make_runtime();

            // Run 1: define a module-level constant and return its value.
            let (val1, _, _) = rt
                .run("const magic = 42; return magic;", HashMap::new(), null_tx(), caps())
                .await
                .unwrap();
            assert_eq!(val1, json!(42));

            // Run 2: `magic` must not be visible — each run is a fresh module scope.
            let (val2, _, _) =
                rt.run("return typeof magic;", HashMap::new(), null_tx(), caps()).await.unwrap();
            assert_eq!(val2, json!("undefined"), "variable from run 1 must not leak into run 2");

            // run_count() should reflect both successful runs.
            assert_eq!(rt.run_count(), 2);
        })
        .await;
}

// ─── 9. run_count() increments correctly ─────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn run_count_increments_per_run() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut rt = make_runtime();
            assert_eq!(rt.run_count(), 0);

            for i in 1..=3 {
                rt.run("return 1;", HashMap::new(), null_tx(), caps()).await.unwrap();
                assert_eq!(rt.run_count(), i);
            }
        })
        .await;
}

// ─── 10. JSON return values round-trip correctly ──────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn json_return_value_roundtrip() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut rt = make_runtime();

            let (val, _, _) = rt
                .run(
                    r#"
                    return {
                        num:  1.618,
                        str:  "hello",
                        arr:  [1, 2, 3],
                        bool: true,
                        nil:  null,
                        nested: { a: { b: 42 } },
                    };
                    "#,
                    HashMap::new(),
                    null_tx(),
                    caps(),
                )
                .await
                .unwrap();

            assert_eq!(val["num"], json!(1.618));
            assert_eq!(val["str"], json!("hello"));
            assert_eq!(val["arr"], json!([1, 2, 3]));
            assert_eq!(val["bool"], json!(true));
            assert_eq!(val["nil"], Value::Null);
            assert_eq!(val["nested"]["a"]["b"], json!(42));
        })
        .await;
}
