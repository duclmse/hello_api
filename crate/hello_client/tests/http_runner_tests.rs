//! HTTP Test Runner integration tests (Phase 19 + F4/F6/F8 features).

use std::collections::HashMap;
use std::time::Duration;

use hello_client::http_runner::{HttpTestRunner, SecurityProfile, interpolate};
use hello_client::{HttpRequest, TestCase};
use hello_sandbox::{PoolConfig, Sandbox, SandboxConfig, SandboxError};
use serde_json::json;
use tokio::task::LocalSet;

// ─── Constants ────────────────────────────────────────────────────────────────

const TEST_JS: &str = include_str!("../sdk-ts/src/test.js");

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn single_slot() -> PoolConfig {
    PoolConfig {
        pool_size: 1,
        ..Default::default()
    }
}

fn make_test_sandbox() -> Sandbox {
    Sandbox::builder()
        .config(SandboxConfig::trusted())
        .pool(single_slot())
        .module("sandbox:test", TEST_JS)
        .build()
        .unwrap()
}

fn make_runner(server_url: &str) -> HttpTestRunner {
    HttpTestRunner::builder()
        .pool(single_slot())
        .allowed_prefixes(vec![server_url.to_string()])
        .build()
        .unwrap()
}

fn make_pm_runner(server_url: &str) -> HttpTestRunner {
    use hello_sandbox::PmPack;
    use hello_sandbox::sdk::assert_sdk::AssertPack;
    use hello_sandbox::sdk::http_sdk::{HttpConfig, HttpPack};
    use hello_sandbox::sdk::kv_sdk::KvPack;

    let http_config = HttpConfig {
        allowed_prefixes: vec![server_url.to_string()],
        ..HttpConfig::default()
    };
    let sandbox = Sandbox::builder()
        .config(SandboxConfig::power_user())
        .pool(single_slot())
        .sdk(KvPack::default())
        .sdk(HttpPack::new(http_config))
        .sdk(AssertPack)
        .sdk(PmPack)
        .build()
        .unwrap();
    HttpTestRunner::new(sandbox)
}

