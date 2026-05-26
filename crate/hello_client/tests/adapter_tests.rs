//! Integration tests for Postman and Bruno adapters.
//!
//! These are pure unit/logic tests — no sandbox or network required.

use hello_client::{BrunoAdapter, PostmanAdapter, PostmanError};
use hello_client::{HttpRequest, TestCase};
use std::collections::HashMap;

// ═══════════════════════════════════════════════════════════════════════════════
// Postman adapter tests
// ═══════════════════════════════════════════════════════════════════════════════

// ─── Import ───────────────────────────────────────────────────────────────────

#[test]
fn postman_import_minimal_get() {
    let json = r#"{
        "info": { "name": "My API", "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json" },
        "item": [{
            "name": "Get Users",
            "request": {
                "method": "GET",
                "url": { "raw": "https://api.example.com/users" }
            },
            "event": []
        }]
    }"#;
    let col = PostmanAdapter::import(json).unwrap();
    assert_eq!(col.name, "My API");
    assert_eq!(col.tests.len(), 1);
    let tc = &col.tests[0];
    assert_eq!(tc.name, "Get Users");
    assert_eq!(tc.request.method, "GET");
    assert_eq!(tc.request.url, "https://api.example.com/users");
    assert!(tc.pre_script.is_none());
    assert!(tc.post_script.is_none());
}

#[test]
fn postman_import_raw_string_url() {
    let json = r#"{
        "info": { "name": "Test", "schema": "v2.0" },
        "item": [{
            "name": "ping",
            "request": {
                "method": "GET",
                "url": "https://api.example.com/ping"
            },
            "event": []
        }]
    }"#;
    let col = PostmanAdapter::import(json).unwrap();
    assert_eq!(col.tests[0].request.url, "https://api.example.com/ping");
}

#[test]
fn postman_import_post_with_raw_body() {
    let json = r#"{
        "info": { "name": "Test", "schema": "v2.1" },
        "item": [{
            "name": "Create User",
            "request": {
                "method": "POST",
                "url": { "raw": "https://api.example.com/users" },
                "body": { "mode": "raw", "raw": "{\"name\": \"Alice\"}" }
            },
            "event": []
        }]
    }"#;
    let col = PostmanAdapter::import(json).unwrap();
    let tc = &col.tests[0];
    assert_eq!(tc.request.method, "POST");
    assert_eq!(tc.request.body.as_deref().unwrap(), "{\"name\": \"Alice\"}");
}

#[test]
fn postman_import_headers() {
    let json = r#"{
        "info": { "name": "Test", "schema": "v2.1" },
        "item": [{
            "name": "Req with headers",
            "request": {
                "method": "GET",
                "url": { "raw": "https://api.example.com/" },
                "header": [
                    { "key": "Accept", "value": "application/json" },
                    { "key": "X-Custom", "value": "value1" }
                ]
            },
            "event": []
        }]
    }"#;
    let col = PostmanAdapter::import(json).unwrap();
    let headers = &col.tests[0].request.headers;
    assert!(headers.iter().any(|(k, v)| k == "Accept" && v == "application/json"));
    assert!(headers.iter().any(|(k, v)| k == "X-Custom" && v == "value1"));
}

#[test]
fn postman_import_disabled_headers_skipped() {
    let json = r#"{
        "info": { "name": "Test", "schema": "v2.1" },
        "item": [{
            "name": "req",
            "request": {
                "method": "GET",
                "url": { "raw": "https://api.example.com/" },
                "header": [
                    { "key": "X-Active", "value": "yes", "disabled": false },
                    { "key": "X-Disabled", "value": "no", "disabled": true }
                ]
            },
            "event": []
        }]
    }"#;
    let col = PostmanAdapter::import(json).unwrap();
    let headers = &col.tests[0].request.headers;
    assert!(headers.iter().any(|(k, _)| k == "X-Active"));
    assert!(!headers.iter().any(|(k, _)| k == "X-Disabled"));
}

