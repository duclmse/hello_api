//! Phase 5 integration tests for KvPack, CryptoPack, HttpPack.
//!
//! All tests use LocalSet (JsRuntime is !Send).

use std::collections::HashMap;
use std::time::Duration;

use hello_sandbox::loader::AllowlistModuleLoader;
use hello_sandbox::runtime::SharedRuntime;
use hello_sandbox::sdk::core_sdk::CorePack;
use hello_sandbox::sdk::crypto_sdk::CryptoPack;
use hello_sandbox::sdk::http_sdk::{HttpConfig, HttpPack};
use hello_sandbox::sdk::kv_sdk::KvPack;
use hello_sandbox::sdk::SdkRegistry;
use hello_sandbox::{RunCapabilities, SandboxConfig, SandboxError, SandboxEvent};
use serde_json::{json, Value};
use tokio::sync::mpsc;

fn null_tx() -> mpsc::UnboundedSender<SandboxEvent> {
    mpsc::unbounded_channel().0
}

fn caps() -> RunCapabilities {
    RunCapabilities::default()
}

fn make_runtime_with_sdk(packs: impl FnOnce(SdkRegistry) -> SdkRegistry) -> SharedRuntime {
    let loader = AllowlistModuleLoader::new();
    let sdk = packs(SdkRegistry::empty().register(CorePack));
    SharedRuntime::new(SandboxConfig::trusted(), loader, &sdk)
}

// ─── KV tests ─────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn kv_set_and_get() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut rt = make_runtime_with_sdk(|s| s.register(KvPack::default()));
            let (val, _, _) = rt
                .run(
                    r#"
                    import { kv } from "sandbox:kv";
                    await kv.set("answer", 42);
                    const v = await kv.get("answer");
                    return v;
                "#,
                    HashMap::new(),
                    null_tx(),
                    caps(),
                )
                .await
                .unwrap();
            assert_eq!(val, json!(42));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn kv_get_missing_returns_null() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut rt = make_runtime_with_sdk(|s| s.register(KvPack::default()));
            let (val, _, _) = rt
                .run(
                    r#"
                    import { kv } from "sandbox:kv";
                    return await kv.get("no_such_key");
                "#,
                    HashMap::new(),
                    null_tx(),
                    caps(),
                )
                .await
                .unwrap();
            assert_eq!(val, Value::Null);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn kv_persists_across_runs() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut rt = make_runtime_with_sdk(|s| s.register(KvPack::default()));

            // Run 1: write
            rt.run(
                r#"
                    import { kv } from "sandbox:kv";
                    await kv.set("x", 99);
                "#,
                HashMap::new(),
                null_tx(),
                caps(),
            )
            .await
            .unwrap();

            // Run 2: read — should see the value from run 1
            let (val, _, _) = rt
                .run(
                    r#"
                    import { kv } from "sandbox:kv";
                    return await kv.get("x");
                "#,
                    HashMap::new(),
                    null_tx(),
                    caps(),
                )
                .await
                .unwrap();
            assert_eq!(val, json!(99), "KV store must persist across runs");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn kv_delete_removes_key() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut rt = make_runtime_with_sdk(|s| s.register(KvPack::default()));
            let (val, _, _) = rt
                .run(
                    r#"
                    import { kv } from "sandbox:kv";
                    await kv.set("k", "v");
                    await kv.delete("k");
                    return await kv.get("k");
                "#,
                    HashMap::new(),
                    null_tx(),
                    caps(),
                )
                .await
                .unwrap();
            assert_eq!(val, Value::Null);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn kv_list_filters_by_prefix() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut rt = make_runtime_with_sdk(|s| s.register(KvPack::default()));
            let (val, _, _) = rt
                .run(
                    r#"
                    import { kv } from "sandbox:kv";
                    await kv.set("user:1", "alice");
                    await kv.set("user:2", "bob");
                    await kv.set("session:1", "xyz");
                    const keys = await kv.list("user:");
                    keys.sort();
                    return keys;
                "#,
                    HashMap::new(),
                    null_tx(),
                    caps(),
                )
                .await
                .unwrap();
            assert_eq!(val, json!(["user:1", "user:2"]));
        })
        .await;
}

