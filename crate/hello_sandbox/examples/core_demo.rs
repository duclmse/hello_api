//! Phase 3 demo — `cargo run --example core`
//!
//! Demonstrates the Core SDK pack + SharedRuntime bootstrap:
//!   - console.log / info / warn / error captured in logs
//!   - sandbox.readInput() reads host-provided values
//!   - sandbox.emit() sends events through the mpsc channel
//!   - globalThis.Deno is deleted after bootstrap
//!   - globalThis is frozen (evil assignment throws TypeError)
//!   - Prototype pollution guard: Array.prototype.map cannot be overwritten

use std::collections::HashMap;

use hello_sandbox::loader::AllowlistModuleLoader;
use hello_sandbox::runtime::SharedRuntime;
use hello_sandbox::sdk::core_sdk::CorePack;
use hello_sandbox::sdk::SdkRegistry;
use hello_sandbox::{RunCapabilities, SandboxConfig, SandboxEvent};
use serde_json::{json, Value};
use tokio::sync::mpsc;

#[tokio::main]
async fn main() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            demo_console().await;
            demo_read_input().await;
            demo_emit().await;
            demo_deno_deleted().await;
            demo_frozen_global().await;
            demo_prototype_frozen().await;
        })
        .await;
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn make_runtime() -> SharedRuntime {
    let loader = AllowlistModuleLoader::new();
    let sdk = SdkRegistry::empty().register(CorePack);
    SharedRuntime::new(SandboxConfig::trusted(), loader, &sdk)
}

fn null_channel() -> mpsc::UnboundedSender<SandboxEvent> {
    let (tx, _rx) = mpsc::unbounded_channel();
    tx
}

// ── 1. console.* captured in logs ────────────────────────────────────────────

async fn demo_console() {
    println!("=== 1. console.* captured in logs ===");
    let mut rt = make_runtime();

    let (value, logs, _) = rt
        .run(
            r#"
            console.log("hello from log");
            console.info("info line");
            console.warn("warn line");
            console.error("error line");
            console.debug("debug line");
            return 42;
        "#,
            HashMap::new(),
            null_channel(),
            RunCapabilities::default(),
        )
        .await
        .unwrap();

    println!("  value:  {value}");
    println!("  logs:   {logs:?}");
    assert_eq!(value, json!(42));
    assert!(logs.iter().any(|l| l.contains("hello from log")));
    assert!(logs.iter().any(|l| l.contains("INFO")));
    assert!(logs.iter().any(|l| l.contains("WARN")));
    assert!(logs.iter().any(|l| l.contains("ERROR")));
    assert!(logs.iter().any(|l| l.contains("DEBUG")));
    println!("  ok\n");
}

// ── 2. sandbox.readInput() ────────────────────────────────────────────────────

async fn demo_read_input() {
    println!("=== 2. sandbox.readInput() ===");
    let mut rt = make_runtime();

    let mut inputs = HashMap::new();
    inputs.insert("name".to_string(), json!("Alice"));
    inputs.insert("count".to_string(), json!(7));

    let (value, _logs, _) = rt
        .run(
            r#"
            const name  = sandbox.readInput("name");
            const count = sandbox.readInput("count");
            const missing = sandbox.readInput("not_set");
            return { name, count, missing };
        "#,
            inputs,
            null_channel(),
            RunCapabilities::default(),
        )
        .await
        .unwrap();

    println!("  value: {value}");
    assert_eq!(value["name"], json!("Alice"));
    assert_eq!(value["count"], json!(7));
    assert_eq!(value["missing"], Value::Null);
    println!("  ok\n");
}

// ── 3. sandbox.emit() sends events ───────────────────────────────────────────

async fn demo_emit() {
    println!("=== 3. sandbox.emit() events ===");
    let mut rt = make_runtime();

    let (tx, mut rx) = mpsc::unbounded_channel::<SandboxEvent>();
    rt.run(
        r#"
            sandbox.emit("started",  { step: 1 });
            sandbox.emit("progress", { step: 2, pct: 50 });
            sandbox.emit("done",     { step: 3 });
        "#,
        HashMap::new(),
        tx,
        RunCapabilities::default(),
    )
    .await
    .unwrap();

    // Drain the event channel.
    let mut events = Vec::new();
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }

    println!("  received {} events", events.len());
    for e in &events {
        println!("  event: {} — {}", e.name, e.payload);
    }
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].name, "started");
    assert_eq!(events[1].name, "progress");
    assert_eq!(events[2].name, "done");
    println!("  ok\n");
}

// ── 4. Deno is deleted after bootstrap ────────────────────────────────────────

async fn demo_deno_deleted() {
    println!("=== 4. globalThis.Deno deleted after bootstrap ===");
    let mut rt = make_runtime();

    let (value, _logs, _) = rt
        .run(
            r#"
            return typeof globalThis.Deno;
        "#,
            HashMap::new(),
            null_channel(),
            RunCapabilities::default(),
        )
        .await
        .unwrap();

    println!("  typeof globalThis.Deno = {value}");
    assert_eq!(value, json!("undefined"), "Deno must be deleted");
    println!("  ok\n");
}

// ── 5. globalThis is frozen ───────────────────────────────────────────────────

async fn demo_frozen_global() {
    println!("=== 5. globalThis frozen — assignment throws TypeError ===");
    let mut rt = make_runtime();

    // Assigning to a frozen globalThis should throw TypeError.
    let err = rt
        .run(
            r#"
            "use strict";
            globalThis.evil = 1;
            "#,
            HashMap::new(),
            null_channel(),
            RunCapabilities::default(),
        )
        .await;

    match &err {
        Err(e) => println!("  error (expected): {e}"),
        Ok(_) => panic!("expected an error but script succeeded"),
    }
    assert!(err.is_err(), "frozen globalThis must reject new properties");
    println!("  ok\n");
}

// ── 6. Prototype pollution guard ──────────────────────────────────────────────

async fn demo_prototype_frozen() {
    println!("=== 6. Prototype pollution guard ===");
    let mut rt = make_runtime();

    // Attempting to overwrite Array.prototype.map should throw.
    let err = rt
        .run(
            r#"
            "use strict";
            Array.prototype.map = () => "pwned";
            "#,
            HashMap::new(),
            null_channel(),
            RunCapabilities::default(),
        )
        .await;

    match &err {
        Err(e) => println!("  error (expected): {e}"),
        Ok(_) => panic!("expected an error but script succeeded"),
    }
    assert!(err.is_err(), "frozen prototype must reject assignment");
    println!("  ok\n");

    // A second run on the SAME runtime should still have the original map.
    let (value, _logs, _) = rt
        .run(
            r#"
            return [1, 2, 3].map(x => x * 2);
        "#,
            HashMap::new(),
            null_channel(),
            RunCapabilities::default(),
        )
        .await
        .unwrap();

    println!("  [1,2,3].map(x=>x*2) = {value}");
    assert_eq!(value, json!([2, 4, 6]));
    println!("  ok\n");
}