// ─── sandbox:test module tests ────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn test_js_expect_passes() {
    LocalSet::new()
        .run_until(async {
            let mut sb = make_test_sandbox();
            let result = sb
                .run(
                    r#"
                    import { expect, results } from "sandbox:test";
                    expect(200).toBe(200);
                    expect("hello").toContain("ell");
                    expect(5).toBeGreaterThan(3);
                    return results();
                    "#,
                )
                .await
                .unwrap();
            assert_eq!(result.value["pass"], json!(true));
            assert!(result.value["failures"].as_array().unwrap().is_empty());
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn test_js_expect_fails() {
    LocalSet::new()
        .run_until(async {
            let mut sb = make_test_sandbox();
            let result = sb
                .run(
                    r#"
                    import { expect, results } from "sandbox:test";
                    expect(404).toBe(200);
                    return results();
                    "#,
                )
                .await
                .unwrap();
            assert_eq!(result.value["pass"], json!(false));
            let failures = result.value["failures"].as_array().unwrap();
            assert_eq!(failures.len(), 1);
            assert!(failures[0].as_str().unwrap().contains("404"));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn test_js_wrap_response_status() {
    LocalSet::new()
        .run_until(async {
            let mut sb = make_test_sandbox();
            let result = sb
                .run(r#"
                    import { wrapResponse, results } from "sandbox:test";
                    const raw = { status: 201, ok: true, headers: [], body: "", response_time_ms: 10 };
                    const resp = wrapResponse(raw);
                    return resp.status;
                    "#)
                .await
                .unwrap();
            assert_eq!(result.value, json!(201));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn test_js_wrap_response_headers_get() {
    LocalSet::new()
        .run_until(async {
            let mut sb = make_test_sandbox();
            let result = sb
                .run(
                    r#"
                    import { wrapResponse } from "sandbox:test";
                    const raw = {
                        status: 200, ok: true,
                        headers: [["Content-Type", "application/json"], ["X-Foo", "bar"]],
                        body: "", response_time_ms: 5
                    };
                    const resp = wrapResponse(raw);
                    return {
                        lower: resp.headers.get("content-type"),
                        upper: resp.headers.get("CONTENT-TYPE"),
                        missing: resp.headers.get("x-missing"),
                        has: resp.headers.has("x-foo"),
                    };
                    "#,
                )
                .await
                .unwrap();
            assert_eq!(result.value["lower"], json!("application/json"));
            assert_eq!(result.value["upper"], json!("application/json"));
            assert_eq!(result.value["missing"], json!(null));
            assert_eq!(result.value["has"], json!(true));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn test_js_wrap_response_json() {
    LocalSet::new()
        .run_until(async {
            let mut sb = make_test_sandbox();
            let result = sb
                .run(
                    r#"
                    import { wrapResponse } from "sandbox:test";
                    const raw = {
                        status: 200, ok: true, headers: [],
                        body: '{"answer":42}', response_time_ms: 1
                    };
                    const resp = wrapResponse(raw);
                    return resp.json();
                    "#,
                )
                .await
                .unwrap();
            assert_eq!(result.value, json!({"answer": 42}));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn test_js_results_exported() {
    LocalSet::new()
        .run_until(async {
            let mut sb = make_test_sandbox();
            let result = sb
                .run(
                    r#"
                    import { results } from "sandbox:test";
                    return results();
                    "#,
                )
                .await
                .unwrap();
            assert_eq!(result.value["pass"], json!(true));
            assert!(result.value["failures"].as_array().unwrap().is_empty());
        })
        .await;
}

// ─── HttpTestRunner tests ─────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn runner_no_scripts_returns_response() {
    LocalSet::new()
        .run_until(async {
            let mut server = mockito::Server::new_async().await;
            let _m = server //
                .mock("GET", "/hello")
                .with_status(200)
                .with_header("content-type", "text/plain")
                .with_body("world")
                .create_async()
                .await;

            let url = format!("{}/hello", server.url());
            let mut runner = make_runner(&server.url());
            let result = runner
                .run_test(TestCase {
                    name: "no_scripts".into(),
                    request: HttpRequest::get(url),
                    ..Default::default()
                })
                .await
                .unwrap();

            let resp = result.response.unwrap();
            assert_eq!(resp.status, 200);
            assert!(resp.ok);
            assert_eq!(resp.body, "world");
            assert!(result.passed);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn runner_post_script_assertions_pass() {
    LocalSet::new()
        .run_until(async {
            let mut server = mockito::Server::new_async().await;
            let _m = server //
                .mock("GET", "/")
                .with_status(200)
                .with_body("ok")
                .create_async()
                .await;

            let mut runner = make_runner(&server.url());
            let result = runner
                .run_test(TestCase {
                    name: "assert_pass".into(),
                    request: HttpRequest::get(server.url()),
                    post_script: Some(
                        r#"
                        import { expect, results } from "sandbox:test";
                        const resp = sandbox.readInput("_response");
                        expect(resp.status).toBe(200);
                        expect(resp.ok).toBeTruthy();
                        return results();
                    "#
                        .into(),
                    ),
                    ..Default::default()
                })
                .await
                .unwrap();

            assert!(result.passed);
            assert!(result.failures.is_empty());
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn runner_post_script_assertions_fail() {
    LocalSet::new()
        .run_until(async {
            let mut server = mockito::Server::new_async().await;
            let _m = server //
                .mock("GET", "/")
                .with_status(200)
                .with_body("ok")
                .create_async()
                .await;

            let mut runner = make_runner(&server.url());
            let result = runner
                .run_test(TestCase {
                    name: "assert_fail".into(),
                    request: HttpRequest::get(server.url()),
                    post_script: Some(
                        r#"
                        import { expect, results } from "sandbox:test";
                        const resp = sandbox.readInput("_response");
                        expect(resp.status).toBe(404);
                        return results();
                    "#
                        .into(),
                    ),
                    ..Default::default()
                })
                .await
                .unwrap();

            assert!(!result.passed);
            assert_eq!(result.failures.len(), 1);
            assert!(result.failures[0].contains("200"));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn runner_pre_script_can_override_url() {
    LocalSet::new()
        .run_until(async {
            let mut server = mockito::Server::new_async().await;
            let _m_orig = server //
                .mock("GET", "/original")
                .with_status(200)
                .with_body("original")
                .create_async()
                .await;
            let _m_new = server //
                .mock("GET", "/overridden")
                .with_status(201)
                .with_body("overridden")
                .create_async()
                .await;

            let original_url = format!("{}/original", server.url());
            let override_url = format!("{}/overridden", server.url());

            let mut runner = make_runner(&server.url());
            let pre_script = format!(
                r#"const req = sandbox.readInput("_request");
return {{ url: "{override_url}", method: req.method, headers: req.headers, body: req.body }};"#
            );

            let result = runner
                .run_test(TestCase {
                    name: "override_url".into(),
                    request: HttpRequest::get(original_url),
                    pre_script: Some(pre_script),
                    ..Default::default()
                })
                .await
                .unwrap();

            let resp = result.response.unwrap();
            assert_eq!(resp.status, 201);
            assert_eq!(resp.body, "overridden");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn runner_pre_script_kv_shared_with_post() {
    LocalSet::new()
        .run_until(async {
            let mut server = mockito::Server::new_async().await;
            let _m = server //
                .mock("GET", "/")
                .with_status(200)
                .with_body("ok")
                .create_async()
                .await;

            let mut runner = make_runner(&server.url());
            let result = runner
                .run_test(TestCase {
                    name: "kv_shared".into(),
                    request: HttpRequest::get(server.url()),
                    pre_script: Some(
                        r#"
                        import { kv } from "sandbox:kv";
                        await kv.set("shared_key", "hello_from_pre");
                        return null;
                    "#
                        .into(),
                    ),
                    post_script: Some(
                        r#"
                        import { kv } from "sandbox:kv";
                        import { expect, results } from "sandbox:test";
                        const val = await kv.get("shared_key");
                        expect(val).toBe("hello_from_pre");
                        return results();
                    "#
                        .into(),
                    ),
                    ..Default::default()
                })
                .await
                .unwrap();

            assert!(result.passed, "failures: {:?}", result.failures);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn runner_collection_runs_sequentially() {
    LocalSet::new()
        .run_until(async {
            let mut server = mockito::Server::new_async().await;
            for path in &["/a", "/b", "/c"] {
                server //
                    .mock("GET", *path)
                    .with_status(200)
                    .with_body(*path)
                    .create_async()
                    .await;
            }

            let mut runner = make_runner(&server.url());
            let tests: Vec<TestCase> = ["a", "b", "c"]
                .iter()
                .map(|name| TestCase {
                    name: name.to_string(),
                    request: HttpRequest::get(format!("{}/{}", server.url(), name)),
                    ..Default::default()
                })
                .collect();

            let collection = runner.run_collection(tests).await.unwrap();
            assert_eq!(collection.results.len(), 3);
            assert_eq!(collection.results[0].name, "a");
            assert_eq!(collection.results[1].name, "b");
            assert_eq!(collection.results[2].name, "c");
            assert_eq!(collection.passed, 3);
            assert_eq!(collection.failed, 0);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn runner_collection_kv_shared_across_tests() {
    LocalSet::new()
        .run_until(async {
            let mut server = mockito::Server::new_async().await;
            server //
                .mock("GET", "/")
                .with_status(200)
                .with_body("ok")
                .expect(2)
                .create_async()
                .await;

            let mut runner = make_runner(&server.url());

            let test1 = TestCase {
                name: "writer".into(),
                request: HttpRequest::get(server.url()),
                post_script: Some(
                    r#"
                    import { kv } from "sandbox:kv";
                    import { results } from "sandbox:test";
                    await kv.set("cross_test_key", 99);
                    return results();
                    "#
                    .into(),
                ),
                ..Default::default()
            };

            let test2 = TestCase {
                name: "reader".into(),
                request: HttpRequest::get(server.url()),
                post_script: Some(
                    r#"
                    import { kv } from "sandbox:kv";
                    import { expect, results } from "sandbox:test";
                    const val = await kv.get("cross_test_key");
                    expect(val).toBe(99);
                    return results();
                "#
                    .into(),
                ),
                ..Default::default()
            };

            let col = runner.run_collection(vec![test1, test2]).await.unwrap();
            assert_eq!(
                col.passed,
                2,
                "test failures: {:?}",
                col.results.iter().map(|r| &r.failures).collect::<Vec<_>>()
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn runner_tags_visible_in_post_script() {
    LocalSet::new()
        .run_until(async {
            let mut server = mockito::Server::new_async().await;
            let _m = server //
                .mock("GET", "/")
                .with_status(200)
                .with_body("ok")
                .create_async()
                .await;

            let mut runner = make_runner(&server.url());
            let mut custom_tags = HashMap::new();
            custom_tags.insert("tenant".into(), "acme".into());

            let result = runner
                .run_test(TestCase {
                    name: "tags_test".into(),
                    request: HttpRequest::get(server.url()),
                    tags: custom_tags,
                    post_script: Some(
                        r#"
                        import { expect, results } from "sandbox:test";
                        const tags = sandbox.tags();
                        expect(tags["_phase"]).toBe("post");
                        expect(tags["_test"]).toBe("tags_test");
                        expect(tags["tenant"]).toBe("acme");
                        return results();
                        "#
                        .into(),
                    ),
                    ..Default::default()
                })
                .await
                .unwrap();

            assert!(result.passed, "failures: {:?}", result.failures);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn runner_timeout_override_respected() {
    LocalSet::new()
        .run_until(async {
            let mut server = mockito::Server::new_async().await;
            let _m = server //
                .mock("GET", "/")
                .with_status(200)
                .with_body("ok")
                .create_async()
                .await;

            let mut runner = make_runner(&server.url());
            let err = runner
                .run_test(TestCase {
                    name: "timeout_test".into(),
                    request: HttpRequest::get(server.url()),
                    timeout_override: Some(Duration::from_millis(200)),
                    post_script: Some("while (true) {}".into()),
                    ..Default::default()
                })
                .await
                .unwrap_err();

            assert!(matches!(err, SandboxError::Timeout(_)), "expected Timeout, got {err:?}");
        })
        .await;
}

// ─── F6: Variable interpolation ───────────────────────────────────────────────

#[test]
fn f6_interpolate_basic() {
    let mut env = HashMap::new();
    env.insert("base".into(), "https://api.example.com".into());
    env.insert("id".into(), "42".into());
    assert_eq!(interpolate("{{base}}/users/{{id}}", &env), "https://api.example.com/users/42");
}

#[test]
fn f6_interpolate_unknown_placeholder_unchanged() {
    let env = HashMap::new();
    assert_eq!(interpolate("{{unknown}}/path", &env), "{{unknown}}/path");
}

#[test]
fn f6_interpolate_empty_env() {
    let env = HashMap::new();
    assert_eq!(interpolate("https://example.com/v1", &env), "https://example.com/v1");
}

#[tokio::test(flavor = "current_thread")]
async fn f6_variable_interpolation_in_url() {
    LocalSet::new()
        .run_until(async {
            let mut server = mockito::Server::new_async().await;
            let _m = server //
                .mock("GET", "/api/users/99")
                .with_status(200)
                .with_body("user 99")
                .create_async()
                .await;

            let mut runner = HttpTestRunner::builder()
                .pool(single_slot())
                .allowed_prefixes(vec![server.url()])
                .env("base_url", server.url())
                .env("user_id", "99")
                .build()
                .unwrap();

            let result = runner
                .run_test(TestCase {
                    name: "interpolate_url".into(),
                    request: HttpRequest::get("{{base_url}}/api/users/{{user_id}}"),
                    ..Default::default()
                })
                .await
                .unwrap();

            let resp = result.response.unwrap();
            assert_eq!(resp.status, 200);
            assert_eq!(resp.body, "user 99");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn f6_variable_interpolation_in_headers() {
    LocalSet::new()
        .run_until(async {
            let mut server = mockito::Server::new_async().await;
            let _m = server //
                .mock("GET", "/secure")
                .match_header("authorization", "Bearer secret-token")
                .with_status(200)
                .with_body("ok")
                .create_async()
                .await;

            let mut runner = HttpTestRunner::builder()
                .pool(single_slot())
                .allowed_prefixes(vec![server.url()])
                .env("token", "secret-token")
                .build()
                .unwrap();

            let url = format!("{}/secure", server.url());
            let result = runner
                .run_test(TestCase {
                    name: "interpolate_header".into(),
                    request: HttpRequest {
                        url,
                        headers: vec![("authorization".into(), "Bearer {{token}}".into())],
                        ..HttpRequest::default()
                    },
                    ..Default::default()
                })
                .await
                .unwrap();

            assert_eq!(result.response.unwrap().status, 200);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn f6_set_env_updates_runner() {
    LocalSet::new()
        .run_until(async {
            let mut server = mockito::Server::new_async().await;
            server //
                .mock("GET", "/v1")
                .with_status(200)
                .with_body("v1")
                .create_async()
                .await;
            server //
                .mock("GET", "/v2")
                .with_status(201)
                .with_body("v2")
                .create_async()
                .await;

            let mut runner = HttpTestRunner::builder()
                .pool(single_slot())
                .allowed_prefixes(vec![server.url()])
                .build()
                .unwrap();

            runner.set_env("base", server.url());
            runner.set_env("path", "v1");
            let r1 = runner
                .run_test(TestCase {
                    name: "r1".into(),
                    request: HttpRequest::get("{{base}}/{{path}}"),
                    ..Default::default()
                })
                .await
                .unwrap();
            assert_eq!(r1.response.unwrap().body, "v1");

            runner.set_env("path", "v2");
            let r2 = runner
                .run_test(TestCase {
                    name: "r2".into(),
                    request: HttpRequest::get("{{base}}/{{path}}"),
                    ..Default::default()
                })
                .await
                .unwrap();
            assert_eq!(r2.response.unwrap().status, 201);
        })
        .await;
}

// ─── F8: Security profiles ────────────────────────────────────────────────────

#[test]
fn f8_public_api_profile_sets_prefix() {
    let caps = SecurityProfile::public_api("https://api.example.com");
    assert_eq!(
        caps.http_allowed_prefixes.as_ref().unwrap(),
        &vec!["https://api.example.com".to_string()]
    );
    assert!(caps.http_calls_limit.is_some());
    assert!(caps.kv_ops_limit.is_some());
}

#[test]
fn f8_auth_flow_profile_restricts_emit_names() {
    let caps = SecurityProfile::auth_flow("https://auth.example.com");
    let names = caps.emit_allowed_names.as_ref().unwrap();
    assert!(names.contains(&"token_extracted".to_string()));
    assert!(names.contains(&"test_pass".to_string()));
}

#[test]
fn f8_sensitive_profile_sets_kv_prefix() {
    let caps = SecurityProfile::sensitive("https://api.example.com", "test-123");
    let prefix = caps.kv_key_prefix.as_ref().unwrap();
    assert_eq!(prefix, "secure:test-123:");
}

#[test]
fn f8_user_script_profile_disables_http() {
    let caps = SecurityProfile::user_script(Duration::from_secs(2));
    assert_eq!(caps.http_enabled, Some(false));
    assert_eq!(caps.timeout_override, Some(Duration::from_secs(2)));
}

#[tokio::test(flavor = "current_thread")]
async fn f8_user_script_profile_blocks_http_in_sandbox() {
    LocalSet::new()
        .run_until(async {
            let mut server = mockito::Server::new_async().await;
            let _m = server //
                .mock("GET", "/")
                .with_status(200)
                .with_body("ok")
                .create_async()
                .await;

            let mut runner = HttpTestRunner::builder()
                .pool(single_slot())
                .allowed_prefixes(vec![server.url()])
                .build()
                .unwrap();

            let url = server.url();
            let pre_script = format!(
                r#"import {{ fetch }} from "sandbox:http";
try {{ await fetch("{url}"); }} catch (e) {{ sandbox.emit("blocked", {{ msg: e.message }}); }}
return null;"#
            );

            let result = runner
                .run_test(TestCase {
                    name: "user_script_profile".into(),
                    request: HttpRequest::get(server.url()),
                    pre_script: Some(pre_script),
                    ..Default::default()
                })
                .await
                .unwrap();

            let caps = SecurityProfile::user_script(Duration::from_millis(500));
            assert_eq!(caps.http_enabled, Some(false));
            assert!(result.response.is_some());
        })
        .await;
}

// ─── Module imports: auto-prelude ────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn auto_prelude_injected_when_no_explicit_imports() {
    LocalSet::new()
        .run_until(async {
            let mut server = mockito::Server::new_async().await;
            let _m = server //
                .mock("GET", "/")
                .with_status(200)
                .with_body("ok")
                .create_async()
                .await;

            let mut runner = make_runner(&server.url());
            // No explicit `import` — auto-prelude should supply expect + results.
            let result = runner
                .run_test(TestCase {
                    name: "auto_prelude".into(),
                    request: HttpRequest::get(server.url()),
                    post_script: Some(
                        r#"
                        const resp = sandbox.readInput("_response");
                        expect(resp.status).toBe(200);
                        return results();
                        "#
                        .into(),
                    ),
                    ..Default::default()
                })
                .await
                .unwrap();

            assert!(result.passed, "failures: {:?}", result.failures);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn auto_prelude_does_not_duplicate_explicit_imports() {
    LocalSet::new()
        .run_until(async {
            let mut server = mockito::Server::new_async().await;
            let _m = server //
                .mock("GET", "/")
                .with_status(200)
                .with_body("ok")
                .create_async()
                .await;

            let mut runner = make_runner(&server.url());
            // `expect` is already imported; prelude should only add wrapResponse + results.
            let result = runner
                .run_test(TestCase {
                    name: "no_dup".into(),
                    request: HttpRequest::get(server.url()),
                    post_script: Some(
                        r#"
                        import { expect } from "sandbox:test";
                        const resp = sandbox.readInput("_response");
                        expect(resp.status).toBe(200);
                        return results();
                        "#
                        .into(),
                    ),
                    ..Default::default()
                })
                .await
                .unwrap();

            assert!(result.passed, "failures: {:?}", result.failures);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn auto_prelude_full_explicit_import_also_works() {
    LocalSet::new()
        .run_until(async {
            let mut server = mockito::Server::new_async().await;
            let _m = server //
                .mock("GET", "/")
                .with_status(200)
                .with_body("ok")
                .create_async()
                .await;

            let mut runner = make_runner(&server.url());
            // Full explicit import → prelude is empty → no SyntaxError.
            let result = runner
                .run_test(TestCase {
                    name: "full_explicit".into(),
                    request: HttpRequest::get(server.url()),
                    post_script: Some(
                        r#"
                        import { expect, wrapResponse, results } from "sandbox:test";
                        const resp = wrapResponse(sandbox.readInput("_response"));
                        expect(resp.status).toBe(200);
                        return results();
                        "#
                        .into(),
                    ),
                    ..Default::default()
                })
                .await
                .unwrap();

            assert!(result.passed, "failures: {:?}", result.failures);
        })
        .await;
}

// ─── Module imports: TestCase::modules field ──────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn test_case_modules_field_registers_and_imports() {
    LocalSet::new()
        .run_until(async {
            let mut server = mockito::Server::new_async().await;
            let _m = server //
                .mock("GET", "/")
                .with_status(200)
                .with_body("ok")
                .create_async()
                .await;

            let mut runner = make_runner(&server.url());
            let result = runner
                .run_test(TestCase {
                    name: "module_field".into(),
                    request: HttpRequest::get(server.url()),
                    modules: vec![(
                        "sandbox:utils".into(),
                        "export const greet = (name) => 'hello ' + name;".into(),
                    )],
                    post_script: Some(
                        r#"
                        import { greet } from "sandbox:utils";
                        const msg = greet("world");
                        expect(msg).toBe("hello world");
                        return results();
                        "#
                        .into(),
                    ),
                    ..Default::default()
                })
                .await
                .unwrap();

            assert!(result.passed, "failures: {:?}", result.failures);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn test_case_modules_available_across_multiple_runs() {
    LocalSet::new()
        .run_until(async {
            let mut server = mockito::Server::new_async().await;
            server //
                .mock("GET", "/")
                .with_status(200)
                .with_body("ok")
                .expect(2)
                .create_async()
                .await;

            let mut runner = make_runner(&server.url());

            // First run registers the module.
            let r1 = runner
                .run_test(TestCase {
                    name: "first".into(),
                    request: HttpRequest::get(server.url()),
                    modules: vec![(
                        "sandbox:shared_module".into(),
                        "export const ANSWER = 42;".into(),
                    )],
                    post_script: Some(
                        r#"
                        import { ANSWER } from "sandbox:shared_module";
                        expect(ANSWER).toBe(42);
                        return results();
                        "#
                        .into(),
                    ),
                    ..Default::default()
                })
                .await
                .unwrap();
            assert!(r1.passed, "first run failures: {:?}", r1.failures);

            // Second run reuses the same sandbox — module should still be registered.
            let r2 = runner
                .run_test(TestCase {
                    name: "second".into(),
                    request: HttpRequest::get(server.url()),
                    post_script: Some(
                        r#"
                        import { ANSWER } from "sandbox:shared_module";
                        expect(ANSWER).toBe(42);
                        return results();
                        "#
                        .into(),
                    ),
                    ..Default::default()
                })
                .await
                .unwrap();
            assert!(r2.passed, "second run failures: {:?}", r2.failures);
        })
        .await;
}

// ─── Module imports: file script with relative imports ────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn file_script_relative_import_is_rewritten_and_registered() {
    LocalSet::new()
        .run_until(async {
            // Set up a temp dir with script files.
            let tmp = std::env::temp_dir().join(format!("http_runner_test_{}", std::process::id()));
            std::fs::create_dir_all(&tmp).unwrap();

            // Helper module.
            std::fs::write(tmp.join("math_helper.js"), "export const double = (x) => x * 2;\n")
                .unwrap();

            // Post-script file that imports the helper.
            std::fs::write(
                tmp.join("post.js"),
                r#"import { double } from "./math_helper.js";
const resp = sandbox.readInput("_response");
expect(resp.status).toBe(200);
const val = double(21);
expect(val).toBe(42);
return results();
"#,
            )
            .unwrap();

            // Parse via public parse_collection so module discovery runs.
            let http_content = format!(
                "GET http://example.com/ HTTP/1.1\n\n> {}\n",
                tmp.join("post.js").display()
            );
            let mut tcs =
                hello_client::runner::parse_collection(&http_content, &Default::default(), &tmp)
                    .unwrap();
            let tc = tcs.remove(0);

            assert_eq!(tc.modules.len(), 1, "math_helper.js should be in modules");
            assert!(
                tc.modules[0].0.contains("math_helper"),
                "spec should contain math_helper: {}",
                tc.modules[0].0
            );

            // Run through a mockito server.
            let mut server = mockito::Server::new_async().await;
            let _m = server //
                .mock("GET", "/")
                .with_status(200)
                .with_body("ok")
                .create_async()
                .await;

            let mut runner = HttpTestRunner::builder()
                .pool(single_slot())
                .allowed_prefixes(vec![server.url()])
                .build()
                .unwrap();

            // Replace the URL in the test case.
            let result = runner
                .run_test(TestCase {
                    request: hello_client::HttpRequest::get(server.url()),
                    ..tc
                })
                .await
                .unwrap();

            assert!(result.passed, "failures: {:?}", result.failures);
            std::fs::remove_dir_all(&tmp).ok();
        })
        .await;
}

// ─── pm.visualizer ───────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn visualizer_html_is_set_when_pm_visualizer_called() {
    use hello_sandbox::PmPack;
    use hello_sandbox::sdk::assert_sdk::AssertPack;
    use hello_sandbox::sdk::http_sdk::{HttpConfig, HttpPack};
    use hello_sandbox::sdk::kv_sdk::KvPack;

    LocalSet::new()
        .run_until(async {
            let mut server = mockito::Server::new_async().await;
            let _m = server //
                .mock("GET", "/")
                .with_status(200)
                .with_body("hello")
                .create_async()
                .await;

            let http_config = HttpConfig {
                allowed_prefixes: vec![server.url()],
                ..HttpConfig::default()
            };
            let sandbox = Sandbox::builder()
                .config(SandboxConfig::power_user())
                .pool(single_slot())
                .sdk(KvPack::default())
                .sdk(HttpPack::new(http_config))
                .sdk(AssertPack)
                .sdk(PmPack)
                .build()
                .unwrap();

            let mut runner = HttpTestRunner::new(sandbox);

            let result = runner
                .run_test(TestCase {
                    name: "viz_test".into(),
                    request: HttpRequest::get(server.url()),
                    post_script: Some(
                        r#"
import { pm, results } from "sandbox:pm";
pm.visualizer.set("<h1>{{title}}</h1>", { title: "Hello" });
return results();
"#
                        .into(),
                    ),
                    ..Default::default()
                })
                .await
                .unwrap();

            let html = result.visualizer_html.expect("visualizer_html should be Some");
            assert!(html.contains("<h1>{{title}}</h1>"), "template in HTML:\n{html}");
            assert!(html.contains(r#""title":"Hello""#), "data JSON in HTML:\n{html}");
            assert!(html.contains("Handlebars.compile"), "Handlebars script in HTML:\n{html}");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn visualizer_html_is_none_when_not_called() {
    LocalSet::new()
        .run_until(async {
            let mut server = mockito::Server::new_async().await;
            let _m = server //
                .mock("GET", "/")
                .with_status(200)
                .with_body("ok")
                .create_async()
                .await;

            let mut runner = make_runner(&server.url());
            let result = runner
                .run_test(TestCase {
                    name: "no_viz".into(),
                    request: HttpRequest::get(server.url()),
                    post_script: Some(r#"return results();"#.into()),
                    ..Default::default()
                })
                .await
                .unwrap();

            assert!(result.visualizer_html.is_none());
        })
        .await;
}

// ─── output_file ──────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn output_file_writes_response_body_bytes() {
    LocalSet::new()
        .run_until(async {
            let mut server = mockito::Server::new_async().await;
            let body = b"raw response bytes";
            let _m = server //
                .mock("GET", "/data")
                .with_status(200)
                .with_body(body.as_ref())
                .create_async()
                .await;

            let tmp_file = std::env::temp_dir().join("hello_client_test_output.bin");
            let _ = std::fs::remove_file(&tmp_file);
            let tmp_path = tmp_file.to_string_lossy().to_string();

            let mut runner = make_runner(&server.url());
            let result = runner
                .run_test(TestCase {
                    name: "output_test".into(),
                    request: HttpRequest::get(format!("{}/data", server.url())),
                    output_file: Some(tmp_path.clone()),
                    ..Default::default()
                })
                .await
                .unwrap();

            assert_eq!(result.output_written.as_deref(), Some(tmp_path.as_str()));
            let written = std::fs::read(&tmp_file).expect("output file should exist");
            let text = std::str::from_utf8(&written).expect("valid utf8");
            assert!(text.starts_with("HTTP/1.1 200 OK\n"), "should have status line");
            assert!(text.ends_with("raw response bytes"), "should end with body");
            let _ = std::fs::remove_file(&tmp_file);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn output_written_is_none_when_output_file_not_set() {
    LocalSet::new()
        .run_until(async {
            let mut server = mockito::Server::new_async().await;
            let _m = server //
                .mock("GET", "/")
                .with_status(200)
                .with_body("ok")
                .create_async()
                .await;

            let mut runner = make_runner(&server.url());
            let result = runner
                .run_test(TestCase {
                    name: "no_output".into(),
                    request: HttpRequest::get(server.url()),
                    ..Default::default()
                })
                .await
                .unwrap();

            assert!(result.output_written.is_none());
        })
        .await;
}

// ─── Dynamic variables ($guid, $timestamp, …) ────────────────────────────────

#[test]
fn interpolate_guid_is_valid_v4_uuid() {
    let env = HashMap::new();
    let result = interpolate("{{$guid}}", &env);
    assert_eq!(result.len(), 36, "UUID length: {result}");
    let parts: Vec<&str> = result.split('-').collect();
    assert_eq!(parts.len(), 5);
    assert!(parts[2].starts_with('4'), "version nibble: {}", parts[2]);
    let variant = parts[3].chars().next().unwrap();
    assert!("89ab".contains(variant), "variant nibble: {}", parts[3]);
}

#[test]
fn interpolate_random_uuid_alias_works() {
    let env = HashMap::new();
    let result = interpolate("{{$randomUUID}}", &env);
    assert_eq!(result.len(), 36);
}

#[test]
fn interpolate_timestamp_is_unix_seconds() {
    let env = HashMap::new();
    let ts: u64 = interpolate("{{$timestamp}}", &env).parse().expect("numeric");
    assert!(ts > 1_700_000_000, "timestamp seems wrong: {ts}");
}

#[test]
fn interpolate_iso_timestamp_format() {
    let env = HashMap::new();
    let result = interpolate("{{$isoTimestamp}}", &env);
    assert!(result.ends_with('Z'), "should end with Z: {result}");
    assert!(result.contains('T'), "should contain T: {result}");
    assert_eq!(result.len(), 24, "expected YYYY-MM-DDTHH:MM:SS.MMMZ: {result}");
}

#[test]
fn interpolate_random_int_in_range() {
    let env = HashMap::new();
    let n: u64 = interpolate("{{$randomInt}}", &env).parse().expect("numeric");
    assert!(n <= 1000, "should be 0-1000: {n}");
}

#[test]
fn interpolate_random_boolean_is_true_or_false() {
    let env = HashMap::new();
    let result = interpolate("{{$randomBoolean}}", &env);
    assert!(result == "true" || result == "false", "unexpected: {result}");
}

#[test]
fn interpolate_unknown_dynamic_var_left_unchanged() {
    let env = HashMap::new();
    assert_eq!(interpolate("{{$unknown}}", &env), "{{$unknown}}");
}

#[test]
fn interpolate_dynamic_var_mixed_with_env_var() {
    let mut env = HashMap::new();
    env.insert("host".into(), "api.example.com".into());
    let result = interpolate("https://{{host}}/id/{{$guid}}", &env);
    assert!(result.starts_with("https://api.example.com/id/"));
    assert_eq!(result.len(), "https://api.example.com/id/".len() + 36);
}

// ─── helper functions ─────────────────────────────────────────────────────────

#[test]
fn interpolate_base64_single_env_var() {
    let mut env = HashMap::new();
    env.insert("token".into(), "hello".into());
    // base64("hello") == "aGVsbG8="
    assert_eq!(interpolate("{{$base64(token)}}", &env), "aGVsbG8=");
}

#[test]
fn interpolate_base64_quoted_literal() {
    let env = HashMap::new();
    // base64("user:pass") == "dXNlcjpwYXNz"
    assert_eq!(interpolate(r#"{{$base64("user:pass")}}"#, &env), "dXNlcjpwYXNz");
}

#[test]
fn interpolate_base64_single_arg() {
    let env = HashMap::new();
    // base64("hello") == "aGVsbG8="
    assert_eq!(interpolate(r#"{{$base64("hello")}}"#, &env), "aGVsbG8=");
}

#[test]
fn interpolate_base64url_single_env_var() {
    let mut env = HashMap::new();
    env.insert("payload".into(), "alice:s3cr3t".into());
    // base64url("alice:s3cr3t") — no padding, URL-safe alphabet
    assert_eq!(interpolate("{{$base64url(payload)}}", &env), "YWxpY2U6czNjcjN0");
}

#[test]
fn interpolate_base64decode_roundtrip() {
    let env = HashMap::new();
    // base64decode("aGVsbG8=") == "hello"
    assert_eq!(interpolate(r#"{{$base64decode("aGVsbG8=")}}"#, &env), "hello");
}

#[test]
fn interpolate_nested_base64_concat() {
    let mut env = HashMap::new();
    env.insert("username".into(), "alice".into());
    env.insert("password".into(), "s3cr3t".into());
    // $base64($concat(username, ":", password)) == base64("alice:s3cr3t")
    assert_eq!(
        interpolate(r#"Basic {{$base64($concat(username, ":", password))}}"#, &env),
        "Basic YWxpY2U6czNjcjN0"
    );
}

#[test]
fn interpolate_nested_sha256_concat() {
    let env = HashMap::new();
    // sha256("hello") — nesting with a quoted literal arg to concat
    let direct = interpolate(r#"{{$sha256("hello")}}"#, &env);
    let nested = interpolate(r#"{{$sha256($concat("hel", "lo"))}}"#, &env);
    assert_eq!(direct, nested);
}

#[test]
fn interpolate_basic_auth_returns_full_header_value() {
    let mut env = HashMap::new();
    env.insert("username".into(), "alice".into());
    env.insert("password".into(), "s3cr3t".into());
    assert_eq!(interpolate("{{$basicAuth(username, password)}}", &env), "Basic YWxpY2U6czNjcjN0");
}

#[test]
fn interpolate_url_encode_special_chars() {
    let mut env = HashMap::new();
    env.insert("q".into(), "hello world".into());
    let out = interpolate("{{$urlEncode(q)}}", &env);
    assert_eq!(out, "hello%20world");
}

#[test]
fn interpolate_url_encode_ampersand() {
    let env = HashMap::new();
    assert_eq!(interpolate(r#"{{$urlEncode("a&b=c")}}"#, &env), "a%26b%3Dc");
}

#[test]
fn interpolate_sha256_known_digest() {
    let env = HashMap::new();
    // echo -n "hello" | sha256sum → 2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
    assert_eq!(
        interpolate(r#"{{$sha256("hello")}}"#, &env),
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
    );
}

#[test]
fn interpolate_md5_known_digest() {
    let env = HashMap::new();
    // echo -n "hello" | md5sum → 5d41402abc4b2a76b9719d911017c592
    assert_eq!(interpolate(r#"{{$md5("hello")}}"#, &env), "5d41402abc4b2a76b9719d911017c592");
}

#[test]
fn interpolate_hmac_sha256_known_digest() {
    let env = HashMap::new();
    // HMAC-SHA256("secret", "message") — verified with standard test vectors
    // echo -n "message" | openssl dgst -sha256 -hmac "secret" -hex
    assert_eq!(
        interpolate(r#"{{$hmacSha256("secret", "message")}}"#, &env),
        "8b5f48702995c1598c573db1e21866a9b825d4a794d169d7060a03605796360b"
    );
}

#[test]
fn interpolate_concat_joins_without_separator() {
    let mut env = HashMap::new();
    env.insert("a".into(), "foo".into());
    env.insert("b".into(), "bar".into());
    assert_eq!(interpolate("{{$concat(a, b)}}", &env), "foobar");
}

#[test]
fn interpolate_to_upper() {
    let mut env = HashMap::new();
    env.insert("word".into(), "hello".into());
    assert_eq!(interpolate("{{$toUpper(word)}}", &env), "HELLO");
}

#[test]
fn interpolate_to_lower() {
    let mut env = HashMap::new();
    env.insert("word".into(), "WORLD".into());
    assert_eq!(interpolate("{{$toLower(word)}}", &env), "world");
}

#[test]
fn interpolate_unknown_helper_fn_left_unchanged() {
    let env = HashMap::new();
    assert_eq!(interpolate("{{$unknown(foo, bar)}}", &env), "{{$unknown(foo, bar)}}");
}

// ─── response_file ────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn response_file_skips_fetch_and_uses_file_body() {
    LocalSet::new()
        .run_until(async {
            let tmp = std::env::temp_dir().join("hello_client_test_response_file.txt");
            std::fs::write(&tmp, b"hello from file").unwrap();

            let mut runner = HttpTestRunner::builder()
                .pool(single_slot())
                .allowed_prefixes(vec![])
                .build()
                .unwrap();

            let result = runner
                .run_test(TestCase {
                    name: "from_file".into(),
                    request: HttpRequest::get("http://unused.local/api"),
                    response_file: Some(tmp.to_string_lossy().to_string()),
                    ..Default::default()
                })
                .await
                .unwrap();

            let resp = result.response.unwrap();
            assert_eq!(resp.status, 200);
            assert!(resp.ok);
            assert_eq!(resp.body, "hello from file");
            std::fs::remove_file(&tmp).ok();
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn response_file_post_script_can_read_body() {
    LocalSet::new()
        .run_until(async {
            let tmp = std::env::temp_dir().join("hello_client_test_response_postscript.json");
            std::fs::write(&tmp, br#"{"value":42}"#).unwrap();

            let mut runner = HttpTestRunner::builder()
                .pool(single_slot())
                .allowed_prefixes(vec![])
                .build()
                .unwrap();

            let result = runner
                .run_test(TestCase {
                    name: "file_post_script".into(),
                    request: HttpRequest::get("http://unused.local/api"),
                    response_file: Some(tmp.to_string_lossy().to_string()),
                    post_script: Some(
                        r#"
const r = wrapResponse(sandbox.readInput("_response"));
expect(r.json().value).toBe(42);
return results();
"#
                        .into(),
                    ),
                    ..Default::default()
                })
                .await
                .unwrap();

            assert!(result.passed, "failures: {:?}", result.failures);
            std::fs::remove_file(&tmp).ok();
        })
        .await;
}

// ─── pm.variables.replaceIn ───────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn pm_variables_replace_in_resolves_stored_var_and_dynamic() {
    use hello_sandbox::PmPack;

    LocalSet::new()
        .run_until(async {
            let mut sb = Sandbox::builder()
                .config(SandboxConfig::trusted())
                .pool(single_slot())
                .sdk(PmPack)
                .build()
                .unwrap();

            let result = sb
                .run(
                    r#"
import { pm } from "sandbox:pm";
pm.variables.set("name", "world");
var s = pm.variables.replaceIn("Hello {{name}}!");
var ts = pm.variables.replaceIn("{{$timestamp}}");
var uuid = pm.variables.replaceIn("{{$guid}}");
var unk = pm.variables.replaceIn("{{$unknown}}");
return { s, tsIsNum: !isNaN(Number(ts)), uuidLen: uuid.length, unk };
"#,
                )
                .await
                .unwrap();

            assert_eq!(result.value["s"], json!("Hello world!"));
            assert_eq!(result.value["tsIsNum"], json!(true));
            assert_eq!(result.value["uuidLen"], json!(36));
            assert_eq!(result.value["unk"], json!("{{$unknown}}"));
        })
        .await;
}

// ─── F4: redirected field ─────────────────────────────────────────────────────

#[test]
fn f4_redirected_false_for_direct_response() {
    // HttpResponse built without a redirect should have redirected == false.
    use hello_client::http_runner::HttpResponse;
    let resp = HttpResponse {
        status: 200,
        ok: true,
        headers: vec![],
        body: "ok".into(),
        response_time_ms: 5,
        redirected: false,
    };
    assert!(!resp.redirected);
}

#[tokio::test(flavor = "current_thread")]
async fn f4_no_redirect_field_is_false() {
    LocalSet::new()
        .run_until(async {
            let mut server = mockito::Server::new_async().await;
            let _m = server //
                .mock("GET", "/")
                .with_status(200)
                .with_body("ok")
                .create_async()
                .await;

            let mut runner = make_runner(&server.url());
            let result = runner
                .run_test(TestCase {
                    name: "no_redirect".into(),
                    request: HttpRequest::get(server.url()),
                    ..Default::default()
                })
                .await
                .unwrap();

            let resp = result.response.unwrap();
            assert!(!resp.redirected, "direct fetch should not be marked redirected");
        })
        .await;
}

// ─── pm.sendRequest ───────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn pm_send_request_makes_side_http_request() {
    use hello_sandbox::PmPack;
    use hello_sandbox::sdk::assert_sdk::AssertPack;
    use hello_sandbox::sdk::http_sdk::{HttpConfig, HttpPack};
    use hello_sandbox::sdk::kv_sdk::KvPack;

    LocalSet::new()
        .run_until(async {
            let mut server = mockito::Server::new_async().await;
            let _main = server //
                .mock("GET", "/main")
                .with_status(200)
                .with_body("main-body")
                .create_async()
                .await;
            let _side = server //
                .mock("GET", "/side")
                .with_status(201)
                .with_body(r#"{"key":"value"}"#)
                .with_header("content-type", "application/json")
                .create_async()
                .await;

            let http_config = HttpConfig {
                allowed_prefixes: vec![server.url()],
                ..HttpConfig::default()
            };
            let sandbox = Sandbox::builder()
                .config(SandboxConfig::power_user())
                .pool(single_slot())
                .sdk(KvPack::default())
                .sdk(HttpPack::new(http_config))
                .sdk(AssertPack)
                .sdk(PmPack)
                .build()
                .unwrap();

            let mut runner = HttpTestRunner::new(sandbox);
            let side_url = format!("{}/side", server.url());

            let result = runner
                .run_test(TestCase {
                    name: "send_request".into(),
                    request: HttpRequest::get(format!("{}/main", server.url())),
                    post_script: Some(format!(
                        r#"
const resp = await pm.sendRequest("{side_url}");
pm.test("side status", function() {{
    pm.expect(resp.status).to.equal(201);
}});
pm.test("side body key", function() {{
    pm.expect(resp.json().key).to.equal("value");
}});
pm.test("side ok flag", function() {{
    pm.expect(resp.ok).to.equal(true);
}});
"#
                    )),
                    ..Default::default()
                })
                .await
                .unwrap();

            assert!(result.passed, "pm.sendRequest failures: {:?}", result.failures);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn pm_send_request_blocked_url_throws() {
    use hello_sandbox::PmPack;
    use hello_sandbox::sdk::assert_sdk::AssertPack;
    use hello_sandbox::sdk::http_sdk::{HttpConfig, HttpPack};
    use hello_sandbox::sdk::kv_sdk::KvPack;

    LocalSet::new()
        .run_until(async {
            let mut server = mockito::Server::new_async().await;
            let _m = server //
                .mock("GET", "/")
                .with_status(200)
                .with_body("ok")
                .create_async()
                .await;

            // Allowlist only contains server.url() — "http://blocked.local" is not in it.
            let http_config = HttpConfig {
                allowed_prefixes: vec![server.url()],
                ..HttpConfig::default()
            };
            let sandbox = Sandbox::builder()
                .config(SandboxConfig::power_user())
                .pool(single_slot())
                .sdk(KvPack::default())
                .sdk(HttpPack::new(http_config))
                .sdk(AssertPack)
                .sdk(PmPack)
                .build()
                .unwrap();

            let mut runner = HttpTestRunner::new(sandbox);

            let result = runner
                .run_test(TestCase {
                    name: "blocked".into(),
                    request: HttpRequest::get(server.url()),
                    post_script: Some(
                        r#"
import { pm, results } from "sandbox:pm";
try {
    await pm.sendRequest("http://blocked.local/api");
    pm.test("should have thrown", function() { pm.expect(false).to.equal(true); });
} catch (e) {
    pm.test("error mentions allowlist", function() {
        pm.expect(String(e)).to.include("allowlist");
    });
}
return results();
"#
                        .into(),
                    ),
                    ..Default::default()
                })
                .await
                .unwrap();

            assert!(result.passed, "failures: {:?}", result.failures);
        })
        .await;
}

// ─── collection-level scripts ─────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn collection_pre_script_overrides_request_url() {
    LocalSet::new()
        .run_until(async {
            let mut server = mockito::Server::new_async().await;
            let _orig = server //
                .mock("GET", "/original")
                .with_status(200)
                .with_body("original-body")
                .create_async()
                .await;
            let _mod = server //
                .mock("GET", "/modified")
                .with_status(200)
                .with_body("modified-body")
                .create_async()
                .await;

            let pre_script = format!(r#"return {{ url: "{}/modified" }};"#, server.url());

            let mut runner = HttpTestRunner::builder()
                .pool(single_slot())
                .allowed_prefixes(vec![server.url()])
                .collection_pre_script(pre_script, vec![])
                .build()
                .unwrap();

            let result = runner
                .run_test(TestCase {
                    name: "pre_override".into(),
                    request: HttpRequest::get(format!("{}/original", server.url())),
                    ..Default::default()
                })
                .await
                .unwrap();

            let resp = result.response.unwrap();
            assert_eq!(resp.body, "modified-body", "pre-script should have overridden URL");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn collection_post_script_receives_response_and_can_fail() {
    LocalSet::new()
        .run_until(async {
            let mut server = mockito::Server::new_async().await;
            let _m = server //
                .mock("GET", "/")
                .with_status(404)
                .with_body("not found")
                .create_async()
                .await;

            let post_script = r#"
const resp = sandbox.readInput("_response");
const pass = resp.status === 200;
return { pass, failures: pass ? [] : ["status was " + resp.status] };
"#;

            let mut runner = HttpTestRunner::builder()
                .pool(single_slot())
                .allowed_prefixes(vec![server.url()])
                .collection_post_script(post_script.to_string(), vec![])
                .build()
                .unwrap();

            let result = runner
                .run_test(TestCase {
                    name: "post_fail".into(),
                    request: HttpRequest::get(server.url()),
                    ..Default::default()
                })
                .await
                .unwrap();

            assert!(!result.passed, "should fail: 404");
            assert!(
                result.failures.iter().any(|f| f.contains("404")),
                "failures: {:?}",
                result.failures
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn collection_scripts_run_for_every_request_in_collection() {
    LocalSet::new()
        .run_until(async {
            let mut server = mockito::Server::new_async().await;
            let _a = server //
                .mock("GET", "/a")
                .with_status(200)
                .with_body("a")
                .create_async()
                .await;
            let _b = server //
                .mock("GET", "/b")
                .with_status(200)
                .with_body("b")
                .create_async()
                .await;

            let post_script = r#"
const resp = sandbox.readInput("_response");
return { pass: resp.status === 200, failures: resp.status === 200 ? [] : ["not ok: " + resp.status] };
"#;

            let mut runner = HttpTestRunner::builder()
                .pool(single_slot())
                .allowed_prefixes(vec![server.url()])
                .collection_post_script(post_script.to_string(), vec![])
                .build()
                .unwrap();

            for path in &["/a", "/b"] {
                let result = runner
                    .run_test(TestCase {
                        name: format!("req_{}", path),
                        request: HttpRequest::get(format!("{}{}", server.url(), path)),
                        ..Default::default()
                    })
                    .await
                    .unwrap();
                assert!(result.passed, "{path} failed: {:?}", result.failures);
            }
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn collection_pre_script_from_http_annotation_runs() {
    use hello_client::runner::{RunOpts, run_collection_from_str_with_opts};

    LocalSet::new()
        .run_until(async {
            let mut server = mockito::Server::new_async().await;
            let _orig = server //
                .mock("GET", "/original")
                .with_status(200)
                .with_body("original")
                .create_async()
                .await;
            let _mod = server //
                .mock("GET", "/via-pre")
                .with_status(200)
                .with_body("via-pre-script")
                .create_async()
                .await;

            // Write the collection pre-script to a temp file.
            let tmp = std::env::temp_dir();
            let pre_path = tmp.join(format!("col_pre_{}.js", std::process::id()));
            std::fs::write(&pre_path, format!(r#"return {{ url: "{}/via-pre" }};"#, server.url()))
                .unwrap();

            let http_content = format!(
                "### @param collection-pre-script {}\n\nGET {}/original HTTP/1.1\n\n",
                pre_path.to_string_lossy(),
                server.url()
            );

            let result = run_collection_from_str_with_opts(
                &http_content,
                &HashMap::new(),
                &tmp,
                RunOpts::default(),
            )
            .await
            .unwrap();

            assert_eq!(result.results.len(), 1);
            let r = &result.results[0];
            let resp = r.response.as_ref().expect("response should be present");
            assert_eq!(resp.body, "via-pre-script", "pre-script should redirect to /via-pre");

            std::fs::remove_file(&pre_path).ok();
        })
        .await;
}

// ─── S4: pm.environment persistence across collection tests ──────────────────

#[tokio::test(flavor = "current_thread")]
async fn pm_environment_persists_across_collection_tests() {
    LocalSet::new()
        .run_until(async {
            let mut server = mockito::Server::new_async().await;
            server //
                .mock("GET", "/")
                .with_status(200)
                .with_body("ok")
                .expect(3)
                .create_async()
                .await;

            let mut runner = make_pm_runner(&server.url());

            // Test 1: set token in pm.environment
            let test1 = TestCase {
                name: "set-env".into(),
                request: HttpRequest::get(server.url()),
                post_script: Some(
                    r#"
                    import { pm, results } from "sandbox:pm";
                    pm.environment.set("token", "abc123");
                    return results();
                    "#
                    .into(),
                ),
                ..Default::default()
            };

            // Test 2: read token written by test 1 — also add another key
            let test2 = TestCase {
                name: "read-env".into(),
                request: HttpRequest::get(server.url()),
                post_script: Some(
                    r#"
                    import { pm, test, expect, results } from "sandbox:pm";
                    test("token persists from test 1", function() {
                        expect(pm.environment.get("token")).to.equal("abc123");
                    });
                    pm.environment.set("step", "two");
                    return results();
                "#
                    .into(),
                ),
                ..Default::default()
            };

            // Test 3: both token and step are visible
            let test3 = TestCase {
                name: "read-both".into(),
                request: HttpRequest::get(server.url()),
                post_script: Some(
                    r#"
                    import { pm, test, expect, results } from "sandbox:pm";
                    test("token still present", function() {
                        expect(pm.environment.get("token")).to.equal("abc123");
                    });
                    test("step from test 2 present", function() {
                        expect(pm.environment.get("step")).to.equal("two");
                    });
                    return results();
                "#
                    .into(),
                ),
                ..Default::default()
            };

            let col = runner.run_collection(vec![test1, test2, test3]).await.unwrap();
            assert_eq!(
                col.passed,
                3,
                "failures: {:?}",
                col.results.iter().map(|r| &r.failures).collect::<Vec<_>>()
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn pm_environment_reset_between_collection_runs() {
    LocalSet::new()
        .run_until(async {
            let mut server = mockito::Server::new_async().await;
            server //
                .mock("GET", "/")
                .with_status(200)
                .with_body("ok")
                .expect(2)
                .create_async()
                .await;

            let mut runner = make_pm_runner(&server.url());

            // First collection: set a value
            let setter = TestCase {
                name: "setter".into(),
                request: HttpRequest::get(server.url()),
                post_script: Some(
                    r#"
                    import { pm, results } from "sandbox:pm";
                    pm.environment.set("secret", "should-not-leak");
                    return results();
                    "#
                    .into(),
                ),
                ..Default::default()
            };
            runner.run_collection(vec![setter]).await.unwrap();

            // Second collection: env should be fresh — "secret" must not be visible
            let checker = TestCase {
                name: "checker".into(),
                request: HttpRequest::get(server.url()),
                post_script: Some(
                    r#"
                    import { pm, test, expect, results } from "sandbox:pm";
                    test("env cleared between collections", function() {
                        expect(pm.environment.has("secret")).to.be.false;
                    });
                    return results();
                    "#
                    .into(),
                ),
                ..Default::default()
            };
            let col2 = runner.run_collection(vec![checker]).await.unwrap();
            assert_eq!(
                col2.passed,
                1,
                "failures: {:?}",
                col2.results.iter().map(|r| &r.failures).collect::<Vec<_>>()
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn pm_globals_persist_across_collection_runs() {
    LocalSet::new()
        .run_until(async {
            let mut server = mockito::Server::new_async().await;
            server //
                .mock("GET", "/")
                .with_status(200)
                .with_body("ok")
                .expect(2)
                .create_async()
                .await;

            let mut runner = make_pm_runner(&server.url());

            // First collection: set a global
            let setter = TestCase {
                name: "setter".into(),
                request: HttpRequest::get(server.url()),
                post_script: Some(
                    r#"
                    import { pm, results } from "sandbox:pm";
                    pm.globals.set("run_count", 1);
                    return results();
                    "#
                    .into(),
                ),
                ..Default::default()
            };
            runner.run_collection(vec![setter]).await.unwrap();

            // Second collection: globals persist (unlike environment)
            let checker = TestCase {
                name: "checker".into(),
                request: HttpRequest::get(server.url()),
                post_script: Some(
                    r#"
                    import { pm, test, expect, results } from "sandbox:pm";
                    test("global persists across collection runs", function() {
                        expect(pm.globals.get("run_count")).to.equal(1);
                    });
                    return results();
                    "#
                    .into(),
                ),
                ..Default::default()
            };
            let col2 = runner.run_collection(vec![checker]).await.unwrap();
            assert_eq!(
                col2.passed,
                1,
                "failures: {:?}",
                col2.results.iter().map(|r| &r.failures).collect::<Vec<_>>()
            );
        })
        .await;
}
