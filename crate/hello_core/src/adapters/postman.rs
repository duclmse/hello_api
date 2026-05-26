//! Postman Collection v2.0 / v2.1 import and export adapter.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::types::HttpRequest;
use crate::types::TestCase;

// ─── Preamble constants ───────────────────────────────────────────────────────

const PM_PRE_PREAMBLE: &str = "import { pm, bru, req } from \"sandbox:pm\";\n";
const PM_POST_PREAMBLE: &str =
    "import { pm, res, bru, test, expect, results } from \"sandbox:pm\";\n";
const PM_POST_SUFFIX: &str = "\nreturn results();";

// ─── Serde types ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PostmanCollectionRaw {
    pub info: PostmanInfo,

    #[serde(default)]
    pub item: Vec<PostmanItem>,

    #[serde(default)]
    pub variable: Vec<PostmanVariable>,

    pub auth: Option<PostmanAuth>,

    #[serde(default)]
    pub event: Vec<PostmanEvent>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PostmanInfo {
    pub name: String,
    pub schema: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PostmanItem {
    pub name: String,
    /// If Some, this is a folder containing sub-items
    pub item: Option<Vec<PostmanItem>>,
    pub request: Option<PostmanRequest>,
    #[serde(default)]
    pub event: Vec<PostmanEvent>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PostmanRequest {
    #[serde(default = "default_method")]
    pub method: String,
    pub url: PostmanUrl,
    #[serde(default)]
    pub header: Vec<PostmanHeader>,
    pub body: Option<PostmanBody>,
    pub auth: Option<PostmanAuth>,
    pub description: Option<String>,
}

fn default_method() -> String {
    "GET".to_string()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum PostmanUrl {
    Raw(String),
    Structured {
        raw: String,
        #[serde(default)]
        query: Vec<PostmanQueryParam>,
        #[serde(default)]
        variable: Vec<PostmanUrlVariable>,
    },
}

impl PostmanUrl {
    pub fn raw(&self) -> &str {
        match self {
            PostmanUrl::Raw(s) => s,
            PostmanUrl::Structured { raw, .. } => raw,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PostmanHeader {
    pub key: String,
    pub value: String,
    #[serde(default)]
    pub disabled: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PostmanBody {
    pub mode: String,
    pub raw: Option<String>,
    pub formdata: Option<Vec<PostmanFormField>>,
    pub urlencoded: Option<Vec<PostmanFormField>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PostmanFormField {
    pub key: String,
    pub value: Option<String>,
    pub src: Option<String>,
    #[serde(default)]
    pub disabled: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PostmanAuth {
    #[serde(rename = "type")]
    pub auth_type: String,
    pub bearer: Option<serde_json::Value>,
    pub basic: Option<serde_json::Value>,
    pub apikey: Option<serde_json::Value>,
    pub oauth2: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PostmanEvent {
    pub listen: String,
    pub script: PostmanScript,
    #[serde(default)]
    pub disabled: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PostmanScript {
    pub exec: serde_json::Value,
    #[serde(rename = "type")]
    pub script_type: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PostmanVariable {
    pub key: String,
    pub value: Option<String>,
}

/// Postman environment file — one variable entry (P6).
#[derive(Debug, Deserialize)]
struct PostmanEnvValue {
    pub key: String,
    pub value: Option<String>,
    #[serde(default = "bool_true")]
    pub enabled: bool,
}

fn bool_true() -> bool {
    true
}

/// Postman environment file root (P6).
#[derive(Debug, Deserialize)]
struct PostmanEnv {
    pub values: Vec<PostmanEnvValue>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PostmanQueryParam {
    pub key: String,
    pub value: Option<String>,
    #[serde(default)]
    pub disabled: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PostmanUrlVariable {
    pub key: String,
    pub value: Option<String>,
    #[serde(default)]
    pub disabled: bool,
}

// ─── Public output types ──────────────────────────────────────────────────────

/// Imported Postman collection with flattened test cases.
pub struct PostmanCollection {
    pub name: String,
    pub variables: HashMap<String, String>,
    pub tests: Vec<TestCase>,
}

// ─── Error type ───────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum PostmanError {
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid collection: {0}")]
    Invalid(String),
}

// ─── Helper functions ─────────────────────────────────────────────────────────

/// Extract a string field from Postman auth.
/// Handles both v2.0 (object: `{ "key": "...", "value": "..." }`) and
/// v2.1 (array: `[{ "key": "...", "value": "..." }]`) formats.
fn auth_get(val: &serde_json::Value, key: &str) -> Option<String> {
    match val {
        // v2.1: array of { key, value } objects
        serde_json::Value::Array(arr) => {
            for item in arr {
                if let serde_json::Value::Object(obj) = item {
                    let k = obj.get("key").and_then(|v| v.as_str());
                    if k == Some(key) {
                        return obj.get("value").and_then(|v| match v {
                            serde_json::Value::String(s) => Some(s.clone()),
                            _ => None,
                        });
                    }
                }
            }
            None
        },
        // v2.0: plain object mapping key → value
        serde_json::Value::Object(obj) => {
            obj.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
        },
        _ => None,
    }
}

/// Convert PostmanScript.exec (string or array of strings) to a single String.
fn exec_to_string(exec: &serde_json::Value) -> String {
    match exec {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(arr) => {
            arr.iter().map(|v| v.as_str().unwrap_or("")).collect::<Vec<_>>().join("\n")
        },
        _ => String::new(),
    }
}

/// Convert PostmanAuth to Authorization header(s).
fn auth_to_headers(auth: &PostmanAuth) -> Vec<(String, String)> {
    let mut headers = Vec::new();
    match auth.auth_type.as_str() {
        "bearer" => {
            if let Some(bearer_val) = &auth.bearer
                && let Some(token) = auth_get(bearer_val, "token")
            {
                headers.push(("Authorization".to_string(), format!("Bearer {}", token)));
            }
        },
        "basic" => {
            if let Some(basic_val) = &auth.basic {
                let username = auth_get(basic_val, "username").unwrap_or_default();
                let password = auth_get(basic_val, "password").unwrap_or_default();
                let encoded = STANDARD.encode(format!("{}:{}", username, password));
                headers.push(("Authorization".to_string(), format!("Basic {}", encoded)));
            }
        },
        "apikey" => {
            if let Some(apikey_val) = &auth.apikey {
                let in_val = auth_get(apikey_val, "in").unwrap_or_else(|| "header".to_string());
                if in_val == "header" {
                    let key_name =
                        auth_get(apikey_val, "key").unwrap_or_else(|| "X-API-Key".to_string());
                    let value = auth_get(apikey_val, "value").unwrap_or_default();
                    headers.push((key_name, value));
                }
                // in=query: query params not supported, skip
            }
        },
        "oauth2" => {
            // P1: If a pre-configured accessToken is present, use it as a bearer token.
            if let Some(oauth2_val) = &auth.oauth2 {
                let token = auth_get(oauth2_val, "accessToken")
                    .or_else(|| auth_get(oauth2_val, "access_token"));
                if let Some(token) = token {
                    let prefix = auth_get(oauth2_val, "headerPrefix")
                        .unwrap_or_else(|| "Bearer".to_string());
                    headers.push(("Authorization".to_string(), format!("{} {}", prefix, token)));
                }
            }
        },
        _ => {},
    }
    headers
}

/// Append an API key to the URL query string (P3).
fn append_apikey_query(url: &str, key: &str, value: &str) -> String {
    let sep = if url.contains('?') { '&' } else { '?' };
    format!("{}{}{}={}", url, sep, urlencodestr(key), urlencodestr(value))
}

/// P5 — build a pre-script that resolves Postman dynamic variables.
///
/// Scans URL, header values, and body for `{{$...}}` patterns. Returns a
/// short JS snippet that patches `req` in-place; only the variables actually
/// used are emitted.
fn build_dynamic_var_script(
    url: &str,
    headers: &[(String, String)],
    body: &Option<String>,
) -> Option<String> {
    let all = std::iter::once(url.to_string())
        .chain(headers.iter().map(|(_, v)| v.clone()))
        .chain(body.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ");

    let mut vars: Vec<&str> = Vec::new();
    for name in &[
        "$timestamp",
        "$isoTimestamp",
        "$randomInt",
        "$guid",
        "$randomAlphaNumeric",
        "$randomBoolean",
        "$randomInt",
        "$randomUUID",
    ] {
        if all.contains(&format!("{{{{{name}}}}}")) && !vars.contains(name) {
            vars.push(name);
        }
    }
    if vars.is_empty() {
        return None;
    }

    let mut lines = vec![
        "const _req = sandbox.readInput(\"_request\");".to_string(),
        "function _dyn(s) {".to_string(),
    ];
    for name in &vars {
        let replacement = match *name {
            "$timestamp" => {
                "  s = s.replace(/\\{\\{\\$timestamp\\}\\}/g, Math.floor(Date.now()/1000).toString());"
                    .to_string()
            },
            "$isoTimestamp" => {
                "  s = s.replace(/\\{\\{\\$isoTimestamp\\}\\}/g, new Date().toISOString());"
                    .to_string()
            },
            "$randomInt" => {
                "  s = s.replace(/\\{\\{\\$randomInt\\}\\}/g, Math.floor(Math.random()*1000).toString());"
                    .to_string()
            },
            "$guid" | "$randomUUID" => {
                let pat = name.replace('$', "\\$");
                format!("  s = s.replace(/\\{{\\{{\\{}\\}}\\}}/g, crypto.randomUUID());", pat)
            },
            _ => {
                let pat = name.replace('$', "\\$");
                format!(
                    "  s = s.replace(/\\{{\\{{\\{}\\}}\\}}/g, Math.random().toString(36).slice(2));",
                    pat
                )
            },
        };
        lines.push(replacement);
    }
    lines.push("  return s;".to_string());
    lines.push("}".to_string());
    lines.push("_req.url = _dyn(_req.url || \"\");".to_string());
    lines.push("_req.headers = (_req.headers||[]).map(([k,v]) => [k, _dyn(v)]);".to_string());
    lines.push("if (_req.body) _req.body = _dyn(_req.body);".to_string());
    lines.push("return _req;".to_string());
    Some(lines.join("\n"))
}

// ─── Import helper: collect items recursively ─────────────────────────────────

fn collect_items(
    items: &[PostmanItem],
    folder_path: &str,
    parent_auth: Option<&PostmanAuth>,
) -> Vec<TestCase> {
    let mut cases = Vec::new();

    for item in items {
        if let Some(sub_items) = &item.item {
            // This is a folder; recurse
            let sub_path = if folder_path.is_empty() {
                item.name.clone()
            } else {
                format!("{}/{}", folder_path, item.name)
            };
            let folder_auth = item.request.as_ref().and_then(|r| r.auth.as_ref()).or(parent_auth);
            let sub_cases = collect_items(sub_items, &sub_path, folder_auth);
            cases.extend(sub_cases);
        } else if let Some(req) = &item.request {
            // Determine the effective name
            let name = if folder_path.is_empty() {
                item.name.clone()
            } else {
                format!("{}/{}", folder_path, item.name)
            };

            // Effective auth: request > parent
            let effective_auth = req.auth.as_ref().or(parent_auth);

            // Build headers
            let mut headers: Vec<(String, String)> = req
                .header
                .iter()
                .filter(|h| !h.disabled)
                .map(|h| (h.key.clone(), h.value.clone()))
                .collect();

            if let Some(auth) = effective_auth {
                let auth_headers = auth_to_headers(auth);
                headers.extend(auth_headers);
            }

            // URL — P3: append apikey-in-query to URL
            let mut url = req.url.raw().to_string();
            if let Some(auth) = effective_auth
                && auth.auth_type == "apikey"
                && let Some(apikey_val) = &auth.apikey
            {
                let in_val = auth_get(apikey_val, "in").unwrap_or_else(|| "header".to_string());
                if in_val == "query" {
                    let key = auth_get(apikey_val, "key").unwrap_or_else(|| "api_key".to_string());
                    let value = auth_get(apikey_val, "value").unwrap_or_default();
                    url = append_apikey_query(&url, &key, &value);
                }
            }

            // Body
            let body = match &req.body {
                Some(b) if b.mode == "raw" => b.raw.clone(),
                Some(b) if b.mode == "urlencoded" => {
                    if let Some(fields) = &b.urlencoded {
                        let encoded: String = fields
                            .iter()
                            .filter(|f| !f.disabled)
                            .map(|f| {
                                format!(
                                    "{}={}",
                                    urlencodestr(&f.key),
                                    urlencodestr(f.value.as_deref().unwrap_or(""))
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("&");
                        Some(encoded)
                    } else {
                        None
                    }
                },
                // P2: multipart/formdata — represent as key=value or key=<file:src> pairs.
                Some(b) if b.mode == "formdata" => {
                    if let Some(fields) = &b.formdata {
                        let parts: Vec<String> = fields
                            .iter()
                            .filter(|f| !f.disabled)
                            .map(|f| {
                                let v = f
                                    .src
                                    .as_ref()
                                    .map(|s| format!("<file:{}>", s))
                                    .or_else(|| f.value.clone())
                                    .unwrap_or_default();
                                format!("{}={}", f.key, v)
                            })
                            .collect();
                        if parts.is_empty() {
                            None
                        } else {
                            Some(parts.join("&"))
                        }
                    } else {
                        None
                    }
                },
                _ => None,
            };

            // Add Content-Type for formdata body if not already set (P2)
            if req.body.as_ref().is_some_and(|b| b.mode == "formdata")
                && !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            {
                headers.push(("Content-Type".to_string(), "multipart/form-data".to_string()));
            }

            // Collect events from both item and request levels
            // Item events take priority, otherwise fall back to none
            let events = if item.event.is_empty() {
                &[] as &[PostmanEvent]
            } else {
                &item.event
            };

            // P5: detect Postman dynamic variables ({{$...}}) and prepend resolver pre-script
            let dynamic_pre = build_dynamic_var_script(&url, &headers, &body);

            // pre_script from events
            let pre_script = events
                .iter()
                .find(|e| e.listen == "prerequest" && !e.disabled)
                .map(|e| exec_to_string(&e.script.exec))
                .filter(|s| !s.trim().is_empty())
                .map(|s| format!("{}{}", PM_PRE_PREAMBLE, s));

            // Prepend the dynamic-var resolver (if needed) before the user pre-script
            let pre_script = match (dynamic_pre, pre_script) {
                (Some(dyn_pre), Some(user_pre)) => Some(format!(
                    "{}{}\n{}",
                    PM_PRE_PREAMBLE,
                    dyn_pre,
                    &user_pre[PM_PRE_PREAMBLE.len()..]
                )),
                (Some(dyn_pre), None) => Some(format!("{}{}", PM_PRE_PREAMBLE, dyn_pre)),
                (None, user_pre) => user_pre,
            };

            // post_script from events
            let post_script = events
                .iter()
                .find(|e| e.listen == "test" && !e.disabled)
                .map(|e| exec_to_string(&e.script.exec))
                .filter(|s| !s.trim().is_empty())
                .map(|s| format!("{}{}{}", PM_POST_PREAMBLE, s, PM_POST_SUFFIX));

            cases.push(TestCase {
                name,
                request: HttpRequest {
                    url,
                    method: req.method.clone(),
                    headers,
                    body,
                },
                pre_script,
                post_script,
                ..Default::default()
            });
        }
    }

    cases
}

/// Simple percent-encoding for URL form values.
fn urlencodestr(s: &str) -> String {
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

// ─── Export helpers ───────────────────────────────────────────────────────────

/// Convert a single [`TestCase`] to a Postman Collection v2.1 request item.
fn tc_to_item(name: &str, tc: &TestCase) -> serde_json::Value {
    let (host_parts, path_parts) = split_url_host_path(&tc.request.url);
    let url_obj = serde_json::json!({
        "raw": tc.request.url,
        "host": host_parts,
        "path": path_parts,
    });

    let headers: Vec<serde_json::Value> =
        tc.request.headers.iter().map(|(k, v)| serde_json::json!({"key": k, "value": v})).collect();

    let mut request = serde_json::json!({
        "method": tc.request.method,
        "url": url_obj,
        "header": headers,
    });

    if let Some(b) = &tc.request.body {
        request["body"] = serde_json::json!({"mode": "raw", "raw": b});
    }

    let mut events: Vec<serde_json::Value> = Vec::new();
    if let Some(pre) = &tc.pre_script {
        let script = pre.strip_prefix(PM_PRE_PREAMBLE).unwrap_or(pre);
        events.push(serde_json::json!({
            "listen": "prerequest",
            "script": {
                "type": "text/javascript",
                "exec": script.lines().collect::<Vec<_>>(),
            }
        }));
    }
    if let Some(post) = &tc.post_script {
        let script = post.strip_prefix(PM_POST_PREAMBLE).unwrap_or(post);
        let script = script.strip_suffix(PM_POST_SUFFIX).unwrap_or(script);
        events.push(serde_json::json!({
            "listen": "test",
            "script": {
                "type": "text/javascript",
                "exec": script.lines().collect::<Vec<_>>(),
            }
        }));
    }

    serde_json::json!({
        "name": name,
        "request": request,
        "event": events,
    })
}

/// Recursively group test cases by the `/`-separated path components of their names.
///
/// `"Users/Get All"` → folder `"Users"` containing item `"Get All"`.
/// Insertion order is preserved; cases with the same leading folder component are
/// placed in the same folder regardless of interleaving with other cases.
fn group_into_folders(tagged: Vec<(Vec<String>, &TestCase)>) -> Vec<serde_json::Value> {
    enum Entry<'a> {
        Folder(String, Vec<(Vec<String>, &'a TestCase)>),
        Leaf(String, &'a TestCase),
    }

    let mut entries: Vec<Entry> = Vec::new();
    let mut folder_idx: HashMap<String, usize> = HashMap::new();

    for (parts, tc) in tagged {
        if parts.len() <= 1 {
            let display = parts.into_iter().next().unwrap_or_else(|| tc.name.clone());
            entries.push(Entry::Leaf(display, tc));
        } else {
            let folder_name = parts[0].clone();
            let rest = parts[1..].to_vec();
            if let Some(&idx) = folder_idx.get(&folder_name) {
                if let Entry::Folder(_, children) = &mut entries[idx] {
                    children.push((rest, tc));
                }
            } else {
                let idx = entries.len();
                folder_idx.insert(folder_name.clone(), idx);
                entries.push(Entry::Folder(folder_name, vec![(rest, tc)]));
            }
        }
    }

    entries
        .into_iter()
        .map(|e| match e {
            Entry::Leaf(name, tc) => tc_to_item(&name, tc),
            Entry::Folder(name, children) => serde_json::json!({
                "name": name,
                "item": group_into_folders(children),
            }),
        })
        .collect()
}

// ─── Adapter ──────────────────────────────────────────────────────────────────

pub struct PostmanAdapter;

impl PostmanAdapter {
    /// Import a Postman Collection JSON string (v2.0 or v2.1) into a
    /// [`PostmanCollection`] with flattened test cases.
    pub fn import(json: &str) -> Result<PostmanCollection, PostmanError> {
        let raw: PostmanCollectionRaw = serde_json::from_str(json)?;

        let variables: HashMap<String, String> = raw
            .variable
            .iter()
            .map(|v| (v.key.clone(), v.value.clone().unwrap_or_default()))
            .collect();

        let tests = collect_items(&raw.item, "", raw.auth.as_ref());

        Ok(PostmanCollection {
            name: raw.info.name,
            variables,
            tests,
        })
    }

    /// Export a list of test cases to a Postman Collection v2.1 JSON string.
    ///
    /// Test cases whose names contain `/` are reconstructed into nested folders
    /// (P4). E.g. `"Users/Get All"` becomes a folder `"Users"` containing request
    /// `"Get All"`.
    pub fn export(name: &str, tests: &[TestCase], variables: &HashMap<String, String>) -> String {
        // Pair each TestCase with its name path components.
        let tagged: Vec<(Vec<String>, &TestCase)> = tests
            .iter()
            .map(|tc| {
                let parts: Vec<String> = tc.name.split('/').map(|s| s.to_string()).collect();
                (parts, tc)
            })
            .collect();
        let items = group_into_folders(tagged);

        // Collection-level variables
        let vars: Vec<serde_json::Value> =
            variables.iter().map(|(k, v)| serde_json::json!({ "key": k, "value": v })).collect();

        let collection = serde_json::json!({
            "info": {
                "name": name,
                "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"
            },
            "item": items,
            "variable": vars,
        });

        serde_json::to_string_pretty(&collection).unwrap_or_default()
    }

    /// Import a Postman Collection together with a Postman environment JSON file (P6).
    ///
    /// Environment variables (enabled entries only) are merged into
    /// `PostmanCollection.variables`, with env vars overriding collection vars of
    /// the same name.
    pub fn import_with_env(
        collection_json: &str,
        env_json: &str,
    ) -> Result<PostmanCollection, PostmanError> {
        let mut collection = Self::import(collection_json)?;
        let env: PostmanEnv = serde_json::from_str(env_json)?;
        for entry in env.values {
            if entry.enabled {
                collection.variables.insert(entry.key, entry.value.unwrap_or_default());
            }
        }
        Ok(collection)
    }
}

/// Split a URL string into host parts and path parts for Postman's structured
/// URL object (best-effort; falls back to empty vecs on parse failure).
fn split_url_host_path(url: &str) -> (Vec<String>, Vec<String>) {
    // Strip protocol
    let without_proto = if let Some(rest) = url.strip_prefix("https://") {
        rest
    } else if let Some(rest) = url.strip_prefix("http://") {
        rest
    } else {
        url
    };

    // Split host from path
    let (host_str, path_str) = if let Some(idx) = without_proto.find('/') {
        (&without_proto[..idx], &without_proto[idx + 1..])
    } else {
        (without_proto, "")
    };

    let host_parts: Vec<String> = host_str.split('.').map(|s| s.to_string()).collect();
    let path_parts: Vec<String> =
        path_str.split('/').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();

    (host_parts, path_parts)
}
