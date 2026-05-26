//! Import/export adapter for OpenAPI 2.0 (Swagger) and OpenAPI 3.x.
//!
//! ## Import
//! Accepts YAML or JSON input (JSON is valid YAML).  Auto-detects the
//! version by the presence of the `openapi` (3.x) or `swagger` (2.0) key.
//! Produces one [`TestCase`] per path × HTTP-method combination.
//! OpenAPI path parameters (`{name}`) are converted to `{{name}}`.
//! Example bodies from the spec are used as request bodies where available.
//!
//! ## Export
//! Generates an approximate OpenAPI 3.0 YAML spec from a flat list of
//! [`TestCase`] objects.  The spec is intended as a starting point for
//! documentation, not a complete round-trip.

use std::collections::{BTreeMap, BTreeSet};

use serde_yaml::{Mapping, Value};

use crate::types::{HttpRequest, TestCase};

// --- Error --------------------------------------------------------------------

/// Errors from the OpenAPI adapter.
#[derive(Debug, thiserror::Error)]
pub enum OpenApiError {
    #[error("YAML/JSON parse error: {0}")]
    Parse(#[from] serde_yaml::Error),
    #[error("not a valid OpenAPI document: missing 'openapi' or 'swagger' field")]
    NotOpenApi,
}

// --- Public types -------------------------------------------------------------

/// A collection of test cases imported from an OpenAPI spec.
pub struct OpenApiCollection {
    pub name: String,
    pub tests: Vec<TestCase>,
}

// --- Adapter -----------------------------------------------------------------

/// Import/export adapter for OpenAPI 2.0 (Swagger) and OpenAPI 3.x.
pub struct OpenApiAdapter;

#[derive(Clone, Copy)]
enum Version {
    V2,
    V3,
}

const HTTP_METHODS: &[&str] = &["get", "post", "put", "patch", "delete", "head", "options"];

impl OpenApiAdapter {
    /// Import an OpenAPI spec (YAML or JSON) into test cases.
    pub fn import(input: &str) -> Result<OpenApiCollection, OpenApiError> {
        let root: Value = serde_yaml::from_str(input)?;

        let version = detect_version(&root).ok_or(OpenApiError::NotOpenApi)?;

        let name = root
            .get("info")
            .and_then(|i| i.get("title"))
            .and_then(Value::as_str)
            .unwrap_or("Unnamed API")
            .to_string();

        let base_url = extract_base_url(&root, version);
        let tests = extract_test_cases(&root, &base_url, version);

        Ok(OpenApiCollection { name, tests })
    }

