//! Phase 4 demo — `cargo run --example runtime`
//!
//! Demonstrates the complete `SharedRuntime` execution pipeline:
//!   1. Plain JS return value
//!   2. TypeScript script (type annotations stripped)
//!   3. sandbox.readInput() — host-injected values
//!   4. sandbox.emit()      — streaming events to host
//!   5. console.log captured in logs, not in return value
//!   6. Script error → SandboxError::Runtime
//!   7. Infinite loop → SandboxError::Timeout
//!   8. Module scope isolation across sequential runs

use std::collections::HashMap;
use std::time::{Duration, Instant};

use hello_sandbox::loader::AllowlistModuleLoader;
use hello_sandbox::runtime::SharedRuntime;
use hello_sandbox::sdk::core_sdk::CorePack;
use hello_sandbox::sdk::SdkRegistry;
use hello_sandbox::{RunCapabilities, SandboxConfig, SandboxError, SandboxEvent};
use serde_json::{json, Value};
use tokio::sync::mpsc;

// ─── Helpers ─────────────────────────────────────────────────────────────────

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

// ─── Entry point ─────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    println!("=== Phase 4: SharedRuntime ===\n");

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            demo_plain_js().await;
            demo_typescript().await;
            demo_read_input().await;
            demo_emit().await;
            demo_logs_vs_value().await;
            demo_error().await;
            demo_timeout().await;
            demo_scope_isolation().await;
        })
        .await;
}

// ─── 1. Plain JS return value ────────────────────────────────────────────────

async fn demo_plain_js() {
    println!("--- 1. Plain JS ---");
    let mut rt = make_runtime();

    let (val, logs, _) = rt
        .run(
            r#"
            const x = 6;
            const y = 7;
            return x * y;
        "#,
            HashMap::new(),
            null_tx(),
            caps(),
        )
        .await
        .unwrap();

    println!("  return value : {val}");
    println!("  logs         : {logs:?}");
    assert_eq!(val, json!(42));
    assert!(logs.is_empty());
    println!("  ok\n");
}

// ─── 2. TypeScript ───────────────────────────────────────────────────────────

async fn demo_typescript() {
    println!("--- 2. TypeScript ---");
    let mut rt = make_runtime();

    let (val, _, _) = rt
        .run(
            r#"
            interface Point { x: number; y: number; }
            const dist = (p: Point): number =>
                Math.sqrt(p.x ** 2 + p.y ** 2);
            return dist({ x: 3, y: 4 });
        "#,
            HashMap::new(),
            null_tx(),
            caps(),
        )
        .await
        .unwrap();

    println!("  dist(3, 4) = {val}");
    // Math.sqrt(25) = 5, serialised as integer by V8.
    assert!(val == json!(5) || val == json!(5.0));
    println!("  ok\n");
}

// ─── 3. sandbox.readInput() ──────────────────────────────────────────────────

async fn demo_read_input() {
    println!("--- 3. sandbox.readInput() ---");
    let mut rt = make_runtime();

    let (val, _, _) = rt
        .run(
            r#"
            const name    = sandbox.readInput("name");
            const numbers = sandbox.readInput("numbers");
            const missing = sandbox.readInput("nope");
            return { name, numbers, missing };
        "#,
            inputs(&[("name", json!("Alice")), ("numbers", json!([1, 2, 3]))]),
            null_tx(),
            caps(),
        )
        .await
        .unwrap();

    println!("  name    = {}", val["name"]);
    println!("  numbers = {}", val["numbers"]);
    println!("  missing = {}", val["missing"]);
    assert_eq!(val["name"], json!("Alice"));
    assert_eq!(val["numbers"], json!([1, 2, 3]));
    assert_eq!(val["missing"], Value::Null);
    println!("  ok\n");
}

// ─── 4. sandbox.emit() ───────────────────────────────────────────────────────

