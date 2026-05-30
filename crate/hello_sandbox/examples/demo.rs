//! `cargo run --example demo`
use hello_sandbox::{Sandbox, SandboxConfig, SandboxError};
use serde_json::json;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("hello_sandbox=debug").init();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            demo_trusted().await;
            demo_power_user().await;
            demo_typescript().await;
            demo_modules().await;
            demo_events().await;
            demo_timeout().await;
        })
        .await;

    Ok(())
}

// ── 1. Trusted ─────────────────────────────────────────────────────────────

async fn demo_trusted() {
    println!("\n=== Trusted ===");
    let mut sb = Sandbox::new(SandboxConfig::trusted()).unwrap();
    sb.set_input("items", json!([10, 20, 30, 40]));

    let result = sb
        .run(
            r#"
            const items = sandbox.readInput("items");
            const total = items.reduce((a, b) => a + b, 0);
            console.log("total:", total);
            return { total, avg: total / items.length };
            "#,
        )
        .await
        .unwrap();

    print_result(&result);
}

// ── 2. PowerUser ───────────────────────────────────────────────────────────

async fn demo_power_user() {
    println!("\n=== PowerUser ===");
    let mut sb = Sandbox::new(SandboxConfig::power_user()).unwrap();

    // Run the SAME runtime twice to show scope isolation.
    for i in 0..2 {
        sb.set_input("run_id", json!(i));
        let result = sb
            .run(
                r#"
                // `myVar` is module-level; each run gets its own module record.
                const myVar = sandbox.readInput("run_id");
                console.log("run_id:", myVar);
                return myVar * 100;
                "#,
            )
            .await
            .unwrap();
        print_result(&result);
    }
}

// ── 3. TypeScript ──────────────────────────────────────────────────────────

async fn demo_typescript() {
    println!("\n=== TypeScript ===");
    let mut sb = Sandbox::new(SandboxConfig::power_user()).unwrap();
    sb.set_input("name", json!("Alice"));

    let result = sb
        .run(
            r#"
            interface Greeting { message: string; length: number }
            const name: string = sandbox.readInput("name") as string;
            const greeting: Greeting = {
                message: `Hello, ${name}!`,
                length: name.length,
            };
            console.log(greeting.message);
            return greeting;
            "#,
        )
        .await
        .unwrap();

    print_result(&result);
}

// ── 4. Module imports ──────────────────────────────────────────────────────

async fn demo_modules() {
    println!("\n=== Modules ===");
    let mut sb = Sandbox::new(SandboxConfig::power_user()).unwrap();

    // Register a utility module (TypeScript).
    sb.register_module(
        "sandbox:math",
        r#"
        export const clamp = (v: number, lo: number, hi: number): number =>
            Math.min(Math.max(v, lo), hi);

        export const sum = (arr: number[]): number =>
            arr.reduce((a, b) => a + b, 0);
        "#,
    );

    sb.set_input("values", json!([-5, 10, 150, 42, 3]));

    let result = sb
        .run(
            r#"
            import { clamp, sum } from "sandbox:math";
            const raw: number[] = sandbox.readInput("values") as number[];
            const clamped = raw.map(v => clamp(v, 0, 100));
            console.log("clamped:", JSON.stringify(clamped));
            return { clamped, total: sum(clamped) };
            "#,
        )
        .await
        .unwrap();

    print_result(&result);
}

// ── 5. Events (streaming progress) ────────────────────────────────────────

async fn demo_events() {
    println!("\n=== Events (streaming) ===");
    let mut sb = Sandbox::new(SandboxConfig::power_user()).unwrap();
    sb.set_input("steps", json!(5));

    let result = sb
        .run(
            r#"
            const steps: number = sandbox.readInput("steps") as number;
            for (let i = 1; i <= steps; i++) {
                sandbox.emit("progress", { step: i, total: steps, pct: (i / steps * 100) | 0 });
            }
            return { done: true };
            "#,
        )
        .await
        .unwrap();

    println!("  Events received during run:");
    for ev in &result.events {
        println!("    [{:>4}ms] {} -> {}", ev.timestamp_ms, ev.name, ev.payload);
    }
    print_result(&result);
}

// ── 6. Timeout enforcement ─────────────────────────────────────────────────

async fn demo_timeout() {
    println!("\n=== Timeout (PowerUser watchdog) ===");
    let mut config = SandboxConfig::power_user();
    config.timeout = std::time::Duration::from_millis(300);

    let mut sb = Sandbox::new(config).unwrap();

    match sb.run("while (true) {}").await {
        Err(SandboxError::Timeout(d)) => println!("  Timed out after {:?} as expected.", d),
        other => println!("  Unexpected: {:?}", other),
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

fn print_result(r: &hello_sandbox::SandboxResult) {
    for line in &r.logs {
        println!("  [log] {line}");
    }
    println!("  value   = {}", r.value);
    println!("  elapsed = {:?}", r.elapsed);
}