#[test]
fn postman_import_bearer_auth_v21() {
    let json = r#"{
        "info": { "name": "Test", "schema": "v2.1" },
        "item": [{
            "name": "auth req",
            "request": {
                "method": "GET",
                "url": { "raw": "https://api.example.com/me" },
                "auth": {
                    "type": "bearer",
                    "bearer": [{ "key": "token", "value": "my-token-abc" }]
                }
            },
            "event": []
        }]
    }"#;
    let col = PostmanAdapter::import(json).unwrap();
    let headers = &col.tests[0].request.headers;
    let auth = headers.iter().find(|(k, _)| k == "Authorization").map(|(_, v)| v.as_str());
    assert_eq!(auth, Some("Bearer my-token-abc"));
}

#[test]
fn postman_import_basic_auth_v20() {
    let json = r#"{
        "info": { "name": "Test", "schema": "v2.0" },
        "item": [{
            "name": "basic auth",
            "request": {
                "method": "GET",
                "url": { "raw": "https://api.example.com/secure" },
                "auth": {
                    "type": "basic",
                    "basic": { "username": "user", "password": "pass" }
                }
            },
            "event": []
        }]
    }"#;
    let col = PostmanAdapter::import(json).unwrap();
    let headers = &col.tests[0].request.headers;
    // Authorization: Basic base64("user:pass")
    let auth = headers.iter().find(|(k, _)| k == "Authorization").map(|(_, v)| v.clone());
    assert!(auth.is_some());
    let auth_val = auth.unwrap();
    assert!(auth_val.starts_with("Basic "));
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    let decoded = STANDARD.decode(&auth_val["Basic ".len()..]).unwrap();
    assert_eq!(std::str::from_utf8(&decoded).unwrap(), "user:pass");
}

#[test]
fn postman_import_pre_and_post_scripts() {
    let json = r#"{
        "info": { "name": "Test", "schema": "v2.1" },
        "item": [{
            "name": "scripted req",
            "request": {
                "method": "GET",
                "url": { "raw": "https://api.example.com/x" }
            },
            "event": [
                {
                    "listen": "prerequest",
                    "script": { "exec": "console.log('pre');" }
                },
                {
                    "listen": "test",
                    "script": { "exec": ["pm.test('ok', function() {", "  pm.expect(1).to.equal(1);", "});" ] }
                }
            ]
        }]
    }"#;
    let col = PostmanAdapter::import(json).unwrap();
    let tc = &col.tests[0];
    let pre = tc.pre_script.as_deref().unwrap();
    let post = tc.post_script.as_deref().unwrap();
    // Pre-script has import preamble
    assert!(pre.contains("sandbox:pm"));
    assert!(pre.contains("console.log('pre');"));
    // Post-script has import preamble + return results()
    assert!(post.contains("sandbox:pm"));
    assert!(post.contains("pm.test"));
    assert!(post.contains("return results();"));
}

#[test]
fn postman_import_exec_array_joined() {
    let json = r#"{
        "info": { "name": "Test", "schema": "v2.1" },
        "item": [{
            "name": "array exec",
            "request": { "method": "GET", "url": { "raw": "https://api.example.com/" } },
            "event": [{
                "listen": "test",
                "script": { "exec": ["line1", "line2", "line3"] }
            }]
        }]
    }"#;
    let col = PostmanAdapter::import(json).unwrap();
    let post = col.tests[0].post_script.as_deref().unwrap();
    assert!(post.contains("line1\nline2\nline3"));
}

#[test]
fn postman_import_variables() {
    let json = r#"{
        "info": { "name": "Test", "schema": "v2.1" },
        "item": [],
        "variable": [
            { "key": "base_url", "value": "https://api.example.com" },
            { "key": "version", "value": "v2" }
        ]
    }"#;
    let col = PostmanAdapter::import(json).unwrap();
    assert_eq!(col.variables.get("base_url").map(|s| s.as_str()), Some("https://api.example.com"));
    assert_eq!(col.variables.get("version").map(|s| s.as_str()), Some("v2"));
}

