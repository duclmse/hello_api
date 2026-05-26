//! Import/export adapter for the OpenCollection v1.0.0 format.
//!
//! The canonical format is JSON. YAML is also accepted on import (same schema).
//! Format is auto-detected by the leading character (`{` → JSON, else YAML).

use std::collections::HashMap;

use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::types::{HttpRequest, TestCase};

const PRE_PREAMBLE: &str = "import { pm, bru, req } from \"sandbox:pm\";\n";
const POST_PREAMBLE: &str = "import { pm, res, bru, test, expect, results } from \"sandbox:pm\";\n";
const POST_SUFFIX: &str = "\nreturn results();";

// --- Error -------------------------------------------------------------------

/// Errors from the OpenCollection adapter.
#[derive(Debug, thiserror::Error)]
pub enum OpenCollectionError {
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("YAML parse error: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("{0}")]
    Other(String),
}

// --- Import serde types (OpenCollection v1.0.0) ------------------------------

#[derive(Debug, Deserialize)]
struct OcRoot {
    #[serde(default)]
    info: Option<OcInfo>,
    #[serde(default)]
    config: Option<OcConfig>,
    #[serde(default)]
    items: Vec<OcItem>,
}

#[derive(Debug, Deserialize)]
struct OcInfo {
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OcConfig {
    environments: Option<Vec<OcEnvironment>>,
}

#[derive(Debug, Deserialize)]
struct OcEnvironment {
    variables: Option<Vec<OcVariable>>,
    selected: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct OcVariable {
    name: String,
    value: Option<serde_json::Value>,
    variants: Option<Vec<OcVariant>>,
}

#[derive(Debug, Deserialize)]
struct OcVariant {
    value: Option<serde_json::Value>,
    selected: Option<bool>,
}

// An item is a folder (has `items`) or an HTTP request (has `http`).
// Other types (graphql, grpc, websocket) are skipped.
#[derive(Debug, Deserialize)]
struct OcItem {
    info: Option<OcItemInfo>,
    http: Option<OcHttpDef>,
    items: Option<Vec<OcItem>>,
    auth: Option<OcAuth>,
    runtime: Option<OcRuntime>,
}

#[derive(Debug, Deserialize)]
struct OcItemInfo {
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OcHttpDef {
    method: Option<String>,
    url: Option<String>,
    headers: Option<Vec<OcHeader>>,
    body: Option<OcBody>,
    auth: Option<OcAuthValue>,
}

#[derive(Debug, Deserialize)]
struct OcHeader {
    name: String,
    value: Option<String>,
    disabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct OcBody {
    #[serde(rename = "type")]
    ty: Option<String>,
    body: Option<String>,
    #[serde(rename = "contentType")]
    content_type: Option<String>,
    /// Bundled format field name for form fields.
    form: Option<Vec<OcFormField>>,
    /// Per-file YAML format field name for form fields (alias for `form`).
    data: Option<Vec<OcFormField>>,
    #[serde(rename = "multipartForm")]
    multipart_form: Option<Vec<OcFormField>>,
}

#[derive(Debug, Deserialize)]
struct OcFormField {
    name: String,
    value: Option<String>,
    disabled: Option<bool>,
}

/// Auth can be an object `{type: "bearer", ...}` or a plain string like `"inherit"` / `"none"`.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum OcAuthValue {
    Auth(OcAuth),
    Literal(#[allow(dead_code)] String),
}

impl OcAuthValue {
    fn as_auth(&self) -> Option<&OcAuth> {
        match self {
            OcAuthValue::Auth(a) => Some(a),
            OcAuthValue::Literal(_) => None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct OcAuth {
    #[serde(rename = "type")]
    ty: Option<String>,
    bearer: Option<OcBearerAuth>,
    basic: Option<OcBasicAuth>,
    #[serde(rename = "apiKey")]
    api_key: Option<OcApiKeyAuth>,
    token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OcBearerAuth {
    token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OcBasicAuth {
    username: Option<String>,
    password: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OcApiKeyAuth {
    key: Option<String>,
    value: Option<String>,
    placement: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OcRuntime {
    scripts: Option<Vec<OcScript>>,
    assertions: Option<Vec<OcAssertion>>,
    /// Per-request variables (like `vars:pre-request` in `.bru`). Not mapped to TestCase.
    #[serde(default)]
    #[allow(dead_code)]
    variables: Option<Vec<OcVariable>>,
    /// Runtime actions (set-variable etc.). Ignored on import.
    #[serde(default)]
    #[allow(dead_code)]
    actions: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Deserialize)]
struct OcScript {
    #[serde(rename = "type")]
    ty: String,
    code: Option<String>,
    disabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct OcAssertion {
    expression: Option<String>,
    operator: Option<String>,
    value: Option<serde_json::Value>,
    disabled: Option<bool>,
}

// --- Per-file YAML types (Bruno OpenCollection directory format) --------------

#[derive(Debug, Deserialize)]
struct OcFileItemInfo {
    name: Option<String>,
    seq: Option<i64>,
    #[serde(rename = "type")]
    _ty: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OcFileItem {
    info: Option<OcFileItemInfo>,
    http: Option<OcHttpDef>,
    runtime: Option<OcRuntime>,
}

#[derive(Debug, Deserialize)]
struct OcManifest {
    info: Option<OcManifestInfo>,
}

#[derive(Debug, Deserialize)]
struct OcManifestInfo {
    name: Option<String>,
}

// --- Export serde types -------------------------------------------------------

#[derive(Debug, Serialize)]
struct OcExportRoot {
    opencollection: String,
    info: OcExportInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    config: Option<OcExportConfig>,
    items: Vec<OcExportItem>,
}

#[derive(Debug, Serialize)]
struct OcExportInfo {
    name: String,
    version: String,
}

#[derive(Debug, Serialize)]
struct OcExportConfig {
    environments: Vec<OcExportEnvironment>,
}

#[derive(Debug, Serialize)]
struct OcExportEnvironment {
    name: String,
    variables: Vec<OcExportVariable>,
    selected: bool,
}

#[derive(Debug, Serialize)]
struct OcExportVariable {
    name: String,
    value: String,
}

#[derive(Debug, Serialize)]
struct OcExportItem {
    info: OcExportItemInfo,
    http: OcExportHttpDef,
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime: Option<OcExportRuntime>,
}

#[derive(Debug, Serialize)]
struct OcExportItemInfo {
    name: String,
    sequence: i64,
}

#[derive(Debug, Serialize)]
struct OcExportHttpDef {
    method: String,
    url: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    headers: Vec<OcExportHeader>,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<OcExportBody>,
}

#[derive(Debug, Serialize)]
struct OcExportHeader {
    name: String,
    value: String,
}

#[derive(Debug, Serialize)]
struct OcExportBody {
    #[serde(rename = "type")]
    ty: String,
    body: String,
    #[serde(rename = "contentType")]
    content_type: String,
}

#[derive(Debug, Serialize)]
struct OcExportRuntime {
    scripts: Vec<OcExportScript>,
}

#[derive(Debug, Serialize)]
struct OcExportScript {
    #[serde(rename = "type")]
    ty: String,
    code: String,
}

// --- Public types ------------------------------------------------------------

/// A parsed OpenCollection document.
pub struct OpenCollection {
    pub name: String,
    pub env: HashMap<String, String>,
    pub tests: Vec<TestCase>,
}

// --- Adapter -----------------------------------------------------------------

/// Import/export adapter for the OpenCollection v1.0.0 format.
pub struct OpenCollectionAdapter;

impl OpenCollectionAdapter {
    /// Parse an OpenCollection v1.0.0 document (JSON or YAML) into test cases.
    pub fn import(input: &str) -> Result<OpenCollection, OpenCollectionError> {
        let trimmed = input.trim();
        let root: OcRoot = if trimmed.starts_with('{') {
            serde_json::from_str(trimmed)?
        } else {
            serde_yaml::from_str(trimmed)?
        };

        let name = root
            .info
            .as_ref()
            .and_then(|i| i.name.clone())
            .unwrap_or_else(|| "Unnamed Collection".to_string());

        let env = extract_env(&root.config);

        let mut tests = Vec::new();
        for item in &root.items {
            flatten_item(item, None, &mut tests);
        }

        Ok(OpenCollection { name, env, tests })
    }

    /// Serialize test cases to an OpenCollection v1.0.0 JSON document.
    pub fn export(name: &str, tests: &[TestCase], env: &HashMap<String, String>) -> String {
        let config = if env.is_empty() {
            None
        } else {
            let mut vars: Vec<OcExportVariable> = env
                .iter()
                .map(|(k, v)| OcExportVariable {
                    name: k.clone(),
                    value: v.clone(),
                })
                .collect();
            vars.sort_by(|a, b| a.name.cmp(&b.name));
            Some(OcExportConfig {
                environments: vec![OcExportEnvironment {
                    name: "default".to_string(),
                    variables: vars,
                    selected: true,
                }],
            })
        };

        let items: Vec<OcExportItem> = tests
            .iter()
            .enumerate()
            .map(|(i, tc)| {
                let pre = tc.pre_script.as_deref().map(strip_pre_preamble);
                let post = tc.post_script.as_deref().map(strip_post_preamble);

                let mut scripts = Vec::new();
                if let Some(code) = pre {
                    scripts.push(OcExportScript {
                        ty: "before-request".to_string(),
                        code,
                    });
                }
                if let Some(code) = post {
                    scripts.push(OcExportScript {
                        ty: "tests".to_string(),
                        code,
                    });
                }

                let runtime = if scripts.is_empty() {
                    None
                } else {
                    Some(OcExportRuntime { scripts })
                };

                let headers: Vec<OcExportHeader> = tc
                    .request
                    .headers
                    .iter()
                    .map(|(k, v)| OcExportHeader {
                        name: k.clone(),
                        value: v.clone(),
                    })
                    .collect();

                let body = tc.request.body.as_ref().map(|b| {
                    let ct = tc
                        .request
                        .headers
                        .iter()
                        .find(|(k, _)| k.to_lowercase() == "content-type")
                        .map(|(_, v)| v.clone())
                        .unwrap_or_else(|| "text/plain".to_string());
                    OcExportBody {
                        ty: "raw".to_string(),
                        body: b.clone(),
                        content_type: ct,
                    }
                });

                OcExportItem {
                    info: OcExportItemInfo {
                        name: tc.name.clone(),
                        sequence: (i + 1) as i64,
                    },
                    http: OcExportHttpDef {
                        method: tc.request.method.clone(),
                        url: tc.request.url.clone(),
                        headers,
                        body,
                    },
                    runtime,
                }
            })
            .collect();

        let root = OcExportRoot {
            opencollection: "1.0.0".to_string(),
            info: OcExportInfo {
                name: name.to_string(),
                version: "1.0.0".to_string(),
            },
            config,
            items,
        };

        serde_json::to_string_pretty(&root).unwrap_or_default()
    }

    /// Import a Bruno OpenCollection directory (per-file YAML format).
    ///
    /// The directory must contain an `opencollection.yml` manifest and one
    /// `.yml` file per request. Files are sorted by the `info.seq` field.
    pub fn import_dir(dir: &std::path::Path) -> Result<OpenCollection, OpenCollectionError> {
        let manifest_path = dir.join("opencollection.yml");
        let name = if manifest_path.exists() {
            let content = std::fs::read_to_string(&manifest_path)
                .map_err(|e| OpenCollectionError::Other(e.to_string()))?;
            let manifest: OcManifest = serde_yaml::from_str(&content)?;
            manifest.info.and_then(|i| i.name).unwrap_or_else(|| "Unnamed Collection".to_string())
        } else {
            "Unnamed Collection".to_string()
        };

        let mut entries: Vec<(i64, TestCase)> = Vec::new();

        let read_dir =
            std::fs::read_dir(dir).map_err(|e| OpenCollectionError::Other(e.to_string()))?;

        for entry in read_dir {
            let entry = entry.map_err(|e| OpenCollectionError::Other(e.to_string()))?;
            let path = entry.path();
            let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
            if path.extension().and_then(|e| e.to_str()) != Some("yml") {
                continue;
            }
            if file_name == "opencollection.yml" {
                continue;
            }
            let content = std::fs::read_to_string(&path)
                .map_err(|e| OpenCollectionError::Other(e.to_string()))?;
            let item: OcFileItem = serde_yaml::from_str(&content)?;
            let seq = item.info.as_ref().and_then(|i| i.seq).unwrap_or(i64::MAX);
            let tc = build_test_case_from_file_item(&item);
            entries.push((seq, tc));
        }

        entries.sort_by_key(|(seq, _)| *seq);
        Ok(OpenCollection {
            name,
            env: HashMap::new(),
            tests: entries.into_iter().map(|(_, tc)| tc).collect(),
        })
    }
}

// --- Import helpers ----------------------------------------------------------

fn extract_env(config: &Option<OcConfig>) -> HashMap<String, String> {
    let envs = match config.as_ref().and_then(|c| c.environments.as_ref()) {
        Some(e) => e,
        None => return HashMap::new(),
    };

    let env = envs.iter().find(|e| e.selected == Some(true)).or_else(|| envs.first());
    let vars = match env.and_then(|e| e.variables.as_ref()) {
        Some(v) => v,
        None => return HashMap::new(),
    };

    vars.iter()
        .filter_map(|v| {
            let value = resolve_variable_value(v)?;
            Some((v.name.clone(), value))
        })
        .collect()
}

fn resolve_variable_value(v: &OcVariable) -> Option<String> {
    if let Some(variants) = &v.variants
        && let Some(selected) = variants.iter().find(|vr| vr.selected == Some(true))
        && let Some(val) = &selected.value
    {
        return Some(json_val_to_string(val));
    }
    v.value.as_ref().map(json_val_to_string)
}

fn json_val_to_string(val: &serde_json::Value) -> String {
    match val {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Object(obj) => {
            // typed value: {"type": "...", "data": ...}
            if let Some(data) = obj.get("data") {
                return json_val_to_string(data);
            }
            val.to_string()
        },
        _ => val.to_string(),
    }
}

fn flatten_item(item: &OcItem, folder_prefix: Option<&str>, tests: &mut Vec<TestCase>) {
    if let Some(sub_items) = &item.items {
        let folder_name = item.info.as_ref().and_then(|i| i.name.as_deref()).unwrap_or("folder");
        let prefix = match folder_prefix {
            Some(p) => format!("{}/{}", p, folder_name),
            None => folder_name.to_string(),
        };
        for sub in sub_items {
            flatten_item(sub, Some(&prefix), tests);
        }
    } else if let Some(http) = &item.http {
        let name = item.info.as_ref().and_then(|i| i.name.as_deref()).unwrap_or("request");
        let full_name = match folder_prefix {
            Some(p) => format!("{}/{}", p, name),
            None => name.to_string(),
        };
        tests.push(build_test_case(full_name, http, item));
    }
    // graphql / grpc / websocket items have none of those fields — skipped silently
}

fn build_test_case(name: String, http: &OcHttpDef, item: &OcItem) -> TestCase {
    let method = http.method.clone().unwrap_or_else(|| "GET".to_string()).to_uppercase();
    let url = http.url.clone().unwrap_or_default();

    let mut headers: Vec<(String, String)> = Vec::new();

    if let Some(hdrs) = &http.headers {
        for h in hdrs {
            if h.disabled != Some(true)
                && let Some(val) = &h.value
            {
                headers.push((h.name.clone(), val.clone()));
            }
        }
    }

    // item-level auth overrides http-level auth
    let auth: Option<&OcAuth> =
        item.auth.as_ref().or_else(|| http.auth.as_ref().and_then(OcAuthValue::as_auth));
    if let Some(auth) = auth {
        apply_auth_headers(auth, &mut headers);
    }

    let body = build_body(http.body.as_ref(), &mut headers);
    let (pre_script, post_script) = build_scripts(item.runtime.as_ref());

    TestCase {
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
    }
}

fn build_test_case_from_file_item(item: &OcFileItem) -> TestCase {
    let name = item.info.as_ref().and_then(|i| i.name.as_deref()).unwrap_or("request").to_string();

    let Some(http) = &item.http else {
        return TestCase {
            name,
            ..Default::default()
        };
    };

    let method = http.method.clone().unwrap_or_else(|| "GET".to_string()).to_uppercase();
    let url = http.url.clone().unwrap_or_default();

    let mut headers: Vec<(String, String)> = Vec::new();

    if let Some(hdrs) = &http.headers {
        for h in hdrs {
            if h.disabled != Some(true)
                && let Some(val) = &h.value
            {
                headers.push((h.name.clone(), val.clone()));
            }
        }
    }

    if let Some(auth) = http.auth.as_ref().and_then(OcAuthValue::as_auth) {
        apply_auth_headers(auth, &mut headers);
    }

    let body = build_body(http.body.as_ref(), &mut headers);
    let (pre_script, post_script) = build_scripts(item.runtime.as_ref());

    TestCase {
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
    }
}

fn apply_auth_headers(auth: &OcAuth, headers: &mut Vec<(String, String)>) {
    match auth.ty.as_deref().unwrap_or("") {
        "bearer" => {
            // Bundled format: bearer: { token: "..." }  Per-file format: token: "..."
            let token =
                auth.bearer.as_ref().and_then(|b| b.token.as_deref()).or(auth.token.as_deref());
            if let Some(token) = token {
                headers.push(("Authorization".to_string(), format!("Bearer {}", token)));
            }
        },
        "basic" => {
            if let Some(basic) = &auth.basic {
                let user = basic.username.as_deref().unwrap_or("");
                let pass = basic.password.as_deref().unwrap_or("");
                let encoded =
                    base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", user, pass));
                headers.push(("Authorization".to_string(), format!("Basic {}", encoded)));
            }
        },
        "apiKey" => {
            if let Some(api_key) = &auth.api_key
                && api_key.placement.as_deref().unwrap_or("header") == "header"
                && let (Some(key), Some(val)) = (&api_key.key, &api_key.value)
            {
                headers.push((key.clone(), val.clone()));
            }
        },
        _ => {},
    }
}

fn build_body(body: Option<&OcBody>, headers: &mut Vec<(String, String)>) -> Option<String> {
    let body = body?;
    match body.ty.as_deref().unwrap_or("raw") {
        "raw" => {
            if let Some(ct) = &body.content_type
                && !ct.is_empty()
            {
                headers.push(("Content-Type".to_string(), ct.clone()));
            }
            body.body.clone()
        },
        // Both camelCase (bundled JSON) and kebab-case (per-file YAML) variants.
        "formUrlEncoded" | "form-urlencoded" => {
            let form = body.form.as_deref().or(body.data.as_deref()).unwrap_or(&[]);
            let encoded = form
                .iter()
                .filter(|f| f.disabled != Some(true))
                .filter_map(|f| {
                    f.value.as_ref().map(|v| {
                        format!("{}={}", urlencode(f.name.as_str()), urlencode(v.as_str()))
                    })
                })
                .collect::<Vec<_>>()
                .join("&");
            headers.push((
                "Content-Type".to_string(),
                "application/x-www-form-urlencoded".to_string(),
            ));
            if encoded.is_empty() {
                None
            } else {
                Some(encoded)
            }
        },
        "multipartForm" | "multipart-form" => {
            let form = body.multipart_form.as_deref().or(body.data.as_deref()).unwrap_or(&[]);
            let fields = form
                .iter()
                .filter(|f| f.disabled != Some(true))
                .filter_map(|f| f.value.as_ref().map(|v| format!("{}={}", f.name, v)))
                .collect::<Vec<_>>()
                .join("&");
            if fields.is_empty() {
                None
            } else {
                Some(fields)
            }
        },
        _ => None,
    }
}

fn build_scripts(runtime: Option<&OcRuntime>) -> (Option<String>, Option<String>) {
    let runtime = match runtime {
        Some(r) => r,
        None => return (None, None),
    };

    let mut pre_parts: Vec<String> = Vec::new();
    let mut post_parts: Vec<String> = Vec::new();

    if let Some(scripts) = &runtime.scripts {
        for s in scripts {
            if s.disabled == Some(true) {
                continue;
            }
            let code = match &s.code {
                Some(c) if !c.trim().is_empty() => c.clone(),
                _ => continue,
            };
            match s.ty.as_str() {
                "before-request" => pre_parts.push(code),
                "after-response" | "tests" => post_parts.push(code),
                _ => {},
            }
        }
    }

    if let Some(assertions) = &runtime.assertions {
        for a in assertions {
            if a.disabled == Some(true) {
                continue;
            }
            if let Some(js) = assertion_to_js(a) {
                post_parts.push(js);
            }
        }
    }

    let pre_script = if pre_parts.is_empty() {
        None
    } else {
        let body = pre_parts.join("\n");
        Some(format!("{}{}", PRE_PREAMBLE, body.trim_end()))
    };

    let post_script = if post_parts.is_empty() {
        None
    } else {
        let body = post_parts.join("\n");
        let body = body.trim_end();
        let has_return = body
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .is_some_and(|l| l.trim().starts_with("return"));
        if has_return {
            Some(format!("{}{}", POST_PREAMBLE, body))
        } else {
            Some(format!("{}{}{}", POST_PREAMBLE, body, POST_SUFFIX))
        }
    };

    (pre_script, post_script)
}

fn assertion_to_js(a: &OcAssertion) -> Option<String> {
    let expr_str = a.expression.as_deref()?.trim();
    let op = a.operator.as_deref()?.trim();
    if expr_str.is_empty() || op.is_empty() {
        return None;
    }

    let js_expr = expression_to_js(expr_str);
    let label = format!(
        "assert: {} {}{}",
        expr_str,
        op,
        a.value.as_ref().map_or(String::new(), |v| format!(" {}", json_val_display(v)))
    );

    let check = match op {
        "equals" | "eq" => {
            format!("pm.expect({}).to.equal({})", js_expr, js_literal(a.value.as_ref()?))
        },
        "notEquals" | "neq" => {
            format!("pm.expect({}).to.not.equal({})", js_expr, js_literal(a.value.as_ref()?))
        },
        "contains" => {
            format!("pm.expect({}).to.include({})", js_expr, js_literal(a.value.as_ref()?))
        },
        "notContains" => {
            format!("pm.expect({}).to.not.include({})", js_expr, js_literal(a.value.as_ref()?))
        },
        "greaterThan" | "gt" => {
            format!("pm.expect({}).to.be.above({})", js_expr, js_literal(a.value.as_ref()?))
        },
        "greaterThanOrEqual" | "gte" => {
            format!("pm.expect({}).to.be.at.least({})", js_expr, js_literal(a.value.as_ref()?))
        },
        "lessThan" | "lt" => {
            format!("pm.expect({}).to.be.below({})", js_expr, js_literal(a.value.as_ref()?))
        },
        "lessThanOrEqual" | "lte" => {
            format!("pm.expect({}).to.be.at.most({})", js_expr, js_literal(a.value.as_ref()?))
        },
        "isDefined" | "exists" => format!("pm.expect({}).to.exist", js_expr),
        "isNull" | "null" => format!("pm.expect({}).to.be.null", js_expr),
        "isEmpty" | "empty" => format!("pm.expect({}).to.be.empty", js_expr),
        "isNotEmpty" => format!("pm.expect({}).to.not.be.empty", js_expr),
        "startsWith" => {
            let val = js_literal(a.value.as_ref()?);
            format!("pm.expect(String({})).to.satisfy(v => v.startsWith({}))", js_expr, val)
        },
        "endsWith" => {
            let val = js_literal(a.value.as_ref()?);
            format!("pm.expect(String({})).to.satisfy(v => v.endsWith({}))", js_expr, val)
        },
        "matches" => {
            let val = js_literal(a.value.as_ref()?);
            format!("pm.expect(String({})).to.satisfy(v => new RegExp({}).test(v))", js_expr, val)
        },
        _ => return None,
    };

    Some(format!("pm.test(\"{}\", function () {{\n  {};\n}});", label, check))
}

fn expression_to_js(expr: &str) -> String {
    if expr == "status" {
        return "pm.response.code".to_string();
    }
    if expr == "body" {
        return "pm.response.json()".to_string();
    }
    if let Some(rest) = expr.strip_prefix("body.") {
        let access = rest.split('.').map(|p| format!("?.{}", p)).collect::<String>();
        return format!("pm.response.json(){}", access);
    }
    if let Some(rest) = expr.strip_prefix("headers.") {
        return format!("pm.response.headers.get(\"{}\")", rest);
    }
    // Fallback: treat as a field on the response body
    format!("pm.response.json()?.{}", expr)
}

fn js_literal(val: &serde_json::Value) -> String {
    match val {
        serde_json::Value::String(s) => format!("{:?}", s),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Null => "null".to_string(),
        _ => val.to_string(),
    }
}

fn json_val_display(val: &serde_json::Value) -> String {
    match val {
        serde_json::Value::String(s) => s.clone(),
        _ => val.to_string(),
    }
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            },
            b' ' => out.push('+'),
            _ => {
                out.push('%');
                out.push(char::from_digit((b >> 4) as u32, 16).unwrap_or('0').to_ascii_uppercase());
                out.push(
                    char::from_digit((b & 0xf) as u32, 16).unwrap_or('0').to_ascii_uppercase(),
                );
            },
        }
    }
    out
}

// --- Export helpers ----------------------------------------------------------

fn strip_pre_preamble(s: &str) -> String {
    s.strip_prefix(PRE_PREAMBLE).unwrap_or(s).to_string()
}

fn strip_post_preamble(s: &str) -> String {
    let s = s.strip_prefix(POST_PREAMBLE).unwrap_or(s);
    let s = s.trim_end();
    s.strip_suffix("return results();").unwrap_or(s).trim_end().to_string()
}

// --- Tests -------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_JSON: &str = r#"{
  "opencollection": "1.0.0",
  "info": { "name": "Test Collection" },
  "config": {
    "environments": [{
      "name": "default",
      "selected": true,
      "variables": [
        { "name": "base_url", "value": "https://httpbin.org" },
        { "name": "token", "value": "secret" }
      ]
    }]
  },
  "items": [
    {
      "info": { "name": "get anything", "sequence": 1 },
      "http": {
        "method": "GET",
        "url": "{{base_url}}/anything",
        "headers": [{ "name": "Accept", "value": "application/json" }]
      }
    },
    {
      "info": { "name": "post json", "sequence": 2 },
      "http": {
        "method": "POST",
        "url": "{{base_url}}/post",
        "body": {
          "type": "raw",
          "body": "{\"name\": \"Alice\"}",
          "contentType": "application/json"
        }
      },
      "runtime": {
        "scripts": [
          { "type": "tests", "code": "pm.test(\"ok\", function () { expect(pm.response.code).to.equal(200); });" }
        ]
      }
    }
  ]
}"#;

    #[test]
    fn import_json_basic() {
        let col = OpenCollectionAdapter::import(SAMPLE_JSON).unwrap();
        assert_eq!(col.name, "Test Collection");
        assert_eq!(col.env.get("base_url").map(String::as_str), Some("https://httpbin.org"));
        assert_eq!(col.tests.len(), 2);

        let get = &col.tests[0];
        assert_eq!(get.name, "get anything");
        assert_eq!(get.request.method, "GET");
        assert_eq!(get.request.url, "{{base_url}}/anything");
        assert!(get.pre_script.is_none());
        assert!(get.post_script.is_none());

        let post = &col.tests[1];
        assert_eq!(post.request.body.as_deref(), Some("{\"name\": \"Alice\"}"));
        let ps = post.post_script.as_ref().unwrap();
        assert!(ps.contains(POST_PREAMBLE));
        assert!(ps.contains("return results();"));
    }

    #[test]
    fn import_folder_flattening() {
        let input = r#"{
  "opencollection": "1.0.0",
  "info": { "name": "Col" },
  "items": [{
    "info": { "name": "users" },
    "items": [{
      "info": { "name": "get user" },
      "http": { "method": "GET", "url": "https://api.example.com/users/1" }
    }]
  }]
}"#;
        let col = OpenCollectionAdapter::import(input).unwrap();
        assert_eq!(col.tests.len(), 1);
        assert_eq!(col.tests[0].name, "users/get user");
    }

    #[test]
    fn import_bearer_auth() {
        let input = r#"{
  "opencollection": "1.0.0",
  "info": { "name": "Auth" },
  "items": [{
    "info": { "name": "protected" },
    "http": { "method": "GET", "url": "https://api.example.com/me" },
    "auth": { "type": "bearer", "bearer": { "token": "tok123" } }
  }]
}"#;
        let col = OpenCollectionAdapter::import(input).unwrap();
        let headers = &col.tests[0].request.headers;
        assert!(headers.iter().any(|(k, v)| k == "Authorization" && v == "Bearer tok123"));
    }

    #[test]
    fn import_assertions_become_js() {
        let input = r#"{
  "opencollection": "1.0.0",
  "info": { "name": "Assertions" },
  "items": [{
    "info": { "name": "check" },
    "http": { "method": "GET", "url": "https://api.example.com/data" },
    "runtime": {
      "assertions": [
        { "expression": "status", "operator": "equals", "value": 200 },
        { "expression": "body.name", "operator": "contains", "value": "Alice" }
      ]
    }
  }]
}"#;
        let col = OpenCollectionAdapter::import(input).unwrap();
        let ps = col.tests[0].post_script.as_ref().unwrap();
        assert!(ps.contains("pm.response.code"));
        assert!(ps.contains("pm.response.json()?.name"));
        assert!(ps.contains("to.include"));
    }