async fn demo_emit() {
    println!("--- 4. sandbox.emit() ---");
    let mut rt = make_runtime();
    let (tx, mut rx) = mpsc::unbounded_channel::<SandboxEvent>();

    let (val, _, _) = rt
        .run(
            r#"
            for (let i = 1; i <= 3; i++) {
                sandbox.emit("step", { i, done: i === 3 });
            }
            return "done";
        "#,
            HashMap::new(),
            tx,
            caps(),
        )
        .await
        .unwrap();

    assert_eq!(val, json!("done"));

    let mut events = Vec::new();
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }
    println!("  {} events received:", events.len());
    for e in &events {
        println!("    {} — {}", e.name, e.payload);
    }
    assert_eq!(events.len(), 3);
    assert_eq!(events[2].payload["done"], json!(true));
    println!("  ok\n");
}

// ─── 5. console.log in logs, not in return value ─────────────────────────────

async fn demo_logs_vs_value() {
    println!("--- 5. Logs vs return value ---");
    let mut rt = make_runtime();

    let (val, logs, _) = rt
        .run(
            r#"
            console.log("step 1");
            console.info("step 2");
            console.warn("step 3");
            return { status: "ok", steps: 3 };
        "#,
            HashMap::new(),
            null_tx(),
            caps(),
        )
        .await
        .unwrap();

    println!("  value : {val}");
    println!("  logs  : {logs:?}");

    // Logs have the console lines, not the return value.
    assert!(logs.iter().any(|l| l.contains("step 1")));
    assert!(logs.iter().any(|l| l.contains("step 2")));
    assert!(logs.iter().any(|l| l.contains("step 3")));
    assert_eq!(val["status"], json!("ok"));

    // The sentinel is stripped from logs.
    assert!(!logs.iter().any(|l| l.starts_with("__RETURN__:")));
    println!("  ok\n");
}

// ─── 6. Script error ─────────────────────────────────────────────────────────

async fn demo_error() {
    println!("--- 6. Script error → SandboxError::Runtime ---");
    let mut rt = make_runtime();

    let err = rt
        .run(r#"throw new TypeError("bad input");"#, HashMap::new(), null_tx(), caps())
        .await
        .unwrap_err();

    match &err {
        SandboxError::Runtime(e) => println!("  ✓ Runtime error: {e}"),
        other => panic!("unexpected error type: {other:?}"),
    }
    println!("  ok\n");
}

// ─── 7. Timeout ──────────────────────────────────────────────────────────────

async fn demo_timeout() {
    println!("--- 7. Infinite loop → SandboxError::Timeout ---");

    let mut config = SandboxConfig::power_user();
    config.timeout = Duration::from_millis(300);
    let deadline = config.timeout;

    let mut rt = make_runtime_with_config(config);

    let start = Instant::now();
    let err = rt.run("while (true) {}", HashMap::new(), null_tx(), caps()).await.unwrap_err();
    let elapsed = start.elapsed();

    match &err {
        SandboxError::Timeout(d) => println!("  ✓ Timeout after {d:?}  (wall: {elapsed:?})"),
        other => panic!("expected Timeout, got: {other:?}"),
    }
    assert!(elapsed < deadline + Duration::from_millis(500), "took too long: {elapsed:?}");
    println!("  ok\n");
}

// ─── 8. Module scope isolation ───────────────────────────────────────────────

async fn demo_scope_isolation() {
    println!("--- 8. Module scope isolation ---");
    let mut rt = make_runtime();

    // Run 1: define a module-local constant.
    let (val1, _, _) = rt
        .run(
            r#"
            const secret = "run-1-only";
            return secret;
        "#,
            HashMap::new(),
            null_tx(),
            caps(),
        )
        .await
        .unwrap();
    assert_eq!(val1, json!("run-1-only"));

    // Run 2: `secret` must not be reachable — different module scope.
    let (val2, _, _) =
        rt.run("return typeof secret;", HashMap::new(), null_tx(), caps()).await.unwrap();
    println!("  typeof secret in run 2 = {val2}");
    assert_eq!(val2, json!("undefined"), "scope isolation failed");

    println!("  run_count() = {}", rt.run_count());
    assert_eq!(rt.run_count(), 2);
    println!("  ok\n");

    println!("All Phase 4 assertions passed.");
}