#[test]
fn postman_import_folder_flattened_with_path() {
    let json = r#"{
        "info": { "name": "Test", "schema": "v2.1" },
        "item": [{
            "name": "Users",
            "item": [
                {
                    "name": "Get All",
                    "request": { "method": "GET", "url": { "raw": "https://api.example.com/users" } },
                    "event": []
                },
                {
                    "name": "Get One",
                    "request": { "method": "GET", "url": { "raw": "https://api.example.com/users/1" } },
                    "event": []
                }
            ]
        }]
    }"#;
    let col = PostmanAdapter::import(json).unwrap();
    assert_eq!(col.tests.len(), 2);
    assert_eq!(col.tests[0].name, "Users/Get All");
    assert_eq!(col.tests[1].name, "Users/Get One");
}

#[test]
fn postman_import_nested_folders() {
    let json = r#"{
        "info": { "name": "Test", "schema": "v2.1" },
        "item": [{
            "name": "A",
            "item": [{
                "name": "B",
                "item": [{
                    "name": "Req",
                    "request": { "method": "DELETE", "url": { "raw": "https://api.example.com/x" } },
                    "event": []
                }]
            }]
        }]
    }"#;
    let col = PostmanAdapter::import(json).unwrap();
    assert_eq!(col.tests[0].name, "A/B/Req");
    assert_eq!(col.tests[0].request.method, "DELETE");
}

#[test]
fn postman_import_invalid_json_returns_error() {
    let result = PostmanAdapter::import("not json at all");
    assert!(result.is_err());
    assert!(matches!(result.err().unwrap(), PostmanError::Json(_)));
}

#[test]
fn postman_import_urlencoded_body() {
    let json = r#"{
        "info": { "name": "Test", "schema": "v2.1" },
        "item": [{
            "name": "form post",
            "request": {
                "method": "POST",
                "url": { "raw": "https://api.example.com/form" },
                "body": {
                    "mode": "urlencoded",
                    "urlencoded": [
                        { "key": "username", "value": "alice" },
                        { "key": "password", "value": "s3cr3t" }
                    ]
                }
            },
            "event": []
        }]
    }"#;
    let col = PostmanAdapter::import(json).unwrap();
    let body = col.tests[0].request.body.as_deref().unwrap();
    assert!(body.contains("username=alice"));
    assert!(body.contains("password=s3cr3t"));
}

// ─── Export ───────────────────────────────────────────────────────────────────

#[test]
fn postman_export_basic_structure() {
    let tests = vec![TestCase {
        name: "Get ping".to_string(),
        request: HttpRequest {
            url: "https://api.example.com/ping".to_string(),
            method: "GET".to_string(),
            headers: vec![],
            body: None,
        },
        ..Default::default()
    }];
    let vars = HashMap::new();
    let json_str = PostmanAdapter::export("My API", &tests, &vars);
    let v: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    assert_eq!(v["info"]["name"].as_str().unwrap(), "My API");
    assert!(v["info"]["schema"].as_str().unwrap().contains("v2.1"));
    assert_eq!(v["item"][0]["name"].as_str().unwrap(), "Get ping");
    assert_eq!(v["item"][0]["request"]["method"].as_str().unwrap(), "GET");
    assert_eq!(
        v["item"][0]["request"]["url"]["raw"].as_str().unwrap(),
        "https://api.example.com/ping"
    );
}

#[test]
fn postman_export_with_headers() {
    let tests = vec![TestCase {
        name: "req".to_string(),
        request: HttpRequest {
            url: "https://api.example.com/".to_string(),
            method: "POST".to_string(),
            headers: vec![("Content-Type".to_string(), "application/json".to_string())],
            body: Some("{\"x\": 1}".to_string()),
        },
        ..Default::default()
    }];
    let json_str = PostmanAdapter::export("T", &tests, &HashMap::new());
    let v: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    let headers = &v["item"][0]["request"]["header"];
    assert_eq!(headers[0]["key"].as_str().unwrap(), "Content-Type");
    assert_eq!(headers[0]["value"].as_str().unwrap(), "application/json");
    let body = &v["item"][0]["request"]["body"];
    assert_eq!(body["raw"].as_str().unwrap(), "{\"x\": 1}");
}

