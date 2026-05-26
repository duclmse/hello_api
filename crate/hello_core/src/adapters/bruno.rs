//! Bruno `.bru` file format import and export adapter.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::path::Path;

use super::bru_parser::{parse_kv, parse_sections};
use crate::types::HttpRequest;
use crate::types::TestCase;

// ─── Preamble constants ───────────────────────────────────────────────────────

const BRU_PRE_PREAMBLE: &str = "import { pm, bru, req } from \"sandbox:pm\";\n";
const BRU_POST_PREAMBLE: &str =
    "import { pm, res, bru, test, expect, results } from \"sandbox:pm\";\n";
const BRU_POST_SUFFIX: &str = "\nreturn results();";

// ─── Error type ───────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum BrunoError {
    #[error("parse error: {0}")]
    Parse(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

// ─── Section / KV parsing — delegated to bru_parser ─────────────────────────

fn parse_bru_sections(input: &str) -> Vec<(String, String)> {
    parse_sections(input).into_iter().map(|s| (s.name, s.content)).collect()
}

fn parse_bru_kv(content: &str) -> Vec<(String, String)> {
    parse_kv(content)
}

// ─── Assert block conversion ──────────────────────────────────────────────────

/// Convert a Bruno `assert` section into JavaScript test statements.
///
/// Each line has the form `path: operator value`, e.g. `res.status: eq 200`.
fn assert_to_js(content: &str) -> String {
    let helper = "function _bruGet(obj, path) { \
        return path.split(\".\").reduce(function(o, k) { return o && o[k]; }, obj); \
    }\n";
    let mut stmts = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("//")
            || trimmed.starts_with('#')
            || trimmed.starts_with('~')
        {
            continue;
        }

        // Split `path: operator [value]`
        let colon_pos = match trimmed.find(':') {
            Some(p) => p,
            None => continue,
        };
        let path = trimmed[..colon_pos].trim();
        let rest = trimmed[colon_pos + 1..].trim();

        // Split operator from value
        let (operator, value_str) = {
            let mut parts = rest.splitn(2, ' ');
            let op = parts.next().unwrap_or("").trim();
            let val = parts.next().unwrap_or("").trim();
            (op, val)
        };

        // Produce the JS expression to retrieve the value
        let getter = format!("_bruGet(res, \"{}\")", path);

        let stmt = match operator {
            "eq" => {
                let js_val = to_js_literal(value_str);
                format!(
                    "test(\"assert: {} eq {}\", function() {{ expect({}).to.equal({}); }});",
                    path, value_str, getter, js_val
                )
            },
            "neq" => {
                let js_val = to_js_literal(value_str);
                format!(
                    "test(\"assert: {} neq {}\", function() {{ expect({}).to.not.equal({}); }});",
                    path, value_str, getter, js_val
                )
            },
            "gt" => {
                let js_val = to_js_literal(value_str);
                format!(
                    "test(\"assert: {} gt {}\", function() {{ expect({}).to.be.above({}); }});",
                    path, value_str, getter, js_val
                )
            },
            "gte" => {
                let js_val = to_js_literal(value_str);
                format!(
                    "test(\"assert: {} gte {}\", function() {{ expect({}).to.be.at.least({}); }});",
                    path, value_str, getter, js_val
                )
            },
            "lt" => {
                let js_val = to_js_literal(value_str);
                format!(
                    "test(\"assert: {} lt {}\", function() {{ expect({}).to.be.below({}); }});",
                    path, value_str, getter, js_val
                )
            },
            "lte" => {
                let js_val = to_js_literal(value_str);
                format!(
                    "test(\"assert: {} lte {}\", function() {{ expect({}).to.be.at.most({}); }});",
                    path, value_str, getter, js_val
                )
            },
            "contains" => {
                format!(
                    "test(\"assert: {} contains {}\", function() {{ expect({}).to.include({}); }});",
                    path,
                    value_str,
                    getter,
                    to_js_literal(value_str)
                )
            },
            "notContains" => {
                format!(
                    "test(\"assert: {} notContains {}\", function() {{ \
                        expect({}).to.not.include({}); }});",
                    path,
                    value_str,
                    getter,
                    to_js_literal(value_str)
                )
            },
            "isDefined" => {
                format!(
                    "test(\"assert: {} isDefined\", function() {{ \
                        expect({}).to.not.equal(null); \
                        expect({}).to.not.equal(undefined); }});",
                    path, getter, getter
                )
            },
            "isNull" => {
                format!(
                    "test(\"assert: {} isNull\", function() {{ expect({}).to.be.null; }});",
                    path, getter
                )
            },
            "isTruthy" => {
                format!(
                    "test(\"assert: {} isTruthy\", function() {{ expect(!!{}).to.be.true; }});",
                    path, getter
                )
            },
            "isFalsy" => {
                format!(
                    "test(\"assert: {} isFalsy\", function() {{ expect(!!{}).to.be.false; }});",
                    path, getter
                )
            },
            "startsWith" => {
                format!(
                    "test(\"assert: {} startsWith {}\", function() {{ \
                        expect({}).to.startsWith({}); }});",
                    path,
                    value_str,
                    getter,
                    to_js_literal(value_str)
                )
            },
            "endsWith" => {
                format!(
                    "test(\"assert: {} endsWith {}\", function() {{ \
                        expect({}).to.endsWith({}); }});",
                    path,
                    value_str,
                    getter,
                    to_js_literal(value_str)
                )
            },
            _ => {
                // Unknown operator — emit a comment
                format!("// unsupported assert: {}", trimmed)
            },
        };

        stmts.push(stmt);
    }

    if stmts.is_empty() {
        return String::new();
    }

    format!("{}{}", helper, stmts.join("\n"))
}