// ─── Crypto tests ─────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn crypto_sha256_hash_correct_length() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut rt = make_runtime_with_sdk(|s| s.register(CryptoPack));
            let (val, _, _) = rt
                .run(
                    r#"
                    import { crypto } from "sandbox:crypto";
                    const digest = await crypto.hash("sha256", "hello");
                    return digest.length;
                "#,
                    HashMap::new(),
                    null_tx(),
                    caps(),
                )
                .await
                .unwrap();
            // SHA-256 hex digest is always 64 chars
            assert_eq!(val, json!(64));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn crypto_sha256_known_value() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut rt = make_runtime_with_sdk(|s| s.register(CryptoPack));
            let (val, _, _) = rt
                .run(
                    r#"
                    import { crypto } from "sandbox:crypto";
                    return await crypto.hash("sha256", "");
                "#,
                    HashMap::new(),
                    null_tx(),
                    caps(),
                )
                .await
                .unwrap();
            // sha256("") = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
            assert_eq!(
                val,
                json!("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn crypto_unsupported_algorithm_errors() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut rt = make_runtime_with_sdk(|s| s.register(CryptoPack));
            let err = rt
                .run(
                    r#"
                    import { crypto } from "sandbox:crypto";
                    return await crypto.hash("md5", "test");
                "#,
                    HashMap::new(),
                    null_tx(),
                    caps(),
                )
                .await
                .unwrap_err();
            match err {
                SandboxError::Runtime(_) => {},
                other => panic!("expected Runtime error, got: {other:?}"),
            }
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn crypto_random_bytes_correct_length() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut rt = make_runtime_with_sdk(|s| s.register(CryptoPack));
            let (val, _, _) = rt
                .run(
                    r#"
                    import { crypto } from "sandbox:crypto";
                    const bytes = crypto.randomBytes(16);
                    return bytes.length;
                "#,
                    HashMap::new(),
                    null_tx(),
                    caps(),
                )
                .await
                .unwrap();
            assert_eq!(val, json!(16));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn crypto_random_uuid_format() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut rt = make_runtime_with_sdk(|s| s.register(CryptoPack));
            let (val, _, _) = rt
                .run(
                    r#"
                    import { crypto } from "sandbox:crypto";
                    return crypto.randomUUID();
                "#,
                    HashMap::new(),
                    null_tx(),
                    caps(),
                )
                .await
                .unwrap();
            let uuid = val.as_str().unwrap();
            // UUID v4: xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx
            assert_eq!(uuid.len(), 36, "UUID should be 36 chars");
            assert_eq!(&uuid[14..15], "4", "UUID v4 must have '4' at position 14");
        })
        .await;
}