    /// Export test cases to an OpenAPI 3.0 YAML spec.
    pub fn export(name: &str, tests: &[TestCase]) -> String {
        build_openapi_yaml(name, tests)
    }
}

// --- Version detection --------------------------------------------------------

fn detect_version(root: &Value) -> Option<Version> {
    if root.get("openapi").is_some() {
        Some(Version::V3)
    } else if root.get("swagger").is_some() {
        Some(Version::V2)
    } else {
        None
    }
}

// --- Import: base URL ---------------------------------------------------------

fn extract_base_url(root: &Value, version: Version) -> String {
    match version {
        Version::V3 => root
            .get("servers")
            .and_then(Value::as_sequence)
            .and_then(|s| s.first())
            .and_then(|s| s.get("url"))
            .and_then(Value::as_str)
            .unwrap_or("https://localhost")
            .trim_end_matches('/')
            .to_string(),

        Version::V2 => {
            let scheme = root
                .get("schemes")
                .and_then(Value::as_sequence)
                .and_then(|s| s.first())
                .and_then(Value::as_str)
                .unwrap_or("https");
            let host = root.get("host").and_then(Value::as_str).unwrap_or("localhost");
            let base = root.get("basePath").and_then(Value::as_str).unwrap_or("");
            format!("{scheme}://{host}{}", base.trim_end_matches('/'))
        },
    }
}

// --- Import: operations → TestCase -------------------------------------------

fn extract_test_cases(root: &Value, base_url: &str, version: Version) -> Vec<TestCase> {
    let Some(paths) = root.get("paths").and_then(Value::as_mapping) else {
        return vec![];
    };

    let mut tests = Vec::new();

    for (path_key, path_item) in paths {
        let path_str = path_key.as_str().unwrap_or("");
        // OpenAPI {param} → our {{param}} interpolation syntax
        let converted_path = path_str.replace('{', "{{").replace('}', "}}");
        let url = format!("{base_url}{converted_path}");

        for &method in HTTP_METHODS {
            let Some(op) = path_item.get(method) else {
                continue;
            };
            tests.push(build_test_case(op, &url, method, path_str, version));
        }
    }

    tests
}

fn build_test_case(op: &Value, url: &str, method: &str, path: &str, version: Version) -> TestCase {
    let name = op
        .get("operationId")
        .or_else(|| op.get("summary"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("{} {path}", method.to_uppercase()));

    let (headers, body) = match version {
        Version::V3 => extract_body_v3(op),
        Version::V2 => extract_body_v2(op),
    };

    TestCase {
        name,
        request: HttpRequest {
            url: url.to_string(),
            method: method.to_uppercase(),
            headers,
            body,
        },
        ..Default::default()
    }
}

// --- Import: request body extraction -----------------------------------------

fn extract_body_v3(op: &Value) -> (Vec<(String, String)>, Option<String>) {
    let Some(rb) = op.get("requestBody") else {
        return (vec![], None);
    };
    let Some(content) = rb.get("content").and_then(Value::as_mapping) else {
        return (vec![], None);
    };

    // Prefer application/json; fall back to first available media type.
    let (media_type, media_obj) = content
        .iter()
        .find(|(k, _)| k.as_str() == Some("application/json"))
        .or_else(|| content.iter().next())
        .map(|(k, v)| (k.as_str().unwrap_or("application/json"), v))
        .unwrap_or(("application/json", &Value::Null));

    let body = extract_example(media_obj);
    let headers = if body.is_some() {
        vec![("Content-Type".to_string(), media_type.to_string())]
    } else {
        vec![]
    };

    (headers, body)
}

fn extract_body_v2(op: &Value) -> (Vec<(String, String)>, Option<String>) {
    let body_param = op
        .get("parameters")
        .and_then(Value::as_sequence)
        .and_then(|ps| ps.iter().find(|p| p.get("in").and_then(Value::as_str) == Some("body")));

    let body = body_param.and_then(|p| p.get("schema")).and_then(extract_example);

    let consumes = op
        .get("consumes")
        .and_then(Value::as_sequence)
        .and_then(|s| s.first())
        .and_then(Value::as_str)
        .unwrap_or("application/json")
        .to_string();

    let headers = if body.is_some() {
        vec![("Content-Type".to_string(), consumes)]
    } else {
        vec![]
    };

    (headers, body)
}

/// Try to pull an example value from a schema or media-type object and
/// serialize it as a JSON string for use as a request body.
fn extract_example(obj: &Value) -> Option<String> {
    let example = obj
        .get("example")
        .or_else(|| obj.get("schema").and_then(|s| s.get("example")))
        .or_else(|| obj.get("schema").and_then(|s| s.get("default")));

    example.and_then(|e| {
        let as_json = yaml_to_json(e);
        serde_json::to_string_pretty(&as_json).ok()
    })
}

// --- Export: TestCase → OpenAPI 3.0 YAML -------------------------------------

fn build_openapi_yaml(name: &str, tests: &[TestCase]) -> String {
    let mut root = Mapping::new();

    root.insert(s("openapi"), s("3.0.0"));

    // info
    let mut info = Mapping::new();
    info.insert(s("title"), s(name));
    info.insert(s("version"), s("1.0.0"));
    root.insert(s("info"), Value::Mapping(info));

    // servers — deduplicated, order-stable
    let mut seen_servers: BTreeSet<String> = BTreeSet::new();
    let mut servers: Vec<Value> = Vec::new();
    for tc in tests {
        if let Some(base) = server_prefix(&tc.request.url)
            && seen_servers.insert(base.clone())
        {
            let mut srv = Mapping::new();
            srv.insert(s("url"), s(&base));
            servers.push(Value::Mapping(srv));
        }
    }
    if !servers.is_empty() {
        root.insert(s("servers"), Value::Sequence(servers));
    }

    // paths — collect (path, method, TestCase) grouped by path
    let mut by_path: BTreeMap<String, Vec<(String, &TestCase)>> = BTreeMap::new();
    for tc in tests {
        let path = openapi_path(&tc.request.url);
        by_path.entry(path).or_default().push((tc.request.method.to_lowercase(), tc));
    }

    let mut paths = Mapping::new();
    for (path, ops) in &by_path {
        let mut path_item = Mapping::new();

        for (method, tc) in ops {
            let mut op = Mapping::new();
            op.insert(s("summary"), s(&tc.name));
            op.insert(s("operationId"), s(&slugify(&tc.name)));

            // Path parameters from `{param}` segments in path
            let params: Vec<Value> = path_params(path)
                .into_iter()
                .map(|p| {
                    let mut pm = Mapping::new();
                    pm.insert(s("name"), s(&p));
                    pm.insert(s("in"), s("path"));
                    pm.insert(s("required"), Value::Bool(true));
                    let mut schema = Mapping::new();
                    schema.insert(s("type"), s("string"));
                    pm.insert(s("schema"), Value::Mapping(schema));
                    Value::Mapping(pm)
                })
                .collect();
            if !params.is_empty() {
                op.insert(s("parameters"), Value::Sequence(params));
            }

            // requestBody
            if let Some(ref body) = tc.request.body {
                let ct = tc
                    .request
                    .headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
                    .map(|(_, v)| v.as_str())
                    .unwrap_or("application/json");

                let mut media = Mapping::new();
                if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(body) {
                    let mut schema = Mapping::new();
                    schema.insert(s("type"), s("object"));
                    schema.insert(s("example"), json_to_yaml(&json_val));
                    media.insert(s("schema"), Value::Mapping(schema));
                }
                let mut content = Mapping::new();
                content.insert(s(ct), Value::Mapping(media));
                let mut rb = Mapping::new();
                rb.insert(s("required"), Value::Bool(true));
                rb.insert(s("content"), Value::Mapping(content));
                op.insert(s("requestBody"), Value::Mapping(rb));
            }

            // responses — minimal placeholder
            let mut ok = Mapping::new();
            ok.insert(s("description"), s("OK"));
            let mut responses = Mapping::new();
            responses.insert(s("'200'"), Value::Mapping(ok));
            op.insert(s("responses"), Value::Mapping(responses));

            path_item.insert(s(method), Value::Mapping(op));
        }

        paths.insert(s(path), Value::Mapping(path_item));
    }
    root.insert(s("paths"), Value::Mapping(paths));

    serde_yaml::to_string(&Value::Mapping(root)).unwrap_or_default()
}

// --- Helpers ------------------------------------------------------------------

/// Convenience: create a YAML string `Value`.
fn s(v: &str) -> Value {
    Value::String(v.to_string())
}

/// Convert `{{param}}` URL to an OpenAPI path `{param}` and strip the server prefix.
fn openapi_path(url: &str) -> String {
    let path = if let Some(pos) = url.find("://") {
        let rest = &url[pos + 3..];
        match rest.find('/') {
            Some(i) => &rest[i..],
            None => "/",
        }
    } else {
        url
    };
    path.replace("{{", "{").replace("}}", "}")
}

/// Extract `scheme://host` prefix from a URL (used for the `servers` list).
fn server_prefix(url: &str) -> Option<String> {
    if url.starts_with("{{") {
        return None; // template URL — no literal server
    }
    let pos = url.find("://")?;
    let rest = &url[pos + 3..];
    let host_end = rest.find('/').unwrap_or(rest.len());
    Some(format!("{}{}", &url[..pos + 3], &rest[..host_end]))
}

/// Extract `{name}` path parameter names from an OpenAPI path string.
fn path_params(path: &str) -> Vec<String> {
    let mut params = Vec::new();
    let mut rest = path;
    while let Some(open) = rest.find('{') {
        rest = &rest[open + 1..];
        if let Some(close) = rest.find('}') {
            let name = rest[..close].trim().to_string();
            if !name.is_empty() {
                params.push(name);
            }
            rest = &rest[close + 1..];
        } else {
            break;
        }
    }
    params
}

/// Turn a display name into a URL-safe operationId.
fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut capitalize_next = false;
    for ch in name.chars() {
        if ch.is_alphanumeric() {
            if capitalize_next {
                out.extend(ch.to_uppercase());
                capitalize_next = false;
            } else {
                out.push(ch);
            }
        } else {
            capitalize_next = true;
        }
    }
    out
}

// --- Value conversion helpers -------------------------------------------------

/// Convert `serde_yaml::Value` to `serde_json::Value` via the serde data model.
fn yaml_to_json(v: &Value) -> serde_json::Value {
    serde_json::to_value(v).unwrap_or(serde_json::Value::Null)
}

/// Convert `serde_json::Value` to `serde_yaml::Value` via the serde data model.
fn json_to_yaml(v: &serde_json::Value) -> Value {
    serde_yaml::to_value(v).unwrap_or(Value::Null)
}

// --- Tests --------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const OPENAPI3: &str = r#"
openapi: "3.0.0"
info:
  title: Test API
  version: "1.0.0"
servers:
  - url: https://api.example.com
paths:
  /users:
    get:
      operationId: listUsers
      summary: List users
      responses:
        "200":
          description: OK
    post:
      operationId: createUser
      summary: Create user
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              example:
                name: Alice
      responses:
        "201":
          description: Created
  /users/{id}:
    get:
      operationId: getUser
      summary: Get user by ID
      responses:
        "200":
          description: OK
"#;