/// Convert a string value to a JS literal (number or quoted string).
fn to_js_literal(s: &str) -> String {
    // Try to parse as a number
    if s.parse::<i64>().is_ok() || s.parse::<f64>().is_ok() {
        return s.to_string();
    }
    // Otherwise wrap in double quotes (escape internal quotes)
    format!("\"{}\"", s.replace('"', "\\\""))
}

// ─── Import ───────────────────────────────────────────────────────────────────

fn parse_bru(content: &str) -> Result<TestCase, BrunoError> {
    let sections = parse_bru_sections(content);

    let mut name = String::new();
    let mut url = String::new();
    let mut method = "GET".to_string();
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut body: Option<String> = None;
    let mut pre_script: Option<String> = None;
    let mut post_script: Option<String> = None;
    for (section_name, section_content) in &sections {
        let sn = section_name.as_str();

        match sn {
            "meta" => {
                for (k, v) in parse_bru_kv(section_content) {
                    if k == "name" {
                        name = v;
                    }
                }
            },

            "get" | "post" | "put" | "delete" | "patch" | "head" | "options" => {
                method = sn.to_uppercase();
                for (k, v) in parse_bru_kv(section_content) {
                    if k == "url" {
                        url = v;
                    }
                }
            },

            "headers" => {
                for (k, v) in parse_bru_kv(section_content) {
                    headers.push((k, v));
                }
            },

            "params:query" => {
                // Append query params to the URL
                let params: Vec<String> = parse_bru_kv(section_content)
                    .into_iter()
                    .map(|(k, v)| format!("{}={}", percent_encode(&k), percent_encode(&v)))
                    .collect();
                if !params.is_empty() {
                    let sep = if url.contains('?') { "&" } else { "?" };
                    url.push_str(sep);
                    url.push_str(&params.join("&"));
                }
            },

            "body:json" | "body:text" | "body:xml" => {
                let trimmed = section_content.trim();
                if !trimmed.is_empty() {
                    body = Some(trimmed.to_string());
                }
                // Add Content-Type header based on section type
                let ct = match sn {
                    "body:json" => Some("application/json"),
                    "body:xml" => Some("application/xml"),
                    "body:text" => Some("text/plain"),
                    _ => None,
                };
                if let Some(ct_val) = ct
                    && !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type"))
                {
                    headers.push(("Content-Type".to_string(), ct_val.to_string()));
                }
            },

            "body:form-urlencoded" => {
                let pairs = parse_bru_kv(section_content);
                if !pairs.is_empty() {
                    let encoded: String = pairs
                        .iter()
                        .map(|(k, v)| format!("{}={}", percent_encode(k), percent_encode(v)))
                        .collect::<Vec<_>>()
                        .join("&");
                    body = Some(encoded);
                    if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type")) {
                        headers.push((
                            "Content-Type".to_string(),
                            "application/x-www-form-urlencoded".to_string(),
                        ));
                    }
                }
            },

            // B4: body:file — single file upload; store as `< path` reference
            "body:file" => {
                let path = section_content.trim();
                if !path.is_empty() {
                    body = Some(format!("< {}", path));
                }
            },

            // B3: body:form-data — synonym for body:multipart-form
            "body:form-data" | "body:multipart-form" => {
                // Parse each field; values may be plain text or @file(path) references.
                // We cannot read file content at import time, so @file() values are stored
                // as a `<file:path>` placeholder. Real multipart encoding requires a
                // runtime boundary, so we represent fields as key=value pairs.
                let pairs = parse_bru_kv(section_content);
                if !pairs.is_empty() {
                    let fields: String = pairs
                        .iter()
                        .map(|(k, v)| {
                            let resolved = if let Some(path) = extract_file_ref(v) {
                                format!("<file:{}>", path)
                            } else {
                                v.clone()
                            };
                            format!("{}={}", k, resolved)
                        })
                        .collect::<Vec<_>>()
                        .join("&");
                    body = Some(fields);
                    if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type")) {
                        headers
                            .push(("Content-Type".to_string(), "multipart/form-data".to_string()));
                    }
                }
            },

            "script:pre-request" => {
                let trimmed = section_content.trim();
                if !trimmed.is_empty() {
                    pre_script = Some(format!("{}{}", BRU_PRE_PREAMBLE, trimmed));
                }
            },

            "script:post-response" | "script:tests" | "test" => {
                let trimmed = section_content.trim();
                if !trimmed.is_empty() {
                    let new_post = format!("{}{}{}", BRU_POST_PREAMBLE, trimmed, BRU_POST_SUFFIX);
                    post_script = Some(match post_script {
                        Some(existing) => format!("{}\n{}", existing, new_post),
                        None => new_post,
                    });
                }
            },

            "assert" => {
                let js = assert_to_js(section_content);
                if !js.is_empty() {
                    let wrapped = format!("{}{}{}", BRU_POST_PREAMBLE, js, BRU_POST_SUFFIX);
                    post_script = Some(match post_script {
                        Some(existing) => format!("{}\n{}", existing, wrapped),
                        None => wrapped,
                    });
                }
            },

            "auth:bearer" => {
                for (k, v) in parse_bru_kv(section_content) {
                    if k == "token" {
                        headers.push(("Authorization".to_string(), format!("Bearer {}", v)));
                    }
                }
            },

            "auth:basic" => {
                let mut user = String::new();
                let mut pass = String::new();
                for (k, v) in parse_bru_kv(section_content) {
                    match k.as_str() {
                        "username" => user = v,
                        "password" => pass = v,
                        _ => {},
                    }
                }
                if !user.is_empty() || !pass.is_empty() {
                    let encoded = STANDARD.encode(format!("{}:{}", user, pass));
                    headers.push(("Authorization".to_string(), format!("Basic {}", encoded)));
                }
            },

            "auth:apikey" => {
                let mut key_name = String::new();
                let mut key_value = String::new();
                let mut placement = "header".to_string();
                for (k, v) in parse_bru_kv(section_content) {
                    match k.as_str() {
                        "key" => key_name = v,
                        "value" => key_value = v,
                        "in" => placement = v,
                        _ => {},
                    }
                }
                if placement == "header" && !key_name.is_empty() {
                    headers.push((key_name, key_value));
                }
            },

            // B1: vars:pre-request — generate pm.environment / bru.setVar calls
            "vars:pre-request" => {
                let pairs = parse_bru_kv(section_content);
                let mut lines: Vec<String> = Vec::new();
                for (k, v) in &pairs {
                    if let Some(env_key) = k.strip_prefix('@') {
                        lines.push(format!(
                            "bru.setEnvVar({}, {});",
                            serde_json::to_string(env_key)
                                .unwrap_or_else(|_| format!("{:?}", env_key)),
                            serde_json::to_string(v).unwrap_or_else(|_| format!("{:?}", v))
                        ));
                    } else {
                        lines.push(format!(
                            "bru.setVar({}, {});",
                            serde_json::to_string(k.as_str())
                                .unwrap_or_else(|_| format!("{:?}", k)),
                            serde_json::to_string(v.as_str())
                                .unwrap_or_else(|_| format!("{:?}", v))
                        ));
                    }
                }
                if !lines.is_empty() {
                    let vars_body = lines.join("\n");
                    pre_script = Some(match pre_script {
                        Some(existing) => format!("{}\n{}", existing, vars_body),
                        None => format!("{}{}", BRU_PRE_PREAMBLE, vars_body),
                    });
                }
            },

            // B2: auth:oauth2 — generate a client_credentials token-exchange pre-script
            "auth:oauth2" => {
                let fields: std::collections::HashMap<String, String> =
                    parse_bru_kv(section_content).into_iter().collect();
                let grant_type = fields.get("grantType").map(|s| s.as_str()).unwrap_or("");
                let oauth2_opt = if grant_type == "client_credentials" {
                    build_oauth2_pre_script(&fields)
                } else {
                    None
                };
                if let Some(oauth2_script) = oauth2_opt {
                    pre_script = Some(match pre_script {
                        Some(existing) => {
                            format!("{}\n{}", existing, &oauth2_script[BRU_PRE_PREAMBLE.len()..])
                        },
                        None => oauth2_script,
                    });
                }
            },

            "docs" => {
                // Documentation — no equivalent in TestCase; ignored.
            },

            _ => {
                // Unknown / unsupported section — ignore.
            },
        }
    }

    if name.is_empty() {
        // Use the URL as fallback name
        name = url.clone();
    }

    Ok(TestCase {
        name,
        request: HttpRequest {
            url,
            method,
            headers,
            body,
        },
        pre_script,
        post_script,
        ..Default::default()
    })
}