// ─── Crypto extended tests ────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn crypto_sha512_known_value() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut rt = make_runtime_with_sdk(|s| s.register(CryptoPack));
            let (val, _, _) = rt
                .run(
                    r#"
                    import { crypto } from "sandbox:crypto";
                    return await crypto.hash("sha512", "");
                "#,
                    HashMap::new(),
                    null_tx(),
                    caps(),
                )
                .await
                .unwrap();
            // sha512("") known value
            assert_eq!(val.as_str().unwrap().len(), 128, "SHA-512 hex digest should be 128 chars");
            // First few chars of sha512("")
            assert!(
                val.as_str().unwrap().starts_with("cf83"),
                "sha512('') should start with cf83, got: {}",
                val.as_str().unwrap()
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn crypto_sha256_same_input_same_output() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut rt = make_runtime_with_sdk(|s| s.register(CryptoPack));
            let (val, _, _) = rt
                .run(
                    r#"
                    import { crypto } from "sandbox:crypto";
                    const a = await crypto.hash("sha256", "hello");
                    const b = await crypto.hash("sha256", "hello");
                    return a === b;
                "#,
                    HashMap::new(),
                    null_tx(),
                    caps(),
                )
                .await
                .unwrap();
            assert_eq!(val, json!(true), "same input must produce same hash");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn crypto_sha256_different_inputs_different_outputs() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut rt = make_runtime_with_sdk(|s| s.register(CryptoPack));
            let (val, _, _) = rt
                .run(
                    r#"
                    import { crypto } from "sandbox:crypto";
                    const a = await crypto.hash("sha256", "hello");
                    const b = await crypto.hash("sha256", "world");
                    return a !== b;
                "#,
                    HashMap::new(),
                    null_tx(),
                    caps(),
                )
                .await
                .unwrap();
            assert_eq!(val, json!(true), "different inputs must produce different hashes");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn crypto_random_bytes_different_each_call() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut rt = make_runtime_with_sdk(|s| s.register(CryptoPack));
            let (val, _, _) = rt
                .run(
                    r#"
                    import { crypto } from "sandbox:crypto";
                    // Two calls should almost certainly produce different results
                    const a = JSON.stringify(crypto.randomBytes(16));
                    const b = JSON.stringify(crypto.randomBytes(16));
                    return a !== b;
                "#,
                    HashMap::new(),
                    null_tx(),
                    caps(),
                )
                .await
                .unwrap();
            assert_eq!(val, json!(true), "random bytes calls should differ");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn crypto_random_bytes_zero_length() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut rt = make_runtime_with_sdk(|s| s.register(CryptoPack));
            let (val, _, _) = rt
                .run(
                    r#"
                    import { crypto } from "sandbox:crypto";
                    const bytes = crypto.randomBytes(0);
                    return bytes.length;
                "#,
                    HashMap::new(),
                    null_tx(),
                    caps(),
                )
                .await
                .unwrap();
            assert_eq!(val, json!(0));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn crypto_uuid_different_each_call() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut rt = make_runtime_with_sdk(|s| s.register(CryptoPack));
            let (val, _, _) = rt
                .run(
                    r#"
                    import { crypto } from "sandbox:crypto";
                    const a = crypto.randomUUID();
                    const b = crypto.randomUUID();
                    return a !== b;
                "#,
                    HashMap::new(),
                    null_tx(),
                    caps(),
                )
                .await
                .unwrap();
            assert_eq!(val, json!(true), "UUIDs must be unique");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn crypto_uuid_v4_variant_bit() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut rt = make_runtime_with_sdk(|s| s.register(CryptoPack));
            let (val, _, _) = rt
                .run(
                    r#"
                    import { crypto } from "sandbox:crypto";
                    return crypto.randomUUID();
                "#,
                    HashMap::new(),
                    null_tx(),
                    caps(),
                )
                .await
                .unwrap();
            let uuid = val.as_str().unwrap();
            // Position 19 in UUID v4: "xxxxxxxx-xxxx-4xxx-Nxxx-..." where N is 8, 9, a, or b
            let variant_char = &uuid[19..20];
            assert!(
                ["8", "9", "a", "b"].contains(&variant_char),
                "UUID v4 variant char at pos 19 must be 8/9/a/b, got: {variant_char}"
            );
        })
        .await;
}

// ─── HTTP tests ───────────────────────────────────────────────────────────────

fn make_http_runtime(allowed: Vec<String>) -> SharedRuntime {
    let loader = AllowlistModuleLoader::new();
    let sdk = SdkRegistry::empty().register(CorePack).register(HttpPack::new(HttpConfig {
        allowed_prefixes: allowed,
        timeout: Duration::from_secs(5),
        ..Default::default()
    }));
    SharedRuntime::new(SandboxConfig::trusted(), loader, &sdk)
}

