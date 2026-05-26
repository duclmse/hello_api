//! HTTP pack -- outbound fetch behind a host-controlled allowlist.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use deno_core::{op2, OpDecl, OpState};
use deno_error::JsErrorBox;
use serde::{Deserialize, Serialize};

use crate::runtime::RunState;
use crate::sdk::SdkExtension;

// ─── Config ───────────────────────────────────────────────────────────────────

/// Configuration for `HttpPack`.
#[derive(Clone, Debug)]
pub struct HttpConfig {
    /// URL prefixes scripts are allowed to reach.
    pub allowed_prefixes: Vec<String>,
    /// Per-request timeout.
    pub timeout: Duration,
    /// Maximum response body size (bytes).
    pub max_response_bytes: usize,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            allowed_prefixes: vec![],
            timeout: Duration::from_secs(10),
            max_response_bytes: 1024 * 1024,
        }
    }
}

// ─── Op state ────────────────────────────────────────────────────────────────

/// Per-slot HTTP state: allowlist config + a reusable reqwest client.
pub struct HttpState {
    pub config: HttpConfig,
    pub client: reqwest::Client,
}

// ─── Response shape returned to JS ───────────────────────────────────────────

#[derive(Serialize)]
struct JsResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: String, // base64-encoded for binary safety
    ok: bool,
    /// `true` if at least one HTTP redirect was followed to reach this response.
    redirected: bool,
}

// ─── Ops ─────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct FetchOptions {
    method: Option<String>,
    headers: Option<Vec<(String, String)>>,
    body: Option<String>,
}

/// Fetch `url` with `opts`. Capability checks happen **before** any network I/O.
#[op2(async(deferred))]
#[serde]
async fn op_http_fetch(
    state: Rc<RefCell<OpState>>,
    #[string] url: String,
    #[serde] opts: FetchOptions,
) -> Result<JsResponse, JsErrorBox> {
    // All synchronous checks — capabilities, rate limits, allowlist — before any I/O.
    let (client, effective_allowlist, max_response_bytes) = {
        let mut s = state.borrow_mut();
        let run_state = s.borrow_mut::<RunState>();

        // Pack-level enable/disable.
        if run_state.capabilities.http_enabled == Some(false) {
            return Err(JsErrorBox::generic("capability denied: http"));
        }

        // HTTP method restriction (if configured).
        let method = opts.method.as_deref().unwrap_or("GET");
        if let Some(allowed_methods) = &run_state.capabilities.http_allowed_methods {
            if !allowed_methods.iter().any(|m| m.eq_ignore_ascii_case(method)) {
                return Err(JsErrorBox::generic(format!(
                    "capability denied: HTTP method '{method}' is not allowed"
                )));
            }
        }

        // Per-run rate limit: capability override takes precedence.
        let limit =
            run_state.capabilities.http_calls_limit.or(run_state.rate_limits.http_calls_per_run);
        run_state.http_calls += 1;
        if let Some(lim) = limit {
            if run_state.http_calls > lim {
                run_state.rate_limit_exceeded = Some(("http".to_string(), lim));
                return Err(JsErrorBox::generic(format!(
                    "rate limit exceeded: http (limit: {lim})"
                )));
            }
        }

        // Effective allowlist: per-run override replaces pool-level list.
        let cap_prefixes = run_state.capabilities.http_allowed_prefixes.clone();
        let _ = run_state; // release mutable borrow before taking immutable one

        let hs = s.borrow::<HttpState>();
        let effective = cap_prefixes.unwrap_or_else(|| hs.config.allowed_prefixes.clone());
        (hs.client.clone(), effective, hs.config.max_response_bytes)
    };

    // Allowlist check -- must happen before any network I/O.
    if !effective_allowlist.iter().any(|p| url.starts_with(p.as_str())) {
        return Err(JsErrorBox::generic(format!(
            "fetch: URL '{}' is not in the sandbox HTTP allowlist.",
            url
        )));
    }

    // Build the request.
    let method = opts.method.as_deref().unwrap_or("GET");
    let method_parsed = reqwest::Method::from_bytes(method.as_bytes())
        .map_err(|e| JsErrorBox::generic(e.to_string()))?;
    let mut req = client.request(method_parsed, &url);

    if let Some(hdrs) = opts.headers {
        for (k, v) in hdrs {
            req = req.header(k, v);
        }
    }
    if let Some(body) = opts.body {
        req = req.body(body);
    }

    // Network I/O.
    let resp = req.send().await.map_err(|e| JsErrorBox::generic(e.to_string()))?;
    let status = resp.status().as_u16();
    let ok = resp.status().is_success();
    // Detect whether any redirect was followed: compare the final response URL
    // with the original request URL (both parsed so normalisation is applied).
    let redirected = reqwest::Url::parse(&url).map(|orig| orig != *resp.url()).unwrap_or(false);
    let headers: Vec<(String, String)> = resp
        .headers()
        .iter()
        .filter_map(|(k, v)| v.to_str().ok().map(|v| (k.to_string(), v.to_string())))
        .collect();

    let body_bytes = resp.bytes().await.map_err(|e| JsErrorBox::generic(e.to_string()))?;

    if body_bytes.len() > max_response_bytes {
        return Err(JsErrorBox::generic(format!(
            "fetch: response body ({} bytes) exceeds max_response_bytes ({} bytes).",
            body_bytes.len(),
            max_response_bytes
        )));
    }

    use base64::Engine;
    let body = base64::engine::general_purpose::STANDARD.encode(&body_bytes);

    Ok(JsResponse {
        status,
        headers,
        body,
        ok,
        redirected,
    })
}

// ─── Pack ────────────────────────────────────────────────────────────────────

/// HTTP SDK pack -- allowlist-gated `fetch`.
pub struct HttpPack {
    config: HttpConfig,
}

impl HttpPack {
    /// Create a new `HttpPack` with the given configuration.
    pub fn new(config: HttpConfig) -> Self {
        Self { config }
    }
}

impl SdkExtension for HttpPack {
    fn name(&self) -> &'static str {
        "http"
    }

    fn ops(&self) -> Vec<OpDecl> {
        vec![op_http_fetch()]
    }

    fn esm_files(&self) -> Vec<(&'static str, &'static str)> {
        vec![("sandbox:http", include_str!("../../sdk-ts/src/http.js"))]
    }

    fn ts_declarations(&self) -> &'static str {
        include_str!("../../sdk-ts/types/http.d.ts")
    }

    fn inject_op_state(&self, op_state: &mut deno_core::OpState) {
        let client =
            reqwest::Client::builder().timeout(self.config.timeout).build().unwrap_or_default();
        op_state.put(HttpState {
            config: self.config.clone(),
            client,
        });
    }
}