fn parse_bru_with_seq(content: &str) -> Result<(TestCase, Option<i64>), BrunoError> {
    let sections = parse_bru_sections(content);
    let mut seq: Option<i64> = None;

    for (section_name, section_content) in &sections {
        if section_name == "meta" {
            for (k, v) in parse_bru_kv(section_content) {
                if k == "seq" {
                    seq = v.parse::<i64>().ok();
                }
            }
        }
    }

    let tc = parse_bru(content)?;
    Ok((tc, seq))
}

// ─── Body helpers ─────────────────────────────────────────────────────────────

/// Extract the file path from a Bruno `@file(path)` annotation, if present.
fn extract_file_ref(value: &str) -> Option<&str> {
    let start = value.find("@file(")?;
    let inner = &value[start + 6..];
    let end = inner.find(')')?;
    Some(inner[..end].trim())
}

/// Build an OAuth2 `client_credentials` token-exchange pre-script (B2).
fn build_oauth2_pre_script(fields: &std::collections::HashMap<String, String>) -> Option<String> {
    let token_url = fields.get("accessTokenUrl")?;
    let client_id = fields.get("clientId").map(String::as_str).unwrap_or("");
    let client_secret = fields.get("clientSecret").map(String::as_str).unwrap_or("");
    let scope = fields.get("scope").map(String::as_str).unwrap_or("");

    let body_str = format!(
        "grant_type=client_credentials&client_id={}&client_secret={}&scope={}",
        client_id, client_secret, scope
    );
    let token_url_js =
        serde_json::to_string(token_url).unwrap_or_else(|_| format!("{:?}", token_url));
    let body_js = serde_json::to_string(&body_str).unwrap_or_else(|_| format!("{:?}", body_str));

    Some(format!(
        "{preamble}\
const _tokenResp = await fetch({token_url}, {{\n  \
  method: \"POST\",\n  \
  headers: [[\"Content-Type\", \"application/x-www-form-urlencoded\"]],\n  \
  body: {body},\n\
}});\n\
const _tokenBody = JSON.parse(await _tokenResp.text());\n\
const _req = sandbox.readInput(\"_request\");\n\
_req.headers = (_req.headers || []).concat([[\"Authorization\", \"Bearer \" + _tokenBody.access_token]]);\n\
return _req;",
        preamble = BRU_PRE_PREAMBLE,
        token_url = token_url_js,
        body = body_js,
    ))
}