#[test]
fn postman_export_pre_post_scripts() {
    let tests = vec![TestCase {
        name: "scripted".to_string(),
        request: HttpRequest {
            url: "https://api.example.com/x".to_string(),
            method: "GET".to_string(),
            headers: vec![],
            body: None,
        },
        pre_script: Some("console.log('pre');".to_string()),
        post_script: Some("console.log('post');".to_string()),
        ..Default::default()
    }];
    let json_str = PostmanAdapter::export("T", &tests, &HashMap::new());
    let v: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    let events = &v["item"][0]["event"];
    let prerequest =
        events.as_array().unwrap().iter().find(|e| e["listen"].as_str() == Some("prerequest"));
    let test_ev = events.as_array().unwrap().iter().find(|e| e["listen"].as_str() == Some("test"));
    assert!(prerequest.is_some(), "prerequest event missing");
    assert!(test_ev.is_some(), "test event missing");
}

#[test]
fn postman_export_variables() {
    let tests = vec![];
    let mut vars = HashMap::new();
    vars.insert("base_url".to_string(), "https://example.com".to_string());
    let json_str = PostmanAdapter::export("T", &tests, &vars);
    let v: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    let var_arr = v["variable"].as_array().unwrap();
    assert!(!var_arr.is_empty());
    let found = var_arr.iter().find(|x| x["key"].as_str() == Some("base_url"));
    assert!(found.is_some());
    assert_eq!(found.unwrap()["value"].as_str().unwrap(), "https://example.com");
}

#[test]
fn postman_export_url_host_path_split() {
    let tests = vec![TestCase {
        name: "test".to_string(),
        request: HttpRequest {
            url: "https://api.example.com/users/123".to_string(),
            method: "GET".to_string(),
            headers: vec![],
            body: None,
        },
        ..Default::default()
    }];
    let json_str = PostmanAdapter::export("T", &tests, &HashMap::new());
    let v: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    let url = &v["item"][0]["request"]["url"];
    let host = url["host"].as_array().unwrap();
    let path = url["path"].as_array().unwrap();
    // host: ["api", "example", "com"]
    assert_eq!(host[0].as_str().unwrap(), "api");
    assert_eq!(host[1].as_str().unwrap(), "example");
    assert_eq!(host[2].as_str().unwrap(), "com");
    // path: ["users", "123"]
    assert_eq!(path[0].as_str().unwrap(), "users");
    assert_eq!(path[1].as_str().unwrap(), "123");
}

// ═══════════════════════════════════════════════════════════════════════════════
// Bruno adapter tests
// ═══════════════════════════════════════════════════════════════════════════════

// ─── Import ───────────────────────────────────────────────────────────────────

#[test]
fn bruno_import_minimal_get() {
    let bru = r#"meta {
  name: Get Users
  type: http
  seq: 1
}

get {
  url: https://api.example.com/users
  body: none
}
"#;
    let tc = BrunoAdapter::import(bru).unwrap();
    assert_eq!(tc.name, "Get Users");
    assert_eq!(tc.request.method, "GET");
    assert_eq!(tc.request.url, "https://api.example.com/users");
    assert!(tc.pre_script.is_none());
    assert!(tc.post_script.is_none());
}

#[test]
fn bruno_import_post_with_json_body() {
    let bru = r#"meta {
  name: Create User
  type: http
  seq: 1
}

post {
  url: https://api.example.com/users
  body: json
}

body:json {
  {"name": "Alice", "email": "alice@example.com"}
}
"#;
    let tc = BrunoAdapter::import(bru).unwrap();
    assert_eq!(tc.request.method, "POST");
    let body = tc.request.body.as_deref().unwrap();
    assert!(body.contains("Alice"));
    // body:json auto-adds Content-Type header
    assert!(tc.request.headers.iter().any(|(k, v)| k == "Content-Type" && v == "application/json"));
}

#[test]
fn bruno_import_headers() {
    let bru = r#"meta {
  name: With Headers
  type: http
  seq: 1
}

get {
  url: https://api.example.com/data
  body: none
}

headers {
  Accept: application/json
  X-API-Version: 2
}
"#;
    let tc = BrunoAdapter::import(bru).unwrap();
    assert!(tc.request.headers.iter().any(|(k, v)| k == "Accept" && v == "application/json"));
    assert!(tc.request.headers.iter().any(|(k, v)| k == "X-API-Version" && v == "2"));
}