    const SWAGGER2: &str = r#"
swagger: "2.0"
info:
  title: Swagger API
host: api.example.com
basePath: /v1
schemes:
  - https
paths:
  /items:
    get:
      operationId: listItems
      summary: List items
      responses:
        200:
          description: OK
    post:
      operationId: createItem
      parameters:
        - in: body
          name: body
          schema:
            type: object
            example:
              title: My Item
      responses:
        201:
          description: Created
"#;

    #[test]
    fn import_openapi3_basic() {
        let col = OpenApiAdapter::import(OPENAPI3).unwrap();
        assert_eq!(col.name, "Test API");
        assert_eq!(col.tests.len(), 3);

        let list = col.tests.iter().find(|t| t.name == "listUsers").unwrap();
        assert_eq!(list.request.method, "GET");
        assert_eq!(list.request.url, "https://api.example.com/users");
        assert!(list.request.body.is_none());

        let create = col.tests.iter().find(|t| t.name == "createUser").unwrap();
        assert_eq!(create.request.method, "POST");
        assert!(create.request.body.as_ref().unwrap().contains("Alice"));
        assert!(create.request.headers.iter().any(|(k, _)| k == "Content-Type"));
    }

    #[test]
    fn import_openapi3_path_param_converted() {
        let col = OpenApiAdapter::import(OPENAPI3).unwrap();
        let get_user = col.tests.iter().find(|t| t.name == "getUser").unwrap();
        assert!(get_user.request.url.contains("{{id}}"), "url: {}", get_user.request.url);
    }