/// Parse a Bruno environment `.bru` file — returns `(key, value)` pairs from
/// the `vars` / `variables` section.
fn parse_bru_env(content: &str) -> Vec<(String, String)> {
    let sections = parse_bru_sections(content);
    let mut vars = Vec::new();
    for (name, body) in sections {
        if name == "vars" || name == "variables" {
            for (k, v) in parse_bru_kv(&body) {
                vars.push((k, v));
            }
        }
    }
    vars
}

/// Build a pre-script that seeds `bru.setEnvVar` calls from environment vars (B6).
fn build_env_pre_script(vars: &[(String, String)]) -> String {
    let mut lines = vec![BRU_PRE_PREAMBLE.to_string()];
    for (k, v) in vars {
        let k_js = serde_json::to_string(k.as_str()).unwrap_or_else(|_| format!("{:?}", k));
        let v_js = serde_json::to_string(v.as_str()).unwrap_or_else(|_| format!("{:?}", v));
        lines.push(format!("bru.setEnvVar({}, {});", k_js, v_js));
    }
    lines.join("\n")
}

// ─── Percent encoding (simple) ────────────────────────────────────────────────

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            },
            _ => {
                out.push_str(&format!("%{:02X}", b));
            },
        }
    }
    out
}

// ─── Export ───────────────────────────────────────────────────────────────────

