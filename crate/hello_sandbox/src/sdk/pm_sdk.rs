//! Postman/Bruno `pm` compatibility pack — `sandbox:pm`.
//!
//! Registers the `sandbox:pm` module which provides the `pm`, `res`, `req`,
//! `bru`, `test`, `expect`, and `results` exports for Postman and Bruno
//! compatible post-request scripts.
//!
//! # Usage
//!
//! ```js
//! import { pm, results } from "sandbox:pm";
//!
//! pm.test("status is 200", function() {
//!   pm.expect(pm.response.code).to.equal(200);
//! });
//!
//! return results();
//! ```
//!
//! # Warm slot correctness
//!
//! Response/request data is read lazily via `sandbox.readInput()` inside
//! getters — not captured at module-init time.  The `results()` export resets
//! all module-level mutable state (`_pm_tests`, `_env`, `_vars`, `_globals`)
//! so warm pool slots start each run completely fresh.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use deno_core::{op2, OpDecl, OpState};
use deno_error::JsErrorBox;
use serde::{Deserialize, Serialize};

use crate::config::PmTestResult;
use crate::runtime::RunState;
use crate::sdk::http_sdk::HttpState;
use crate::sdk::SdkExtension;

// ─── Ops ──────────────────────────────────────────────────────────────────────

/// Record the outcome of a single `pm.test()` call in [`RunState`].
///
/// Called by `pm.test(name, fn)` in `sdk-ts/src/pm.js` after the test
/// function is invoked.  Results are collected in `RunState::pm_tests` and
/// forwarded to [`RunMetrics::pm_tests`] after the run.
#[op2(fast)]
fn op_pm_test(state: &mut OpState, pass: bool, #[string] name: String) {
    let run_state = state.borrow_mut::<RunState>();
    run_state.pm_tests.push(PmTestResult { name, passed: pass });
}

/// Request options for [`op_pm_send_request`].
#[derive(Deserialize)]
struct PmSendOptions {
    url: String,
    method: Option<String>,
    headers: Option<Vec<(String, String)>>,
    body: Option<String>,
}

/// Response returned to JS by [`op_pm_send_request`].
#[derive(Serialize)]
struct PmSendResponse {
    status: u16,
    ok: bool,
    headers: Vec<(String, String)>,
    body: String,
    response_time_ms: u64,
}

/// Make a side HTTP request from within a pm script (`pm.sendRequest`).
///
/// Requires [`crate::sdk::http_sdk::HttpPack`] to be registered on the same
/// sandbox — if `HttpState` is absent, returns an error.  The URL must satisfy
/// the same allowlist configured for `HttpPack`.
#[op2(async(deferred))]
#[serde]
async fn op_pm_send_request(
    state: Rc<RefCell<OpState>>,
    #[serde] opts: PmSendOptions,
) -> Result<PmSendResponse, JsErrorBox> {
    let start = Instant::now();

    let (client, effective_allowlist) = {
        let s = state.borrow();
        let hs = s
            .try_borrow::<HttpState>()
            .ok_or_else(|| JsErrorBox::generic("pm.sendRequest: HttpPack is not registered"))?;
        (hs.client.clone(), hs.config.allowed_prefixes.clone())
    };

    if !effective_allowlist.iter().any(|p| opts.url.starts_with(p.as_str())) {
        return Err(JsErrorBox::generic(format!(
            "pm.sendRequest: URL '{}' is not in the sandbox HTTP allowlist.",
            opts.url
        )));
    }

    let method = opts.method.as_deref().unwrap_or("GET");
    let method_parsed = reqwest::Method::from_bytes(method.as_bytes())
        .map_err(|e| JsErrorBox::generic(e.to_string()))?;
    let mut req = client.request(method_parsed, &opts.url);

    if let Some(hdrs) = opts.headers {
        for (k, v) in hdrs {
            req = req.header(k, v);
        }
    }
    if let Some(body) = opts.body {
        req = req.body(body);
    }

    let resp = req.send().await.map_err(|e| JsErrorBox::generic(e.to_string()))?;
    let response_time_ms = start.elapsed().as_millis() as u64;
    let status = resp.status().as_u16();
    let ok = resp.status().is_success();
    let headers: Vec<(String, String)> = resp
        .headers()
        .iter()
        .filter_map(|(k, v)| v.to_str().ok().map(|v| (k.to_string(), v.to_string())))
        .collect();
    let body_bytes = resp.bytes().await.map_err(|e| JsErrorBox::generic(e.to_string()))?;
    let body = String::from_utf8_lossy(&body_bytes).into_owned();

    Ok(PmSendResponse {
        status,
        ok,
        headers,
        body,
        response_time_ms,
    })
}

// ─── Pack ─────────────────────────────────────────────────────────────────────

/// Postman/Bruno compatibility SDK pack — registers `sandbox:pm`.
///
/// Provides the `pm`, `res`, `req`, `bru`, `test`, `expect`, and `results`
/// exports matching the Postman scripting API and Bruno test helpers.
///
/// # Example
///
/// ```rust,ignore
/// use hello_sandbox::{SandboxBuilder, SandboxConfig};
/// use hello_sandbox::sdk::pm_sdk::PmPack;
///
/// let mut sandbox = SandboxBuilder::new()
///     .config(SandboxConfig::power_user())
///     .register(PmPack)
///     .build();
/// ```
pub struct PmPack;

impl SdkExtension for PmPack {
    fn name(&self) -> &'static str {
        "pm"
    }

    fn ops(&self) -> Vec<OpDecl> {
        vec![op_pm_test(), op_pm_send_request()]
    }

    fn esm_files(&self) -> Vec<(&'static str, &'static str)> {
        vec![
            ("sandbox:pm-assert", include_str!("../../sdk-ts/src/pm-assert.js")),
            ("sandbox:pm-helpers", include_str!("../../sdk-ts/src/pm-helpers.js")),
            ("sandbox:pm", include_str!("../../sdk-ts/src/pm.js")),
        ]
    }

    fn ts_declarations(&self) -> &'static str {
        include_str!("../../sdk-ts/types/pm.d.ts")
    }

    fn auto_imports(&self) -> Option<(&'static str, &'static [&'static str])> {
        Some(("sandbox:pm", &["pm", "test", "expect", "res", "req", "bru", "results"]))
    }
}
