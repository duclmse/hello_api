//! Integration tests for PmPack (`sandbox:pm`) — Postman/Bruno scripting API.
//!
//! All tests use pool_size = 1 (V8 single-thread constraint) and
//! `#[tokio::test(flavor = "current_thread")]` with a `LocalSet`.

use hello_sandbox::{sdk::kv_sdk::KvPack, PmPack, PoolConfig, Sandbox, SandboxConfig};
use serde_json::json;
use tokio::task::LocalSet;

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn single_slot() -> PoolConfig {
    PoolConfig {
        pool_size: 1,
        ..Default::default()
    }
}

fn pm_sandbox() -> Sandbox {
    Sandbox::builder()
        .config(SandboxConfig::power_user())
        .pool(single_slot())
        .sdk(PmPack)
        .build()
        .unwrap()
}

fn pm_kv_sandbox() -> Sandbox {
    Sandbox::builder()
        .config(SandboxConfig::power_user())
        .pool(single_slot())
        .sdk(PmPack)
        .sdk(KvPack::default())
        .build()
        .unwrap()
}

// ─── 1. pm.test() basic recording ─────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn pm_test_pass_recorded_in_metrics() {
    LocalSet::new()
        .run_until(async {
            let mut sb = pm_sandbox();
            let script = r#"
import { pm, results } from "sandbox:pm";
pm.test("two plus two", function() {
    pm.expect(2 + 2).to.equal(4);
});
return results();
"#;
            let r = sb.run(script).await.unwrap();
            assert_eq!(r.metrics.pm_tests.len(), 1);
            assert_eq!(r.metrics.pm_tests[0].name, "two plus two");
            assert!(r.metrics.pm_tests[0].passed);
            assert_eq!(r.value["pass"], json!(true));
            assert_eq!(r.value["failures"], json!([]));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn pm_test_fail_recorded_in_metrics() {
    LocalSet::new()
        .run_until(async {
            let mut sb = pm_sandbox();
            let script = r#"
import { pm, results } from "sandbox:pm";
pm.test("wrong math", function() {
    pm.expect(1).to.equal(2);
});
return results();
"#;
            let r = sb.run(script).await.unwrap();
            assert_eq!(r.metrics.pm_tests.len(), 1);
            assert!(!r.metrics.pm_tests[0].passed);
            assert_eq!(r.value["pass"], json!(false));
            let failures = r.value["failures"].as_array().unwrap();
            assert_eq!(failures.len(), 1);
            assert_eq!(failures[0].as_str().unwrap(), "wrong math");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn pm_multiple_tests_all_pass() {
    LocalSet::new()
        .run_until(async {
            let mut sb = pm_sandbox();
            let r = sb
                .run(
                    r#"
import { pm, results } from "sandbox:pm";
pm.test("a", function() { pm.expect(1).to.equal(1); });
pm.test("b", function() { pm.expect("hello").to.include("ell"); });
pm.test("c", function() { pm.expect([1,2,3]).to.lengthOf(3); });
return results();
"#,
                )
                .await
                .unwrap();
            assert_eq!(r.metrics.pm_tests.len(), 3);
            assert!(r.metrics.pm_tests.iter().all(|t| t.passed));
            assert_eq!(r.value["pass"], json!(true));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn pm_multiple_tests_mixed_pass_fail() {
    LocalSet::new()
        .run_until(async {
            let mut sb = pm_sandbox();
            let r = sb
                .run(
                    r#"
import { pm, results } from "sandbox:pm";
pm.test("pass", function() { pm.expect(1).to.equal(1); });
pm.test("fail", function() { pm.expect(1).to.equal(2); });
return results();
"#,
                )
                .await
                .unwrap();
            assert_eq!(r.metrics.pm_tests.len(), 2);
            assert!(r.metrics.pm_tests[0].passed);
            assert!(!r.metrics.pm_tests[1].passed);
            assert_eq!(r.value["pass"], json!(false));
        })
        .await;
}

// ─── 2. pm.expect() assertion chain ───────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn pm_expect_equal() {
    LocalSet::new()
        .run_until(async {
            let mut sb = pm_sandbox();
            let (val,) = {
                let r = sb
                    .run(
                        r#"
import { pm } from "sandbox:pm";
try { pm.expect(42).to.equal(42); return "pass"; } catch(e) { return "fail"; }
"#,
                    )
                    .await
                    .unwrap();
                (r.value,)
            };
            assert_eq!(val.as_str().unwrap(), "pass");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn pm_expect_not_equal() {
    LocalSet::new()
        .run_until(async {
            let mut sb = pm_sandbox();
            let r = sb
                .run(
                    r#"
import { pm } from "sandbox:pm";
try { pm.expect(1).to.not.equal(2); return "pass"; } catch(e) { return "fail"; }
"#,
                )
                .await
                .unwrap();
            assert_eq!(r.value.as_str().unwrap(), "pass");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn pm_expect_include_string() {
    LocalSet::new()
        .run_until(async {
            let mut sb = pm_sandbox();
            let r = sb
                .run(
                    r#"
import { pm } from "sandbox:pm";
try { pm.expect("hello world").to.include("world"); return "pass"; } catch(e) { return "fail"; }
"#,
                )
                .await
                .unwrap();
            assert_eq!(r.value.as_str().unwrap(), "pass");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn pm_expect_above_below() {
    LocalSet::new()
        .run_until(async {
            let mut sb = pm_sandbox();
            let r = sb
                .run(
                    r#"
import { pm } from "sandbox:pm";
try {
  pm.expect(10).to.be.above(5);
  pm.expect(3).to.be.below(10);
  return "pass";
} catch(e) { return "fail: " + e.message; }
"#,
                )
                .await
                .unwrap();
            assert_eq!(r.value.as_str().unwrap(), "pass");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn pm_expect_eql_deep_equality() {
    LocalSet::new()
        .run_until(async {
            let mut sb = pm_sandbox();
            let r = sb
                .run(
                    r#"
import { pm } from "sandbox:pm";
try {
  pm.expect({a: 1, b: [2, 3]}).to.eql({a: 1, b: [2, 3]});
  return "pass";
} catch(e) { return "fail: " + e.message; }
"#,
                )
                .await
                .unwrap();
            assert_eq!(r.value.as_str().unwrap(), "pass");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn pm_expect_type_check() {
    LocalSet::new()
        .run_until(async {
            let mut sb = pm_sandbox();
            let r = sb
                .run(
                    r#"
import { pm } from "sandbox:pm";
try {
  pm.expect("hello").to.be.a("string");
  pm.expect(42).to.be.a("number");
  pm.expect([]).to.be.an("array");
  return "pass";
} catch(e) { return "fail: " + e.message; }
"#,
                )
                .await
                .unwrap();
            assert_eq!(r.value.as_str().unwrap(), "pass");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn pm_expect_property() {
    LocalSet::new()
        .run_until(async {
            let mut sb = pm_sandbox();
            let r = sb
                .run(
                    r#"
import { pm } from "sandbox:pm";
try {
  pm.expect({x: 5}).to.have.property("x", 5);
  return "pass";
} catch(e) { return "fail: " + e.message; }
"#,
                )
                .await
                .unwrap();
            assert_eq!(r.value.as_str().unwrap(), "pass");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn pm_expect_ok_truthy_falsy() {
    LocalSet::new()
        .run_until(async {
            let mut sb = pm_sandbox();
            let r = sb
                .run(
                    r#"
import { pm } from "sandbox:pm";
try {
  pm.expect(1).to.be.ok;
  pm.expect(0).to.not.be.ok;
  pm.expect(true).to.be.true;
  pm.expect(false).to.be.false;
  return "pass";
} catch(e) { return "fail: " + e.message; }
"#,
                )
                .await
                .unwrap();
            assert_eq!(r.value.as_str().unwrap(), "pass");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn pm_expect_match_regex() {
    LocalSet::new()
        .run_until(async {
            let mut sb = pm_sandbox();
            let r = sb
                .run(
                    r#"
import { pm } from "sandbox:pm";
try {
    pm.expect("hello123").to.match(/\d+/);
    return "pass";
} catch(e) { return "fail: " + e.message; }
"#,
                )
                .await
                .unwrap();
            assert_eq!(r.value.as_str().unwrap(), "pass");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn pm_expect_starts_ends_with() {
    LocalSet::new()
        .run_until(async {
            let mut sb = pm_sandbox();
            let r = sb
                .run(
                    r#"
import { pm } from "sandbox:pm";
try {
    pm.expect("https://example.com").to.startsWith("https");
    pm.expect("hello.json").to.endsWith(".json");
    return "pass";
} catch(e) { return "fail: " + e.message; }
"#,
                )
                .await
                .unwrap();
            assert_eq!(r.value.as_str().unwrap(), "pass");
        })
        .await;
}

// ─── 3. pm.response lazy getter ───────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn pm_response_null_when_no_input() {
    LocalSet::new()
        .run_until(async {
            let mut sb = pm_sandbox();
            let r = sb
                .run(
                    r#"
import { pm, results } from "sandbox:pm";
pm.test("no response is null", function() {
    pm.expect(pm.response).to.equal(null);
});
return results();
"#,
                )
                .await
                .unwrap();
            assert!(r.metrics.pm_tests[0].passed, "pm.response should be null when no input set");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn pm_response_reads_input() {
    LocalSet::new()
        .run_until(async {
            let mut sb = pm_sandbox();
            sb.set_input(
                "_response",
                json!({
                    "status": 200,
                    "ok": true,
                    "headers": [["Content-Type", "application/json"]],
                    "body": "{\"id\": 42}",
                    "response_time_ms": 50
                }),
            );
            let r = sb
                .run(
                    r#"
import { pm, results } from "sandbox:pm";
pm.test("status 200", function() {
    pm.expect(pm.response.code).to.equal(200);
});
pm.test("header content-type", function() {
    pm.expect(pm.response.headers.get("content-type")).to.equal("application/json");
});
pm.test("json body", function() {
    var body = pm.response.json();
    pm.expect(body.id).to.equal(42);
});
return results();
"#,
                )
                .await
                .unwrap();
            assert_eq!(r.metrics.pm_tests.len(), 3);
            for t in &r.metrics.pm_tests {
                assert!(t.passed, "test '{}' should pass", t.name);
            }
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn pm_response_text_and_response_time() {
    LocalSet::new()
        .run_until(async {
            let mut sb = pm_sandbox();
            sb.set_input(
                "_response",
                json!({
                    "status": 201,
                    "ok": true,
                    "headers": [],
                    "body": "hello",
                    "response_time_ms": 123
                }),
            );
            let r = sb
                .run(
                    r#"
import { pm, results } from "sandbox:pm";
pm.test("text body", function() {
    pm.expect(pm.response.text()).to.equal("hello");
});
pm.test("response time", function() {
    pm.expect(pm.response.responseTime).to.equal(123);
});
return results();
"#,
                )
                .await
                .unwrap();
            for t in &r.metrics.pm_tests {
                assert!(t.passed, "test '{}' failed", t.name);
            }
        })
        .await;
}

// ─── 4. pm.environment / pm.variables ─────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn pm_environment_get_set() {
    LocalSet::new()
        .run_until(async {
            let mut sb = pm_sandbox();
            let r = sb
                .run(
                    r#"
import { pm } from "sandbox:pm";
pm.environment.set("base_url", "https://example.com");
return pm.environment.get("base_url");
"#,
                )
                .await
                .unwrap();
            assert_eq!(r.value.as_str().unwrap(), "https://example.com");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn pm_variables_unset() {
    LocalSet::new()
        .run_until(async {
            let mut sb = pm_sandbox();
            let r = sb
                .run(
                    r#"
import { pm } from "sandbox:pm";
pm.variables.set("x", "1");
pm.variables.unset("x");
return pm.variables.get("x");
"#,
                )
                .await
                .unwrap();
            assert!(r.value.is_null());
        })
        .await;
}

// ─── 5. results() resets state ────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn results_resets_pm_tests_between_runs() {
    LocalSet::new()
        .run_until(async {
            let mut sb = pm_sandbox();
            let script = r#"
import { pm, results } from "sandbox:pm";
pm.test("always fail", function() { pm.expect(1).to.equal(2); });
return results();
"#;
            // Run 1
            let r1 = sb.run(script).await.unwrap();
            assert_eq!(r1.metrics.pm_tests.len(), 1);
            assert!(!r1.metrics.pm_tests[0].passed);

            // Run 2 on warm slot — state should be reset
            let r2 = sb.run(script).await.unwrap();
            assert_eq!(
                r2.metrics.pm_tests.len(),
                1,
                "warm slot must see exactly 1 test, not accumulated"
            );
            assert!(!r2.metrics.pm_tests[0].passed);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn results_resets_environment_state() {
    LocalSet::new()
        .run_until(async {
            let mut sb = pm_sandbox();
            // Run 1: set env var, call results()
            sb.run(
                r#"
import { pm, results } from "sandbox:pm";
pm.environment.set("token", "secret");
return results();
"#,
            )
            .await
            .unwrap();

            // Run 2: env should be cleared (results() was called at end of run 1)
            let r2 = sb
                .run(
                    r#"
import { pm } from "sandbox:pm";
return pm.environment.get("token");
"#,
                )
                .await
                .unwrap();
            assert!(r2.value.is_null(), "environment should be cleared after results() call");
        })
        .await;
}

// ─── 6. Bruno compat: res / req / bru ────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn res_status_from_input() {
    LocalSet::new()
        .run_until(async {
            let mut sb = pm_sandbox();
            sb.set_input(
                "_response",
                json!({
                    "status": 404,
                    "ok": false,
                    "headers": [],
                    "body": "not found",
                    "response_time_ms": 10
                }),
            );
            let r = sb
                .run(
                    r#"
import { res } from "sandbox:pm";
return res.status;
"#,
                )
                .await
                .unwrap();
            assert_eq!(r.value.as_i64().unwrap(), 404);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn res_get_body_and_get_header() {
    LocalSet::new()
        .run_until(async {
            let mut sb = pm_sandbox();
            sb.set_input(
                "_response",
                json!({
                    "status": 200,
                    "ok": true,
                    "headers": [["X-Request-Id", "abc123"]],
                    "body": "body text",
                    "response_time_ms": 5
                }),
            );
            let r = sb
                .run(
                    r#"
import { res } from "sandbox:pm";
return { body: res.getBody(), header: res.getHeader("x-request-id") };
"#,
                )
                .await
                .unwrap();
            assert_eq!(r.value["body"].as_str().unwrap(), "body text");
            assert_eq!(r.value["header"].as_str().unwrap(), "abc123");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn bru_set_get_env_var() {
    LocalSet::new()
        .run_until(async {
            let mut sb = pm_sandbox();
            let r = sb
                .run(
                    r#"
import { bru } from "sandbox:pm";
bru.setEnvVar("myKey", "myValue");
return bru.getEnvVar("myKey");
"#,
                )
                .await
                .unwrap();
            assert_eq!(r.value.as_str().unwrap(), "myValue");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn global_test_and_expect_exports() {
    LocalSet::new()
        .run_until(async {
            let mut sb = pm_sandbox();
            let r = sb
                .run(
                    r#"
import { test, expect, results } from "sandbox:pm";
test("via global test()", function() {
    expect(5).to.be.above(3);
});
return results();
"#,
                )
                .await
                .unwrap();
            assert_eq!(r.metrics.pm_tests.len(), 1);
            assert!(r.metrics.pm_tests[0].passed);
        })
        .await;
}

// ─── 7. pm.request lazy getter ────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn pm_request_reads_input() {
    LocalSet::new()
        .run_until(async {
            let mut sb = pm_sandbox();
            sb.set_input(
                "_request",
                json!({
                    "url": "https://api.example.com/users",
                    "method": "POST",
                    "headers": [["Authorization", "Bearer tok"]],
                    "body": null
                }),
            );
            let r = sb
                .run(
                    r#"
import { pm } from "sandbox:pm";
return {
    url: pm.request.url,
    method: pm.request.method,
    auth: pm.request.headers.get("authorization")
};
"#,
                )
                .await
                .unwrap();
            assert_eq!(r.value["url"].as_str().unwrap(), "https://api.example.com/users");
            assert_eq!(r.value["method"].as_str().unwrap(), "POST");
            assert_eq!(r.value["auth"].as_str().unwrap(), "Bearer tok");
        })
        .await;
}

// ─── 8. pm_tests forwarded to RunMetrics ──────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn pm_tests_forwarded_to_run_metrics() {
    LocalSet::new()
        .run_until(async {
            let mut sb = pm_sandbox();
            let r = sb
                .run(
                    r#"
import { pm, results } from "sandbox:pm";
pm.test("check A", function() { pm.expect(1).to.equal(1); });
pm.test("check B", function() { pm.expect(2).to.equal(2); });
return results();
"#,
                )
                .await
                .unwrap();
            let names: Vec<&str> = r.metrics.pm_tests.iter().map(|t| t.name.as_str()).collect();
            assert_eq!(names, vec!["check A", "check B"]);
            assert!(r.metrics.pm_tests.iter().all(|t| t.passed));
        })
        .await;
}

// ─── 9. pm.expect null / undefined ────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn pm_expect_null_and_undefined() {
    LocalSet::new()
        .run_until(async {
            let mut sb = pm_sandbox();
            let r = sb
                .run(
                    r#"
import { pm } from "sandbox:pm";
try {
    pm.expect(null).to.be.null;
    pm.expect(undefined).to.be.undefined;
    return "pass";
} catch(e) { return "fail: " + e.message; }
"#,
                )
                .await
                .unwrap();
            assert_eq!(r.value.as_str().unwrap(), "pass");
        })
        .await;
}

// ─── 10. pm.expect empty ──────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn pm_expect_empty() {
    LocalSet::new()
        .run_until(async {
            let mut sb = pm_sandbox();
            let r = sb
                .run(
                    r#"
import { pm } from "sandbox:pm";
try {
    pm.expect("").to.be.empty;
    pm.expect([]).to.be.empty;
    pm.expect({}).to.be.empty;
    return "pass";
} catch(e) { return "fail: " + e.message; }
"#,
                )
                .await
                .unwrap();
            assert_eq!(r.value.as_str().unwrap(), "pass");
        })
        .await;
}

// ─── 11. pm.globals store ────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn pm_globals_get_set() {
    LocalSet::new()
        .run_until(async {
            let mut sb = pm_sandbox();
            let r = sb
                .run(
                    r#"
import { pm } from "sandbox:pm";
pm.globals.set("g", "global-value");
return pm.globals.get("g");
"#,
                )
                .await
                .unwrap();
            assert_eq!(r.value.as_str().unwrap(), "global-value");
        })
        .await;
}

// ─── 12. pm.collectionVariables alias ────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn pm_collection_variables_alias() {
    LocalSet::new()
        .run_until(async {
            let mut sb = pm_sandbox();
            let r = sb
                .run(
                    r#"
import { pm } from "sandbox:pm";
pm.collectionVariables.set("key", "cv-val");
return pm.variables.get("key");  // both share same store
"#,
                )
                .await
                .unwrap();
            assert_eq!(r.value.as_str().unwrap(), "cv-val");
        })
        .await;
}

// ─── 13. pm.expect least / most ──────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn pm_expect_least_most() {
    LocalSet::new()
        .run_until(async {
            let mut sb = pm_sandbox();
            let r = sb
                .run(
                    r#"
import { pm } from "sandbox:pm";
try {
    pm.expect(5).to.be.at.least(5);
    pm.expect(5).to.be.at.most(5);
    pm.expect(10).to.be.at.least(5);
    pm.expect(3).to.be.at.most(5);
    return "pass";
} catch(e) { return "fail: " + e.message; }
"#,
                )
                .await
                .unwrap();
            assert_eq!(r.value.as_str().unwrap(), "pass");
        })
        .await;
}