#[test]
fn bruno_import_auth_bearer() {
    let bru = r#"meta {
  name: Bearer Auth
  type: http
  seq: 1
}

get {
  url: https://api.example.com/me
  body: none
}

auth:bearer {
  token: my-secret-token
}
"#;
    let tc = BrunoAdapter::import(bru).unwrap();
    let auth = tc.request.headers.iter().find(|(k, _)| k == "Authorization");
    assert_eq!(auth.map(|(_, v)| v.as_str()), Some("Bearer my-secret-token"));
}

#[test]
fn bruno_import_auth_basic() {
    let bru = r#"meta {
  name: Basic Auth
  type: http
  seq: 1
}

get {
  url: https://api.example.com/secure
  body: none
}

auth:basic {
  username: admin
  password: secret
}
"#;
    let tc = BrunoAdapter::import(bru).unwrap();
    let auth =
        tc.request.headers.iter().find(|(k, _)| k == "Authorization").map(|(_, v)| v.clone());
    assert!(auth.is_some());
    let auth_val = auth.unwrap();
    assert!(auth_val.starts_with("Basic "));
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    let decoded = STANDARD.decode(&auth_val["Basic ".len()..]).unwrap();
    assert_eq!(std::str::from_utf8(&decoded).unwrap(), "admin:secret");
}

#[test]
fn bruno_import_auth_apikey_header() {
    let bru = r#"meta {
  name: API Key Auth
  type: http
  seq: 1
}

get {
  url: https://api.example.com/data
  body: none
}

auth:apikey {
  key: X-API-Key
  value: key-value-123
  in: header
}
"#;
    let tc = BrunoAdapter::import(bru).unwrap();
    assert!(tc.request.headers.iter().any(|(k, v)| k == "X-API-Key" && v == "key-value-123"));
}

#[test]
fn bruno_import_pre_script() {
    let bru = r#"meta {
  name: Scripted
  type: http
  seq: 1
}

get {
  url: https://api.example.com/x
  body: none
}

script:pre-request {
  console.log("before request");
  bru.setEnvVar("ts", Date.now().toString());
}
"#;
    let tc = BrunoAdapter::import(bru).unwrap();
    let pre = tc.pre_script.as_deref().unwrap();
    assert!(pre.contains("sandbox:pm"));
    assert!(pre.contains("console.log"));
    assert!(pre.contains("bru.setEnvVar"));
}

#[test]
fn bruno_import_post_script() {
    let bru = r#"meta {
  name: With Tests
  type: http
  seq: 1
}

get {
  url: https://api.example.com/x
  body: none
}

script:post-response {
  test("status is 200", function() {
    expect(res.status).to.equal(200);
  });
}
"#;
    let tc = BrunoAdapter::import(bru).unwrap();
    let post = tc.post_script.as_deref().unwrap();
    assert!(post.contains("sandbox:pm"));
    assert!(post.contains("return results();"));
    assert!(post.contains("status is 200"));
}

#[test]
fn bruno_import_test_section() {
    let bru = r#"meta {
  name: With Test Section
  type: http
  seq: 1
}

get {
  url: https://api.example.com/
  body: none
}

test {
  expect(res.status).to.equal(200);
}
"#;
    let tc = BrunoAdapter::import(bru).unwrap();
    let post = tc.post_script.as_deref().unwrap();
    assert!(post.contains("sandbox:pm"));
    assert!(post.contains("return results();"));
}

#[test]
fn bruno_import_assert_eq() {
    let bru = r#"meta {
  name: Assertions
  type: http
  seq: 1
}

get {
  url: https://api.example.com/
  body: none
}

assert {
  res.status: eq 200
  res.body: contains hello
}
"#;
    let tc = BrunoAdapter::import(bru).unwrap();
    let post = tc.post_script.as_deref().unwrap();
    assert!(post.contains("equal(200)"));
    assert!(post.contains("include"));
}

#[test]
fn bruno_import_assert_various_operators() {
    let bru = r#"meta {
  name: Asserts
  type: http
  seq: 1
}

get {
  url: https://api.example.com/
  body: none
}

