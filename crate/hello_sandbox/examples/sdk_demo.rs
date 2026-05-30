//! Phase 5 demo -- `cargo run --example sdk`
//!
//! Demonstrates opt-in SDK packs:
//!   1. KvPack  -- per-slot key-value store
//!   2. CryptoPack -- hash, randomBytes, randomUUID
//!   3. HttpPack -- outbound fetch (allowlist-gated, rejection demo)
//!   4. Multiple packs together

use std::collections::HashMap;

use hello_sandbox::loader::AllowlistModuleLoader;
use hello_sandbox::runtime::SharedRuntime;
use hello_sandbox::sdk::core_sdk::CorePack;
use hello_sandbox::sdk::crypto_sdk::CryptoPack;
use hello_sandbox::sdk::http_sdk::{HttpConfig, HttpPack};
use hello_sandbox::sdk::kv_sdk::KvPack;
use hello_sandbox::sdk::SdkRegistry;
use hello_sandbox::{RunCapabilities, SandboxConfig, SandboxError, SandboxEvent};
use serde_json::json;
use tokio::sync::mpsc;

fn null_tx() -> mpsc::UnboundedSender<SandboxEvent> {
    mpsc::unbounded_channel().0
}

fn caps() -> RunCapabilities {
    RunCapabilities::default()
}

#[tokio::main]
async fn main() {
    println!("=== Phase 5: SDK Packs ===\n");

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            demo_kv().await;
            demo_crypto().await;
            demo_http().await;
            demo_all_together().await;
        })
        .await;
}

// ─── 1. KV Pack ───────────────────────────────────────────────────────────────

async fn demo_kv() {
    println!("--- 1. KvPack ---");
    let loader = AllowlistModuleLoader::new();
    let sdk = SdkRegistry::empty().register(CorePack).register(KvPack::default());
    let mut rt = SharedRuntime::new(SandboxConfig::trusted(), loader, &sdk);

    // Run 1: write counter
    rt.run(
        r#"
            import { kv } from "sandbox:kv";
            await kv.set("counter", 0);
            console.log("initialised counter");
        "#,
        HashMap::new(),
        null_tx(),
        caps(),
    )
    .await
    .unwrap();

    // Runs 2-4: increment -- KV state persists across runs on the same runtime
    for _ in 0..3 {
        let (val, _, _) = rt
            .run(
                r#"
                import { kv } from "sandbox:kv";
                const n = (await kv.get("counter")) + 1;
                await kv.set("counter", n);
                return n;
                "#,
                HashMap::new(),
                null_tx(),
                caps(),
            )
            .await
            .unwrap();
        println!("  counter = {val}");
    }

    // List keys with "counter" prefix
    let (keys, _, _) = rt
        .run(
            r#"
            import { kv } from "sandbox:kv";
            return await kv.list("counter");
            "#,
            HashMap::new(),
            null_tx(),
            caps(),
        )
        .await
        .unwrap();
    println!("  keys: {keys}");
    println!("  ok\n");
}

// ─── 2. Crypto Pack ───────────────────────────────────────────────────────────

async fn demo_crypto() {
    println!("--- 2. CryptoPack ---");
    let loader = AllowlistModuleLoader::new();
    let sdk = SdkRegistry::empty().register(CorePack).register(CryptoPack);
    let mut rt = SharedRuntime::new(SandboxConfig::trusted(), loader, &sdk);

    let (val, _, _) = rt
        .run(
            r#"
            import { crypto } from "sandbox:crypto";

            const digest = await crypto.hash("sha256", "hello world");
            const uuid   = crypto.randomUUID();
            const bytes  = crypto.randomBytes(8);

            console.log("sha256 of 'hello world':", digest);
            console.log("uuid:", uuid);
            console.log("8 random bytes:", Array.from(bytes).join(","));

            return { digest, uuid, bytesLen: bytes.length };
            "#,
            HashMap::new(),
            null_tx(),
            caps(),
        )
        .await
        .unwrap();

    println!("  sha256: {}", val["digest"]);
    println!("  uuid:   {}", val["uuid"]);
    println!("  bytes:  {} items", val["bytesLen"]);

    // Verify digest length (64 hex chars for SHA-256)
    assert_eq!(val["digest"].as_str().unwrap().len(), 64, "SHA-256 digest must be 64 hex chars");
    assert_eq!(val["bytesLen"], json!(8));
    println!("  ok\n");
}

// ─── 3. HTTP Pack (allowlist) ─────────────────────────────────────────────────

async fn demo_http() {
    println!("--- 3. HttpPack ---");
    let loader = AllowlistModuleLoader::new();
    let sdk = SdkRegistry::empty().register(CorePack).register(HttpPack::new(HttpConfig {
        allowed_prefixes: vec!["https://allowed.example.com/".into()],
        ..Default::default()
    }));
    let mut rt = SharedRuntime::new(SandboxConfig::trusted(), loader, &sdk);

    // Blocked URL
    let err = rt
        .run(
            r#"
            import { fetch } from "sandbox:http";
            await fetch("https://evil.example.com/steal");
            "#,
            HashMap::new(),
            null_tx(),
            caps(),
        )
        .await
        .unwrap_err();
    match &err {
        SandboxError::Runtime(e) => println!("  blocked (expected): {e}"),
        other => panic!("unexpected: {other:?}"),
    }
    println!("  ok\n");
}

// ─── 4. All packs together ────────────────────────────────────────────────────

async fn demo_all_together() {
    println!("--- 4. All packs together ---");
    let loader = AllowlistModuleLoader::new();
    let sdk =
        SdkRegistry::empty().register(CorePack).register(KvPack::default()).register(CryptoPack);
    let mut rt = SharedRuntime::new(SandboxConfig::trusted(), loader, &sdk);

    let (tx, mut rx) = mpsc::unbounded_channel::<SandboxEvent>();

    let (val, logs, _) = rt
        .run(
            r#"
            import { kv }     from "sandbox:kv";
            import { crypto } from "sandbox:crypto";

            const user = sandbox.readInput("user");
            const id   = crypto.randomUUID();

            await kv.set(`session:${id}`, { user, created: Date.now() });
            const session = await kv.get(`session:${id}`);

            sandbox.emit("session_created", { id, user });
            console.log("created session for", user);

            return { id: id.slice(0, 8) + "...", user };
            "#,
            {
                let mut m = HashMap::new();
                m.insert("user".to_string(), json!("Alice"));
                m
            },
            tx,
            caps(),
        )
        .await
        .unwrap();

    println!("  value : {val}");
    println!("  logs  : {logs:?}");

    let events: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    println!("  events: {}", events.len());

    assert_eq!(val["user"], json!("Alice"));
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].name, "session_created");
    println!("  ok\n");

    println!("All Phase 5 assertions passed.");
}