    #[test]
    fn import_swagger2_basic() {
        let col = OpenApiAdapter::import(SWAGGER2).unwrap();
        assert_eq!(col.name, "Swagger API");
        assert_eq!(col.tests.len(), 2);

        let list = col.tests.iter().find(|t| t.name == "listItems").unwrap();
        assert_eq!(list.request.url, "https://api.example.com/v1/items");

        let create = col.tests.iter().find(|t| t.name == "createItem").unwrap();
        assert!(create.request.body.as_ref().unwrap().contains("My Item"));
    }

    #[test]
    fn import_rejects_non_openapi() {
        let yaml = "name: not openapi\nrequests: []\n";
        assert!(matches!(OpenApiAdapter::import(yaml), Err(OpenApiError::NotOpenApi)));
    }

    #[test]
    fn export_produces_valid_yaml() {
        let col = OpenApiAdapter::import(OPENAPI3).unwrap();
        let yaml = OpenApiAdapter::export(&col.name, &col.tests);
        // Must be valid YAML
        let parsed: Value = serde_yaml::from_str(&yaml).expect("export is valid YAML");
        assert_eq!(parsed.get("openapi").and_then(Value::as_str), Some("3.0.0"));
        assert!(parsed.get("paths").is_some());
    }

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("Get User by ID"), "GetUserByID");
        assert_eq!(slugify("list-items"), "listItems");
        assert_eq!(slugify("simple"), "simple");
    }
}