fn export_bru(test: &TestCase, seq: usize) -> String {
    let mut out = String::new();

    // meta block
    out.push_str("meta {\n");
    out.push_str("  name: ");
    out.push_str(&test.name);
    out.push('\n');
    out.push_str("  type: http\n");
    out.push_str(&format!("  seq: {}\n", seq));
    out.push_str("}\n\n");

    // method block
    let method_lower = test.request.method.to_lowercase();
    out.push_str(&format!("{} {{\n", method_lower));
    out.push_str(&format!("  url: {}\n", test.request.url));
    out.push_str("  body: none\n");
    out.push_str("}\n\n");

    // headers
    if !test.request.headers.is_empty() {
        out.push_str("headers {\n");
        for (k, v) in &test.request.headers {
            // Skip auth headers — they will be emitted in auth blocks below
            if k.eq_ignore_ascii_case("authorization") {
                continue;
            }
            out.push_str(&format!("  {}: {}\n", k, v));
        }
        out.push_str("}\n\n");
    }

    // body
    if let Some(b) = &test.request.body {
        let trimmed = b.trim();
        if trimmed.starts_with('{') || trimmed.starts_with('[') {
            out.push_str("body:json {\n");
            for line in trimmed.lines() {
                out.push_str("  ");
                out.push_str(line);
                out.push('\n');
            }
            out.push_str("}\n\n");
        } else {
            out.push_str("body:text {\n");
            for line in trimmed.lines() {
                out.push_str("  ");
                out.push_str(line);
                out.push('\n');
            }
            out.push_str("}\n\n");
        }
    }

    // auth headers: try to detect Bearer / Basic
    for (k, v) in &test.request.headers {
        if k.eq_ignore_ascii_case("authorization") {
            if let Some(token) = v.strip_prefix("Bearer ") {
                out.push_str("auth:bearer {\n");
                out.push_str(&format!("  token: {}\n", token));
                out.push_str("}\n\n");
            } else if let Some(encoded) = v.strip_prefix("Basic ") {
                out.push_str("auth:basic {\n");
                // Decode to retrieve username/password if possible
                if let Ok(decoded_bytes) = STANDARD.decode(encoded) {
                    if let Ok(decoded) = std::str::from_utf8(&decoded_bytes) {
                        if let Some(colon_pos) = decoded.find(':') {
                            let user = &decoded[..colon_pos];
                            let pass = &decoded[colon_pos + 1..];
                            out.push_str(&format!("  username: {}\n", user));
                            out.push_str(&format!("  password: {}\n", pass));
                        } else {
                            out.push_str(&format!("  username: {}\n", decoded));
                        }
                    } else {
                        out.push_str(&format!("  token: {}\n", encoded));
                    }
                } else {
                    out.push_str(&format!("  token: {}\n", encoded));
                }
                out.push_str("}\n\n");
            }
        }
    }

    // pre-request script
    if let Some(pre) = &test.pre_script {
        out.push_str("script:pre-request {\n");
        for line in pre.lines() {
            out.push_str(&format!("  {}\n", line));
        }
        out.push_str("}\n\n");
    }

    // post-response / test script
    if let Some(post) = &test.post_script {
        out.push_str("test {\n");
        for line in post.lines() {
            out.push_str(&format!("  {}\n", line));
        }
        out.push_str("}\n\n");
    }

    // params:query — extract from URL if query string present
    if let Some(query_start) = test.request.url.find('?') {
        let query_str = &test.request.url[query_start + 1..];
        if !query_str.is_empty() {
            out.push_str("params:query {\n");
            for pair in query_str.split('&') {
                if let Some(eq_pos) = pair.find('=') {
                    let k = &pair[..eq_pos];
                    let v = &pair[eq_pos + 1..];
                    out.push_str(&format!("  {}: {}\n", k, v));
                } else {
                    out.push_str(&format!("  {}: \n", pair));
                }
            }
            out.push_str("}\n\n");
        }
    }

    out
}

// ─── Adapter ──────────────────────────────────────────────────────────────────

pub struct BrunoAdapter;

