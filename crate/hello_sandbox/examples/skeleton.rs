//! Phase 0 skeleton demo — `cargo run --example skeleton`
//!
//! Demonstrates the fully-implemented data types exported by the library:
//!   - `SandboxConfig` constructors and field inspection
//!   - `IsolationLevel` comparisons
//!   - `SandboxEvent` construction and JSON serialisation
//!   - `SandboxError` display formatting
//!
//! All execution modules (`Sandbox`, `SharedRuntime`, `AllowlistModuleLoader`,
//! `transpile`) are Phase 0 stubs; this demo exercises only the types that are
//! already complete.

use hello_sandbox::{IsolationLevel, SandboxConfig, SandboxError, SandboxEvent};
use serde_json::json;
use std::time::Duration;

fn main() {
    // ── 1. SandboxConfig constructors ────────────────────────────────────────
    println!("=== SandboxConfig ===\n");

    let configs = [
        ("trusted", SandboxConfig::trusted()),
        ("power_user", SandboxConfig::power_user()),
        ("untrusted", SandboxConfig::untrusted()),
    ];

    for (name, cfg) in &configs {
        println!("  [{name}]");
        println!("    isolation      = {:?}", cfg.isolation);
        println!("    timeout        = {:?}", cfg.timeout);
        println!("    heap_max       = {} MB", cfg.heap_max_bytes / (1024 * 1024));
        println!("    max_log_lines  = {}", cfg.max_log_lines);
        println!("    allow_modules  = {}", cfg.allow_modules);
        println!("    allow_ts       = {}", cfg.allow_typescript);
        println!("    allow_events   = {}", cfg.allow_events);
        println!();
    }

    // ── 2. IsolationLevel ordering ───────────────────────────────────────────
    println!("=== IsolationLevel ===\n");

    let level = IsolationLevel::PowerUser;
    println!("  default level    = {:?}", IsolationLevel::default());
    println!("  is power_user?   = {}", level == IsolationLevel::PowerUser);
    println!("  is untrusted?    = {}", level == IsolationLevel::Untrusted);
    println!();

    // ── 3. Runtime config mutation ───────────────────────────────────────────
    println!("=== Config customisation ===\n");

    let mut custom = SandboxConfig::power_user();
    custom.timeout = Duration::from_millis(500);
    custom.max_log_lines = 50;
    custom.allow_events = false;

    println!("  custom timeout       = {:?}", custom.timeout);
    println!("  custom max_log_lines = {}", custom.max_log_lines);
    println!("  custom allow_events  = {}", custom.allow_events);
    println!();

    // ── 4. SandboxEvent construction & serialisation ─────────────────────────
    println!("=== SandboxEvent ===\n");

    let events = vec![
        SandboxEvent::new("start", json!({"msg": "beginning computation"}), 0),
        SandboxEvent::new("progress", json!({"step": 1, "total": 3, "pct": 33}), 12),
        SandboxEvent::new("progress", json!({"step": 2, "total": 3, "pct": 66}), 24),
        SandboxEvent::new("progress", json!({"step": 3, "total": 3, "pct": 100}), 38),
        SandboxEvent::new("result", json!({"answer": 42}), 42),
    ];

    for ev in &events {
        let serialised = serde_json::to_string(ev).unwrap();
        println!("  [{:>4}ms] {:10} → {}", ev.timestamp_ms, ev.name, ev.payload);
        println!("           json = {serialised}");
    }
    println!();

    // Round-trip through JSON.
    let first_json = serde_json::to_string(&events[0]).unwrap();
    let round_tripped: SandboxEvent = serde_json::from_str(&first_json).unwrap();
    assert_eq!(round_tripped.name, events[0].name);
    assert_eq!(round_tripped.payload, events[0].payload);
    println!("  JSON round-trip OK for '{}'", events[0].name);
    println!();

    // ── 5. SandboxError display ──────────────────────────────────────────────
    println!("=== SandboxError ===\n");

    let errors: &[SandboxError] = &[
        SandboxError::Timeout(Duration::from_secs(5)),
        SandboxError::OutOfMemory,
        SandboxError::QuotaExceeded(1_000),
        SandboxError::ModuleNotFound("sandbox:missing/module".into()),
        SandboxError::TranspileError("unexpected token `}`".into()),
        SandboxError::ChildProcess("worker exited with status 1".into()),
    ];

    for err in errors {
        println!("  {err}");
    }
    println!();

    // ── 6. What's coming ─────────────────────────────────────────────────────
    println!("=== Phase 0 complete — stubs still todo!() ===\n");
    println!("  Sandbox::new()                  — Phase 1");
    println!("  AllowlistModuleLoader::build()  — Phase 2");
    println!("  transpile()                     — Phase 3");
    println!("  SharedRuntime::run()            — Phase 4");
    println!("  RuntimePool                     — Phase 5");
    println!("  SDK packs (core/kv/crypto/http) — Phase 6+");
}