assert {
  res.status: gt 100
  res.status: gte 200
  res.status: lt 500
  res.status: lte 200
  res.status: neq 404
  res.status: isDefined
}
"#;
    let tc = BrunoAdapter::import(bru).unwrap();
    let post = tc.post_script.as_deref().unwrap();
    assert!(post.contains("above"));
    assert!(post.contains("least"));
    assert!(post.contains("below"));
    assert!(post.contains("most"));
    assert!(post.contains("not.equal"));
    assert!(post.contains("isDefined"));
}

#[test]
fn bruno_import_params_query_appended_to_url() {
    let bru = r#"meta {
  name: Query Params
  type: http
  seq: 1
}

get {
  url: https://api.example.com/search
  body: none
}

params:query {
  q: hello world
  page: 1
}
"#;
    let tc = BrunoAdapter::import(bru).unwrap();
    let url = &tc.request.url;
    assert!(url.contains("https://api.example.com/search?"));
    // percent-encoded
    assert!(
        url.contains("q=hello%20world")
            || url.contains("q=hello+world")
            || url.contains("q=hello%20world")
    );
    assert!(url.contains("page=1"));
}

#[test]
fn bruno_import_name_fallback_to_url() {
    let bru = r#"get {
  url: https://api.example.com/fallback
  body: none
}
"#;
    let tc = BrunoAdapter::import(bru).unwrap();
    // No meta block — name falls back to URL
    assert_eq!(tc.name, "https://api.example.com/fallback");
}

#[test]
fn bruno_import_disabled_lines_skipped() {
    let bru = r#"meta {
  name: Test
  type: http
  seq: 1
}

get {
  url: https://api.example.com/
  body: none
}

headers {
  X-Active: yes
  ~X-Disabled: no
}
"#;
    let tc = BrunoAdapter::import(bru).unwrap();
    assert!(tc.request.headers.iter().any(|(k, v)| k == "X-Active" && v == "yes"));
    assert!(!tc.request.headers.iter().any(|(k, _)| k == "X-Disabled" || k.starts_with("~")));
}

#[test]
fn bruno_import_body_form_urlencoded() {
    let bru = r#"meta {
  name: Form Post
  type: http
  seq: 1
}

post {
  url: https://api.example.com/login
  body: urlencoded
}

body:form-urlencoded {
  username: alice
  password: s3cr3t
}
"#;
    let tc = BrunoAdapter::import(bru).unwrap();
    let body = tc.request.body.as_deref().unwrap();
    assert!(body.contains("username=alice"));
    assert!(body.contains("password=s3cr3t"));
    assert!(
        tc.request.headers.iter().any(|(k, v)| k == "Content-Type" && v.contains("urlencoded"))
    );
}

// ─── Export ───────────────────────────────────────────────────────────────────

#[test]
fn bruno_export_basic_get() {
    let tc = TestCase {
        name: "Get Users".to_string(),
        request: HttpRequest {
            url: "https://api.example.com/users".to_string(),
            method: "GET".to_string(),
            headers: vec![],
            body: None,
        },
        ..Default::default()
    };
    let bru = BrunoAdapter::export(&tc);
    assert!(bru.contains("name: Get Users"));
    assert!(bru.contains("get {"));
    assert!(bru.contains("url: https://api.example.com/users"));
}

#[test]
fn bruno_export_post_with_json_body() {
    let tc = TestCase {
        name: "Create".to_string(),
        request: HttpRequest {
            url: "https://api.example.com/users".to_string(),
            method: "POST".to_string(),
            headers: vec![],
            body: Some("{\"name\": \"Alice\"}".to_string()),
        },
        ..Default::default()
    };
    let bru = BrunoAdapter::export(&tc);
    assert!(bru.contains("post {"));
    assert!(bru.contains("body:json {"));
    assert!(bru.contains("Alice"));
}

#[test]
fn bruno_export_bearer_auth_extracted() {
    let tc = TestCase {
        name: "Auth Request".to_string(),
        request: HttpRequest {
            url: "https://api.example.com/me".to_string(),
            method: "GET".to_string(),
            headers: vec![("Authorization".to_string(), "Bearer my-token".to_string())],
            body: None,
        },
        ..Default::default()
    };
    let bru = BrunoAdapter::export(&tc);
    assert!(bru.contains("auth:bearer {"));
    assert!(bru.contains("token: my-token"));
    // Authorization header should NOT appear in headers block (moved to auth:bearer)
    let headers_start = bru.find("headers {").is_some();
    if headers_start {
        // If headers block exists, it shouldn't contain the Authorization key
        assert!(!bru.contains("  Authorization: Bearer"));
    }
}