impl BrunoAdapter {
    /// Parse a single `.bru` file string into a [`TestCase`].
    pub fn import(content: &str) -> Result<TestCase, BrunoError> {
        parse_bru(content)
    }

    /// Export a [`TestCase`] to a `.bru` file string.
    pub fn export(test: &TestCase) -> String {
        export_bru(test, 1)
    }

    /// Import all `.bru` files from a directory, sorted by their `seq` field.
    ///
    /// Any `.bru` file with `meta { type: collection }` or `meta { type: folder }`
    /// is treated as a folder-level file: its `script:pre-request` and
    /// `script:post-response` sections are prepended / appended to every imported
    /// request's scripts (B5).
    pub fn import_dir(dir: &Path) -> Result<Vec<TestCase>, BrunoError> {
        let mut folder_pre: Option<String> = None;
        let mut folder_post: Option<String> = None;
        let mut entries: Vec<(i64, TestCase)> = Vec::new();

        let read_dir = std::fs::read_dir(dir)?;
        let mut paths: Vec<std::path::PathBuf> = read_dir
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("bru"))
            .collect();
        paths.sort();

        for path in &paths {
            let content = std::fs::read_to_string(path)?;
            let sections = parse_bru_sections(&content);

            // Detect folder/collection meta type
            let meta_type = sections.iter().find(|(n, _)| n == "meta").and_then(|(_, body)| {
                parse_bru_kv(body).into_iter().find(|(k, _)| k == "type").map(|(_, v)| v)
            });

            let is_folder =
                meta_type.as_deref().map(|t| t == "folder" || t == "collection").unwrap_or(false);

            if is_folder {
                // Extract folder-level scripts from this file
                for (section_name, section_content) in &sections {
                    let trimmed = section_content.trim();
                    match section_name.as_str() {
                        "script:pre-request" if !trimmed.is_empty() => {
                            folder_pre = Some(format!("{}{}", BRU_PRE_PREAMBLE, trimmed));
                        },
                        "script:post-response" | "script:tests" | "test" if !trimmed.is_empty() => {
                            folder_post = Some(format!(
                                "{}{}{}",
                                BRU_POST_PREAMBLE, trimmed, BRU_POST_SUFFIX
                            ));
                        },
                        _ => {},
                    }
                }
            } else {
                let (tc, seq) = parse_bru_with_seq(&content)
                    .map_err(|e| BrunoError::Parse(format!("{}: {}", path.display(), e)))?;
                entries.push((seq.unwrap_or(i64::MAX), tc));
            }
        }

        // Sort by seq (ascending); unsequenced files go to the end.
        entries.sort_by_key(|(seq, _)| *seq);
        let mut tests: Vec<TestCase> = entries.into_iter().map(|(_, tc)| tc).collect();

        // B5: prepend folder pre-script and append folder post-script to each request
        if folder_pre.is_some() || folder_post.is_some() {
            for tc in &mut tests {
                if let Some(ref fp) = folder_pre {
                    tc.pre_script = Some(match tc.pre_script.take() {
                        Some(existing) => {
                            format!("{}\n{}", fp, &existing[BRU_PRE_PREAMBLE.len()..])
                        },
                        None => fp.clone(),
                    });
                }
                if let Some(ref fp) = folder_post {
                    tc.post_script = Some(match tc.post_script.take() {
                        Some(existing) => format!("{}\n{}", existing, fp),
                        None => fp.clone(),
                    });
                }
            }
        }

        Ok(tests)
    }

    /// Import all `.bru` files from a directory together with a named environment
    /// file from `<dir>/environments/<env_name>.bru` (B6).
    ///
    /// Environment variables are injected as `bru.setEnvVar()` calls prepended to
    /// every test case's pre-script so they are available at run time.
    pub fn import_dir_with_env(dir: &Path, env_name: &str) -> Result<Vec<TestCase>, BrunoError> {
        let mut tests = Self::import_dir(dir)?;

        let env_path = dir.join("environments").join(format!("{}.bru", env_name));
        if env_path.exists() {
            let content = std::fs::read_to_string(&env_path)?;
            let vars = parse_bru_env(&content);
            if !vars.is_empty() {
                let env_script = build_env_pre_script(&vars);
                for tc in &mut tests {
                    tc.pre_script = Some(match tc.pre_script.take() {
                        Some(existing) => {
                            format!("{}\n{}", env_script, &existing[BRU_PRE_PREAMBLE.len()..])
                        },
                        None => env_script.clone(),
                    });
                }
            }
        }

        Ok(tests)
    }
}