#[tokio::test(flavor = "current_thread")]
async fn http_blocked_url_returns_error() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut rt = make_http_runtime(vec!["https://allowed.example.com/".into()]);
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
                SandboxError::Runtime(e) => {
                    let msg = e.to_string();
                    assert!(
                        msg.contains("allowlist") || msg.contains("not in the sandbox"),
                        "error should mention allowlist: {msg}"
                    );
                },
                other => panic!("expected Runtime error, got: {other:?}"),
            }
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn http_empty_allowlist_blocks_all() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut rt = make_http_runtime(vec![]);
            let err = rt
                .run(
                    r#"
                    import { fetch } from "sandbox:http";
                    await fetch("https://anything.example.com/");
                "#,
                    HashMap::new(),
                    null_tx(),
                    caps(),
                )
                .await
                .unwrap_err();
            assert!(matches!(err, SandboxError::Runtime(_)));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn http_url_matching_prefix_is_allowed() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // A URL that starts with the allowed prefix — the fetch will fail
            // at the network level (no real server) but NOT due to allowlist.
            let mut rt = make_http_runtime(vec!["http://localhost".into()]);
            let err = rt
                .run(
                    r#"
                    import { fetch } from "sandbox:http";
                    await fetch("http://localhost:9999/no-server");
                "#,
                    HashMap::new(),
                    null_tx(),
                    caps(),
                )
                .await
                .unwrap_err();
            // Should be a connection error (runtime), NOT an allowlist error
            match &err {
                SandboxError::Runtime(e) => {
                    let msg = e.to_string();
                    assert!(
                        !msg.contains("allowlist"),
                        "localhost:9999 should pass allowlist, got: {msg}"
                    );
                },
                other => panic!("expected Runtime error, got: {other:?}"),
            }
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn http_url_not_matching_prefix_blocked() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // Allowed is "https://api.example.com" — "https://evil.com" must be blocked
            let mut rt = make_http_runtime(vec!["https://api.example.com".into()]);
            let err = rt
                .run(
                    r#"
                    import { fetch } from "sandbox:http";
                    await fetch("https://evil.com/steal");
                "#,
                    HashMap::new(),
                    null_tx(),
                    caps(),
                )
                .await
                .unwrap_err();
            assert!(matches!(err, SandboxError::Runtime(_)));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn kv_stores_complex_json_value() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut rt = make_runtime_with_sdk(|s| s.register(KvPack::default()));
            let (val, _, _) = rt
                .run(
                    r#"
                    import { kv } from "sandbox:kv";
                    await kv.set("obj", { nested: { arr: [1, 2, 3], flag: true } });
                    return await kv.get("obj");
                "#,
                    HashMap::new(),
                    null_tx(),
                    caps(),
                )
                .await
                .unwrap();
            assert_eq!(val["nested"]["arr"], json!([1, 2, 3]));
            assert_eq!(val["nested"]["flag"], json!(true));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn kv_overwrite_existing_key() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut rt = make_runtime_with_sdk(|s| s.register(KvPack::default()));
            let (val, _, _) = rt
                .run(
                    r#"
                    import { kv } from "sandbox:kv";
                    await kv.set("k", "first");
                    await kv.set("k", "second");
                    return await kv.get("k");
                "#,
                    HashMap::new(),
                    null_tx(),
                    caps(),
                )
                .await
                .unwrap();
            assert_eq!(val, json!("second"));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn kv_list_empty_prefix_returns_all_keys() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut rt = make_runtime_with_sdk(|s| s.register(KvPack::default()));
            let (val, _, _) = rt
                .run(
                    r#"
                    import { kv } from "sandbox:kv";
                    await kv.set("a", 1);
                    await kv.set("b", 2);
                    await kv.set("c", 3);
                    const keys = await kv.list("");
                    keys.sort();
                    return keys;
                "#,
                    HashMap::new(),
                    null_tx(),
                    caps(),
                )
                .await
                .unwrap();
            assert_eq!(val, json!(["a", "b", "c"]));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn kv_delete_nonexistent_key_is_noop() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut rt = make_runtime_with_sdk(|s| s.register(KvPack::default()));
            let (val, _, _) = rt
                .run(
                    r#"
                    import { kv } from "sandbox:kv";
                    await kv.delete("no_such_key");
                    return "ok";
                "#,
                    HashMap::new(),
                    null_tx(),
                    caps(),
                )
                .await
                .unwrap();
            assert_eq!(val, json!("ok"));
        })
        .await;
}