#[test]
fn bruno_export_basic_auth_decoded() {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    let encoded = STANDARD.encode("user:pass");
    let tc = TestCase {
        name: "Basic".to_string(),
        request: HttpRequest {
            url: "https://api.example.com/".to_string(),
            method: "GET".to_string(),
            headers: vec![("Authorization".to_string(), format!("Basic {}", encoded))],
            body: None,
        },
        ..Default::default()
    };
    let bru = BrunoAdapter::export(&tc);
    assert!(bru.contains("auth:basic {"));
    assert!(bru.contains("username: user"));
    assert!(bru.contains("password: pass"));
}

#[test]
fn bruno_export_with_pre_post_scripts() {
    let tc = TestCase {
        name: "Scripted".to_string(),
        request: HttpRequest {
            url: "https://api.example.com/x".to_string(),
            method: "GET".to_string(),
            headers: vec![],
            body: None,
        },
        pre_script: Some("console.log('pre');".to_string()),
        post_script: Some("console.log('post');".to_string()),
        ..Default::default()
    };
    let bru = BrunoAdapter::export(&tc);
    assert!(bru.contains("script:pre-request {"));
    assert!(bru.contains("console.log('pre');"));
    assert!(bru.contains("test {"));
    assert!(bru.contains("console.log('post');"));
}

#[test]
fn bruno_export_query_params_from_url() {
    let tc = TestCase {
        name: "Search".to_string(),
        request: HttpRequest {
            url: "https://api.example.com/search?q=rust&page=1".to_string(),
            method: "GET".to_string(),
            headers: vec![],
            body: None,
        },
        ..Default::default()
    };
    let bru = BrunoAdapter::export(&tc);
    assert!(bru.contains("params:query {"));
    assert!(bru.contains("q: rust"));
    assert!(bru.contains("page: 1"));
}

// ─── Roundtrip ────────────────────────────────────────────────────────────────

#[test]
fn bruno_roundtrip_basic() {
    let tc = TestCase {
        name: "Roundtrip Test".to_string(),
        request: HttpRequest {
            url: "https://api.example.com/items".to_string(),
            method: "GET".to_string(),
            headers: vec![("Accept".to_string(), "application/json".to_string())],
            body: None,
        },
        ..Default::default()
    };
    let exported = BrunoAdapter::export(&tc);
    let imported = BrunoAdapter::import(&exported).unwrap();
    assert_eq!(imported.name, "Roundtrip Test");
    assert_eq!(imported.request.url, "https://api.example.com/items");
    assert_eq!(imported.request.method, "GET");
    assert!(imported.request.headers.iter().any(|(k, v)| k == "Accept" && v == "application/json"));
}

#[test]
fn postman_roundtrip_basic() {
    let tests = vec![TestCase {
        name: "Roundtrip".to_string(),
        request: HttpRequest {
            url: "https://api.example.com/test".to_string(),
            method: "POST".to_string(),
            headers: vec![("Content-Type".to_string(), "application/json".to_string())],
            body: Some("{\"x\": 1}".to_string()),
        },
        ..Default::default()
    }];
    let vars = HashMap::new();
    let exported = PostmanAdapter::export("My Collection", &tests, &vars);
    let imported = PostmanAdapter::import(&exported).unwrap();
    assert_eq!(imported.name, "My Collection");
    assert_eq!(imported.tests.len(), 1);
    let tc = &imported.tests[0];
    assert_eq!(tc.name, "Roundtrip");
    assert_eq!(tc.request.method, "POST");
    assert_eq!(tc.request.url, "https://api.example.com/test");
    let ct = tc.request.headers.iter().find(|(k, _)| k == "Content-Type");
    assert_eq!(ct.map(|(_, v)| v.as_str()), Some("application/json"));
}