    #[test]
    fn import_form_urlencoded() {
        let input = r#"{
  "opencollection": "1.0.0",
  "info": { "name": "Form" },
  "items": [{
    "info": { "name": "login" },
    "http": {
      "method": "POST",
      "url": "https://api.example.com/login",
      "body": {
        "type": "formUrlEncoded",
        "form": [
          { "name": "username", "value": "alice" },
          { "name": "password", "value": "s3cr3t" }
        ]
      }
    }
  }]
}"#;
        let col = OpenCollectionAdapter::import(input).unwrap();
        let tc = &col.tests[0];
        let body = tc.request.body.as_deref().unwrap();
        assert!(body.contains("username=alice"));
        assert!(body.contains("password=s3cr3t"));
        let ct =
            tc.request.headers.iter().find(|(k, _)| k == "Content-Type").map(|(_, v)| v.as_str());
        assert_eq!(ct, Some("application/x-www-form-urlencoded"));
    }

    #[test]
    fn export_roundtrip() {
        let col = OpenCollectionAdapter::import(SAMPLE_JSON).unwrap();
        let json = OpenCollectionAdapter::export(&col.name, &col.tests, &col.env);
        // Must be valid JSON
        assert!(serde_json::from_str::<serde_json::Value>(&json).is_ok());
        let col2 = OpenCollectionAdapter::import(&json).unwrap();
        assert_eq!(col2.name, col.name);
        assert_eq!(col2.tests.len(), col.tests.len());
        assert_eq!(col2.tests[0].name, col.tests[0].name);
        assert_eq!(col2.tests[1].request.body, col.tests[1].request.body);
    }

    #[test]
    fn import_missing_name_defaults() {
        let input = r#"{ "opencollection": "1.0.0", "items": [] }"#;
        let col = OpenCollectionAdapter::import(input).unwrap();
        assert_eq!(col.name, "Unnamed Collection");
    }

    #[test]
    fn import_variant_selected() {
        let input = r#"{
  "opencollection": "1.0.0",
  "info": { "name": "Variants" },
  "config": {
    "environments": [{
      "name": "default",
      "selected": true,
      "variables": [{
        "name": "env",
        "value": "prod",
        "variants": [
          { "value": "staging", "selected": false },
          { "value": "prod", "selected": true }
        ]
      }]
    }]
  },
  "items": []
}"#;
        let col = OpenCollectionAdapter::import(input).unwrap();
        assert_eq!(col.env.get("env").map(String::as_str), Some("prod"));
    }
}
