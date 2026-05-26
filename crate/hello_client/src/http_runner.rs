//! HTTP Test Runner — Postman/Insomnia-style request pipeline.
//!
//! Implements the HTTP client feature list (F1–F9):
//!
//! - **F1** — [`HttpTestRunner`]: five-phase orchestration (collection-pre → pre → fetch → post → collection-post)
//! - **F2** — environment store via KvPack + KV prefix per collection, plus threaded Postman-style stores (`pm.environment`, `pm.collectionVariables`, `pm.globals`)
//! - **F3** — built-in `sandbox:test` assertion and `sandbox:pm` compatibility modules registered automatically
//! - **F4** — [`HttpResponse`] with `responseTime`, case-insensitive headers
//! - **F5** — [`HttpTestRunner::run_collection`]: sequential run with shared KV and persistent Postman environment states
//! - **F6** — [`HttpTestRunner::set_env`] / [`interpolate`]: `{{var}}` substitution and dynamic helper functions
//! - **F7** — [`HistorySink`] / [`SqliteHistorySink`]: optional test history persistence
//! - **F8** — [`SecurityProfile`]: pre-built [`RunCapabilities`] profiles
//! - **F9** — `sandbox:assert` formal assertion op (via [`AssertPack`], always registered)
//!
//! **Must be used on a `tokio::task::LocalSet`** — the underlying V8 runtime
//! is `!Send`.

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use hello_core::{HttpRequest, TestCase};
use hello_sandbox::sdk::assert_sdk::AssertPack;
use hello_sandbox::sdk::http_sdk::{HttpConfig, HttpPack};
use hello_sandbox::sdk::kv_sdk::KvPack;
use hello_sandbox::{
    PmPack, PoolConfig, RunCapabilities, RunMetrics, Sandbox, SandboxConfig, SandboxError,
    SandboxEvent,
};

// ─── JS module sources ────────────────────────────────────────────────────────

const TEST_JS: &str = include_str!("../sdk-ts/src/test.js");
const TEST_DTS: &str = include_str!("../sdk-ts/types/test.d.ts");

/// Internal script run for the fetch phase. Sends the request described by the
/// `_request` input and returns a serialised [`FetchScriptResult`].
///
/// We call `op_http_fetch` directly (bypassing the `SandboxResponse` wrapper)
/// so we can return the base64-encoded body without needing `TextDecoder`,
/// which is not provided by `deno_core` alone. The body is decoded to UTF-8
/// in Rust using the `base64` crate.
const FETCH_SCRIPT: &str = r#"
const ops = globalThis.__sandbox_ops;
const req = sandbox.readInput("_request");
const t0 = Date.now();
const raw = await ops.op_http_fetch(req.url, {
  method: req.method || "GET",
  headers: req.headers || [],
  body: req.body ?? null,
});
return {
  status: raw.status,
  ok: raw.ok,
  headers: raw.headers,
  body_b64: raw.body,
  response_time_ms: Date.now() - t0,
  redirected: raw.redirected || false,
};
"#;

// ─── Public types ─────────────────────────────────────────────────────────────

/// HTTP response captured from the sandbox `fetch` call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpResponse {
    /// HTTP status code.
    pub status: u16,
    /// `true` if status is in the 2xx range.
    pub ok: bool,
    /// Response headers as `(name, value)` pairs.
    pub headers: Vec<(String, String)>,
    /// Response body as a decoded UTF-8 string.
    pub body: String,
    /// Time from the start of the fetch call to receiving the full body (ms).
    pub response_time_ms: u64,
    /// `true` if at least one HTTP redirect was followed to reach this response.
    pub redirected: bool,
}

/// Wall-clock time spent in each phase of a single [`TestCase`] execution.
///
/// Fields are `None` when the corresponding phase did not run (e.g. no pre-script).
/// `fetch_ms` comes from the `Date.now()` delta measured inside the fetch script,
/// so it reflects actual network latency rather than sandbox overhead.
#[derive(Debug, Default, Clone)]
pub struct PhaseTimings {
    /// Phase 0 — collection pre-script (ms).
    pub collection_pre_ms: Option<u64>,

    /// Phase 1 — request pre-script (ms).
    pub pre_ms: Option<u64>,

    /// Phase 2 — HTTP fetch network round-trip (ms).
    pub fetch_ms: Option<u64>,

    /// Phase 3 — request post-script (ms).
    pub post_ms: Option<u64>,

    /// Phase 4 — collection post-script (ms).
    pub collection_post_ms: Option<u64>,
}

/// Result of executing a single [`TestCase`].
#[derive(Debug)]
pub struct TestResult {
    /// Test case name.
    pub name: String,
    /// `true` if the post-script's `results()` returned `pass: true`.
    pub passed: bool,
    /// Failure messages from `expect(…)` assertions in the post-script.
    pub failures: Vec<String>,
    /// The effective HTTP request that was sent (after pre-script overrides and interpolation).
    pub request: HttpRequest,
    /// The HTTP response returned by the fetch phase.
    pub response: Option<HttpResponse>,
    /// `console.*` output from pre and post scripts.
    pub logs: Vec<String>,
    /// Events emitted via `sandbox.emit()` across all three phases.
    pub events: Vec<SandboxEvent>,
    /// Metrics from the last phase that ran.
    pub metrics: RunMetrics,
    /// Per-phase wall-clock timings.
    pub phase_timings: PhaseTimings,
    /// Rendered HTML from `pm.visualizer.set(template, data)`, if called in
    /// the post-script. Open in a browser to view the visualisation.
    pub visualizer_html: Option<String>,
    /// Absolute path of the file that was written when `TestCase::output_file`
    /// is set. `None` if no output file was requested or if writing failed.
    pub output_written: Option<String>,
}

/// Aggregate result of a [`HttpTestRunner::run_collection`] call.
pub struct CollectionResult {
    /// Number of test cases that passed.
    pub passed: usize,
    /// Number of test cases that failed.
    pub failed: usize,
    /// Individual results in the same order as the input test cases.
    pub results: Vec<TestResult>,
    /// Wall-clock time for the entire collection run.
    pub total_duration: Duration,
}

// ─── F7 — History persistence ─────────────────────────────────────────────────

/// Implement to persist test results after each run.
///
/// Called synchronously by [`HttpTestRunner::run_test`] immediately before
/// the result is returned to the caller.  Errors should be handled internally
/// (e.g. log and continue) — a failing sink must not abort the test run.
///
/// # Example
///
/// ```rust,ignore
/// runner.set_history(SqliteHistorySink::open("history.db")?);
/// ```
pub trait HistorySink: Send + Sync + 'static {
    fn record(&self, result: &TestResult);
}

/// SQLite-backed [`HistorySink`] — inserts one row per [`TestResult`] into a
/// `test_history` table, creating it on first use.
///
/// Thread-safe: the connection is guarded by a `Mutex` so the sink can be
/// shared across threads even though `rusqlite::Connection` is `!Sync`.
pub struct SqliteHistorySink {
    conn: std::sync::Mutex<rusqlite::Connection>,
}

impl SqliteHistorySink {
    fn init(conn: rusqlite::Connection) -> Result<Self, rusqlite::Error> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS test_history (
                id                INTEGER PRIMARY KEY AUTOINCREMENT,
                name              TEXT    NOT NULL,
                passed            INTEGER NOT NULL,
                failures          TEXT    NOT NULL,
                response_status   INTEGER,
                response_time_ms  INTEGER,
                logs              TEXT    NOT NULL,
                elapsed_ms        INTEGER NOT NULL,
                heap_bytes        INTEGER NOT NULL,
                assertions_passed INTEGER NOT NULL,
                assertions_failed INTEGER NOT NULL,
                recorded_at       INTEGER NOT NULL
            );",
        )?;
        Ok(Self {
            conn: std::sync::Mutex::new(conn),
        })
    }

    /// Open (or create) a file-backed history database at `path`.
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self, rusqlite::Error> {
        Self::init(rusqlite::Connection::open(path)?)
    }

    /// Create an in-memory history database (useful for testing).
    pub fn in_memory() -> Result<Self, rusqlite::Error> {
        Self::init(rusqlite::Connection::open_in_memory()?)
    }
}

impl HistorySink for SqliteHistorySink {
    fn record(&self, result: &TestResult) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let failures_json = serde_json::to_string(&result.failures).unwrap_or_default();
        let logs_json = serde_json::to_string(&result.logs).unwrap_or_default();
        let guard = match self.conn.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let _ = guard.execute(
            "INSERT INTO test_history
             (name, passed, failures, response_status, response_time_ms,
              logs, elapsed_ms, heap_bytes, assertions_passed, assertions_failed,
              recorded_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            rusqlite::params![
                result.name,
                result.passed as i64,
                failures_json,
                result.response.as_ref().map(|r| r.status as i64),
                result.response.as_ref().map(|r| r.response_time_ms as i64),
                logs_json,
                result.metrics.elapsed.as_millis() as i64,
                result.metrics.peak_heap_bytes as i64,
                result.metrics.assertions_passed as i64,
                result.metrics.assertions_failed as i64,
                now,
            ],
        );
    }
}

// ─── Script prelude ───────────────────────────────────────────────────────────

/// Exported names from `sandbox:test` that are auto-imported into every
/// post-script that does not already import them explicitly.
const TEST_EXPORTS: &[&str] = &["expect", "wrapResponse", "results"];

/// All exports from `sandbox:pm` — used for the pm-style auto-prelude.
const PM_EXPORTS: &[&str] = &["pm", "res", "req", "bru", "test", "expect", "results"];

/// Appends `return results();` if the script does not already end with a `return` statement.
fn append_results_if_missing(mut script: String) -> String {
    let last_stmt = script.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    if last_stmt.starts_with("return") {
        script
    } else {
        script.push_str("\nreturn results();");
        script
    }
}

/// Build an auto-import prelude for a script.
///
/// If the script uses `pm.` (Postman-style) but has not yet imported `pm`,
/// injects missing `sandbox:pm` exports. Otherwise injects missing
/// `sandbox:test` exports. Either way appends `return results();` if absent.
fn build_prelude(source: &str) -> String {
    let imported = crate::runner::scan_sandbox_imports(source);

    // If the script accesses the `pm` object and hasn't imported it, use the
    // sandbox:pm prelude so `pm`, `res`, `req`, `bru`, `test`, and `results`
    // are all available without explicit imports.
    if !imported.contains("pm") && source.contains("pm.") {
        let missing: Vec<&str> =
            PM_EXPORTS.iter().copied().filter(|&n| !imported.contains(n)).collect();
        if !missing.is_empty() {
            return format!("import {{ {} }} from \"sandbox:pm\";\n", missing.join(", "));
        }
        return String::new();
    }

    let missing: Vec<&str> =
        TEST_EXPORTS.iter().copied().filter(|&name| !imported.contains(name)).collect();
    if missing.is_empty() {
        return String::new();
    }
    format!("import {{ {} }} from \"sandbox:test\";\n", missing.join(", "))
}

// ─── Internal helpers ─────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct VisualizerPayload {
    template: String,
    data: Value,
}

#[derive(Debug, Deserialize)]
struct PostScriptResults {
    pass: bool,
    failures: Vec<String>,
    #[serde(default)]
    visualizer: Option<VisualizerPayload>,
    /// pm.environment snapshot returned by results() — threaded to next run (S4).
    #[serde(default)]
    pm_env: Option<Value>,
    /// pm.collectionVariables snapshot (S4).
    #[serde(default)]
    pm_col_vars: Option<Value>,
    /// pm.globals snapshot (S4).
    #[serde(default)]
    pm_globals: Option<Value>,
}

/// Render a Postman-compatible visualizer HTML page.
///
/// The page embeds Handlebars.js from a CDN so `{{key}}` template syntax works
/// identically to Postman's Visualize tab. The compiled output is rendered into
/// an iframe via `srcdoc` so scripts inside the template (e.g. Chart.js) execute.
/// Opening the file in a browser renders the template against the captured data.
fn render_visualizer_html(test_name: &str, template: &str, data: &Value) -> String {
    let data_json = serde_json::to_string(data).unwrap_or_else(|_| "{}".to_string());
    // JSON-encode the template so it can be safely embedded inside a <script> block.
    // Replace any "</script>" sequence (even after JSON encoding) so the outer
    // script tag is never terminated by content inside the template or data.
    let template_json = serde_json::to_string(template)
        .unwrap_or_else(|_| "\"\"".to_string())
        .replace("</script>", "<\\/script>");
    let data_json_safe = data_json.replace("</script>", "<\\/script>");
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <title>{title}</title>
  <script src="https://cdn.jsdelivr.net/npm/handlebars@4/dist/handlebars.min.js"></script>
  <style>html,body{{margin:0;padding:0;height:100%}}iframe{{width:100%;height:100vh;border:none;display:block}}</style>
</head>
<body>
  <iframe id="pm-frame"></iframe>
  <script>
    var _data = {data_json};
    var _tpl = {template_json};
    var compiled = Handlebars.compile(_tpl)(_data);
    document.getElementById("pm-frame").srcdoc = compiled;
  </script>
</body>
</html>"#,
        title = test_name,
        template_json = template_json,
        data_json = data_json_safe,
    )
}

#[derive(Debug, Deserialize)]
struct FetchScriptResult {
    status: u16,
    ok: bool,
    headers: Vec<(String, String)>,
    body_b64: String,
    response_time_ms: u64,
    redirected: bool,
}

fn make_caps(test: &TestCase, phase: &str) -> RunCapabilities {
    let mut tags = test.tags.clone();
    tags.insert("_phase".into(), phase.into());
    tags.insert("_test".into(), test.name.clone());
    RunCapabilities {
        tags,
        timeout_override: test.timeout_override,
        kv_key_prefix: test.kv_key_prefix.clone(),
        http_allowed_prefixes: test.http_allowed_prefixes.clone(),
        ..Default::default()
    }
}

fn merge_request(req: &mut HttpRequest, val: &Value) {
    let Value::Object(map) = val else { return };
    if let Some(Value::String(url)) = map.get("url") {
        req.url = url.clone();
    }
    if let Some(Value::String(method)) = map.get("method") {
        req.method = method.clone();
    }
    if let Some(Value::Array(arr)) = map.get("headers") {
        let headers: Vec<(String, String)> = arr
            .iter()
            .filter_map(|v| {
                if let Value::Array(pair) = v
                    && let [Value::String(k), Value::String(v)] = pair.as_slice()
                {
                    Some((k.clone(), v.clone()))
                } else {
                    None
                }
            })
            .collect();
        req.headers = headers;
    }
    match map.get("body") {
        Some(Value::String(body)) => req.body = Some(body.clone()),
        Some(Value::Null) => req.body = None,
        _ => {},
    }
}

/// Parse a saved HTTP message file (written by `format_http_response`) back into
/// its components.  Handles both `\n` and `\r\n` line endings.
///
/// Falls back to treating the entire content as the body when no header/body
/// separator is found, so plain response body files still work as `response_file`.
fn parse_http_message(bytes: &[u8]) -> (u16, bool, Vec<(String, String)>, Vec<u8>) {
    // Locate the blank line separating headers from body (\r\n\r\n or \n\n).
    let sep = bytes
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| (i, i + 4))
        .or_else(|| bytes.windows(2).position(|w| w == b"\n\n").map(|i| (i, i + 2)));

    let (header_end, body_start) = match sep {
        Some(s) => s,
        None => return (200, true, vec![], bytes.to_vec()),
    };

    let header_text = String::from_utf8_lossy(&bytes[..header_end]);
    let body_bytes = bytes[body_start..].to_vec();

    let mut lines = header_text.lines();
    let status: u16 = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(200);
    let ok = (200..300).contains(&status);

    let headers: Vec<(String, String)> = lines
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            Some((k.trim().to_lowercase().to_owned(), v.trim().to_owned()))
        })
        .collect();

    (status, ok, headers, body_bytes)
}

/// Serialise a full HTTP response to bytes: status line, headers, blank line, raw body.
///
/// Uses `\n` line endings (rather than `\r\n`) so the file is comfortable to
/// open in any editor.  The body is written as raw bytes so binary responses
/// (images, PDFs, etc.) are preserved exactly.
fn format_http_response(resp: &HttpResponse, body_bytes: &[u8]) -> Vec<u8> {
    let status_text = match resp.status {
        100 => "Continue",
        101 => "Switching Protocols",
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        206 => "Partial Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        410 => "Gone",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "",
    };

    let mut out = String::new();
    if status_text.is_empty() {
        out.push_str(&format!("HTTP/1.1 {}\n", resp.status));
    } else {
        out.push_str(&format!("HTTP/1.1 {} {}\n", resp.status, status_text));
    }
    for (k, v) in &resp.headers {
        out.push_str(&format!("{}: {}\n", k, v));
    }
    out.push('\n');

    let mut bytes = out.into_bytes();
    bytes.extend_from_slice(body_bytes);
    bytes
}

// ─── F6 — Variable interpolation ─────────────────────────────────────────────

/// Format the current UTC time as an ISO 8601 string (`YYYY-MM-DDTHH:MM:SS.MMMZ`).
fn format_iso_timestamp() -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let total_secs = now.as_secs();
    let millis = now.subsec_millis();

    let mut days = total_secs / 86400;
    let rem = total_secs % 86400;
    let hh = rem / 3600;
    let mm = (rem % 3600) / 60;
    let ss = rem % 60;

    let mut year = 1970u32;
    loop {
        let leap =
            year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
        let yd: u64 = if leap { 366 } else { 365 };
        if days < yd {
            break;
        }
        days -= yd;
        year += 1;
    }
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let month_days: [u64; 12] = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut mon = 1u32;
    for &md in &month_days {
        if days < md {
            break;
        }
        days -= md;
        mon += 1;
    }
    let day = (days + 1) as u32;

    format!("{year:04}-{mon:02}-{day:02}T{hh:02}:{mm:02}:{ss:02}.{millis:03}Z")
}

/// Split `s` on commas that are not inside parentheses or quotes.
///
/// This allows helper args to contain nested calls and quoted strings with
/// commas, e.g. `$concat(a, ",", b)` or `$base64($concat(u, ":", p))`.
fn split_args_balanced(s: &str) -> Vec<&str> {
    let mut args = Vec::new();
    let mut depth = 0usize;
    let mut in_dq = false;
    let mut in_sq = false;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '"' if !in_sq => in_dq = !in_dq,
            '\'' if !in_dq => in_sq = !in_sq,
            '(' if !in_dq && !in_sq => depth += 1,
            ')' if !in_dq && !in_sq => depth = depth.saturating_sub(1),
            ',' if depth == 0 && !in_dq && !in_sq => {
                args.push(&s[start..i]);
                start = i + 1;
            },
            _ => {},
        }
    }
    args.push(&s[start..]);
    args
}

/// Resolve a `{{$fnName(arg, ...)}}` helper-function call.
///
/// Each argument is one of:
/// - a quoted literal: `"foo"` or `'foo'`
/// - a nested helper call: `$fn(…)` — resolved recursively
/// - an env key: looked up in `env`, falls back to the raw token
///
/// Returns `Some(value)` for known functions, `None` otherwise (so the caller
/// can fall through to [`resolve_dynamic_var`]).
///
/// Supported functions:
/// - `$base64(value)`               — standard base64-encode a single value
///   Usage: `Authorization: Basic {{$base64($concat(user, ":", pass))}}`
/// - `$base64url(value)`            — URL-safe base64, no padding
/// - `$base64decode(value)`         — decode a base64 string to UTF-8
/// - `$basicAuth(user, pass)`       — `Basic <base64(user:pass)>` (full header value)
///   Usage: `Authorization: {{$basicAuth(username, password)}}`
/// - `$urlEncode(value)`            — percent-encodes all non-alphanumeric chars
/// - `$sha256(value)`               — SHA-256 hex digest
/// - `$md5(value)`                  — MD5 hex digest
/// - `$hmacSha256(secret, message)` — HMAC-SHA256 hex digest
/// - `$concat(a, b, …)`            — concatenates args with no separator
/// - `$toUpper(value)`              — uppercase
/// - `$toLower(value)`              — lowercase
fn resolve_helper_fn(key: &str, env: &HashMap<String, String>) -> Option<String> {
    let rest = key.strip_prefix('$')?;
    let paren = rest.find('(')?;
    let fn_name = &rest[..paren];
    let args_str = rest[paren + 1..].strip_suffix(')')?;

    let resolve_arg = |arg: &str| -> String {
        let arg = arg.trim();
        if (arg.starts_with('"') && arg.ends_with('"'))
            || (arg.starts_with('\'') && arg.ends_with('\''))
        {
            arg[1..arg.len() - 1].to_string()
        } else if arg.starts_with('$') && arg.contains('(') {
            resolve_helper_fn(arg, env).unwrap_or_else(|| arg.to_string())
        } else {
            env.get(arg).cloned().unwrap_or_else(|| arg.to_string())
        }
    };

    let args: Vec<String> = if args_str.trim().is_empty() {
        vec![]
    } else {
        split_args_balanced(args_str).iter().map(|s| resolve_arg(s)).collect()
    };

    match fn_name {
        "base64" => {
            use base64::Engine as _;
            let input = args.into_iter().next().unwrap_or_default();
            Some(base64::engine::general_purpose::STANDARD.encode(input.as_bytes()))
        },
        "base64url" => {
            use base64::Engine as _;
            let input = args.into_iter().next().unwrap_or_default();
            Some(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(input.as_bytes()))
        },
        "base64decode" => {
            use base64::Engine as _;
            let input = args.into_iter().next().unwrap_or_default();
            let bytes = base64::engine::general_purpose::STANDARD.decode(input.as_bytes()).ok()?;
            Some(String::from_utf8_lossy(&bytes).into_owned())
        },
        "basicAuth" => {
            use base64::Engine as _;
            let encoded =
                base64::engine::general_purpose::STANDARD.encode(args.join(":").as_bytes());
            Some(format!("Basic {encoded}"))
        },
        "urlEncode" => {
            let input = args.into_iter().next().unwrap_or_default();
            Some(
                percent_encoding::utf8_percent_encode(&input, percent_encoding::NON_ALPHANUMERIC)
                    .to_string(),
            )
        },
        "sha256" => {
            use sha2::Digest as _;
            let input = args.into_iter().next().unwrap_or_default();
            let digest = sha2::Sha256::digest(input.as_bytes());
            Some(digest.iter().map(|b| format!("{b:02x}")).collect())
        },
        "md5" => {
            use md5::Digest as _;
            let input = args.into_iter().next().unwrap_or_default();
            let digest = md5::Md5::digest(input.as_bytes());
            Some(digest.iter().map(|b| format!("{b:02x}")).collect())
        },
        "hmacSha256" => {
            use hmac::Mac as _;
            if args.len() < 2 {
                return None;
            }
            let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(args[0].as_bytes()).ok()?;
            mac.update(args[1].as_bytes());
            let digest = mac.finalize().into_bytes();
            Some(digest.iter().map(|b| format!("{b:02x}")).collect())
        },
        "concat" => Some(args.join("")),
        "toUpper" => Some(args.into_iter().next().unwrap_or_default().to_uppercase()),
        "toLower" => Some(args.into_iter().next().unwrap_or_default().to_lowercase()),
        _ => None,
    }
}

/// Resolve a Postman-style dynamic variable (key starts with `$`).
///
/// Returns `Some(value)` for known variables, `None` for unknown ones.
fn resolve_dynamic_var(key: &str) -> Option<String> {
    use rand::Rng as _;
    match key {
        "$guid" | "$randomUUID" => Some(uuid::Uuid::new_v4().to_string()),
        "$timestamp" => {
            let secs = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
            Some(secs.to_string())
        },
        "$isoTimestamp" => Some(format_iso_timestamp()),
        "$randomInt" => Some(rand::thread_rng().gen_range(0u32..=1000).to_string()),
        "$randomFloat" => Some(format!("{:.6}", rand::random::<f64>())),
        "$randomBoolean" => Some(rand::random::<bool>().to_string()),
        _ => None,
    }
}

/// Replace every `{{key}}` placeholder in `template` with the corresponding
/// value from `env`. `{{$name}}` placeholders are resolved as Postman-style
/// dynamic variables first; unknown placeholders are left unchanged.
pub fn interpolate(template: &str, env: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find("{{") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 2..];
        if let Some(close) = after.find("}}") {
            let key = &after[..close];
            if let Some(val) = resolve_helper_fn(key, env) {
                out.push_str(&val);
            } else if let Some(val) = resolve_dynamic_var(key) {
                out.push_str(&val);
            } else if let Some(val) = env.get(key) {
                out.push_str(val);
            } else {
                out.push_str("{{");
                out.push_str(key);
                out.push_str("}}");
            }
            rest = &after[close + 2..];
        } else {
            // Unmatched `{{` — emit as-is and stop scanning.
            out.push_str("{{");
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

fn interpolate_request(req: HttpRequest, env: &HashMap<String, String>) -> HttpRequest {
    if env.is_empty() {
        return req;
    }
    HttpRequest {
        url: interpolate(&req.url, env),
        method: req.method,
        headers: req.headers.into_iter().map(|(k, v)| (k, interpolate(&v, env))).collect(),
        body: req.body.map(|b| interpolate(&b, env)),
    }
}

// ─── F8 — Security profiles ───────────────────────────────────────────────────

/// Pre-built [`RunCapabilities`] profiles for common HTTP testing scenarios.
pub struct SecurityProfile;

impl SecurityProfile {
    /// Public API testing — fetches allowed only to `base_url` prefix.
    pub fn public_api(base_url: impl Into<String>) -> RunCapabilities {
        RunCapabilities {
            http_allowed_prefixes: Some(vec![base_url.into()]),
            http_calls_limit: Some(20),
            kv_ops_limit: Some(50),
            ..Default::default()
        }
    }

    /// Auth flow — restricted emit event names.
    pub fn auth_flow(base_url: impl Into<String>) -> RunCapabilities {
        RunCapabilities {
            http_allowed_prefixes: Some(vec![base_url.into()]),
            http_calls_limit: Some(5),
            kv_ops_limit: Some(20),
            emit_allowed_names: Some(vec![
                "test_pass".into(),
                "test_fail".into(),
                "test_failure".into(),
                "token_extracted".into(),
            ]),
            ..Default::default()
        }
    }

    /// Sensitive endpoint — tight KV namespace isolated to `test_id`.
    pub fn sensitive(base_url: impl Into<String>, test_id: impl Into<String>) -> RunCapabilities {
        RunCapabilities {
            http_allowed_prefixes: Some(vec![base_url.into()]),
            kv_key_prefix: Some(format!("secure:{}:", test_id.into())),
            http_calls_limit: Some(3),
            kv_ops_limit: Some(10),
            emit_calls_limit: Some(10),
            ..Default::default()
        }
    }

    /// Untrusted user-provided script — HTTP disabled, tight timeout.
    pub fn user_script(timeout: Duration) -> RunCapabilities {
        RunCapabilities {
            http_enabled: Some(false),
            timeout_override: Some(timeout),
            kv_ops_limit: Some(5),
            emit_calls_limit: Some(5),
            ..Default::default()
        }
    }
}

// ─── Builder ──────────────────────────────────────────────────────────────────

/// Fluent builder for [`HttpTestRunner`].
pub struct HttpTestRunnerBuilder {
    pool_config: PoolConfig,
    http_config: HttpConfig,
    env: HashMap<String, String>,
    history: Option<Box<dyn HistorySink>>,
    collection_pre_script: Option<(String, Vec<(String, String)>)>,
    collection_post_script: Option<(String, Vec<(String, String)>)>,
}

impl HttpTestRunnerBuilder {
    fn new() -> Self {
        Self {
            pool_config: PoolConfig {
                pool_size: 1,
                ..Default::default()
            },
            http_config: HttpConfig::default(),
            env: HashMap::new(),
            history: None,
            collection_pre_script: None,
            collection_post_script: None,
        }
    }

    /// Set the pool configuration.
    pub fn pool(mut self, pool_config: PoolConfig) -> Self {
        self.pool_config = pool_config;
        self
    }

    /// Set the pack-level URL prefix allowlist for HTTP fetches.
    pub fn allowed_prefixes(mut self, prefixes: Vec<String>) -> Self {
        self.http_config.allowed_prefixes = prefixes;
        self
    }

    /// Set the per-request timeout for HTTP fetches.
    pub fn http_timeout(mut self, timeout: Duration) -> Self {
        self.http_config.timeout = timeout;
        self
    }

    /// Pre-populate a template variable for `{{key}}` interpolation.
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    /// Attach a history sink — called after every [`HttpTestRunner::run_test`].
    pub fn history(mut self, sink: impl HistorySink) -> Self {
        self.history = Some(Box::new(sink));
        self
    }

    /// Set a script (source + dep modules) that runs before each request's
    /// own pre-script. See [`HttpTestRunner::set_collection_pre_script`].
    pub fn collection_pre_script(
        mut self,
        src: impl Into<String>,
        modules: Vec<(String, String)>,
    ) -> Self {
        self.collection_pre_script = Some((src.into(), modules));
        self
    }

    /// Set a script (source + dep modules) that runs after each request's
    /// own post-script. See [`HttpTestRunner::set_collection_post_script`].
    pub fn collection_post_script(
        mut self,
        src: impl Into<String>,
        modules: Vec<(String, String)>,
    ) -> Self {
        self.collection_post_script = Some((src.into(), modules));
        self
    }

    /// Build the [`HttpTestRunner`].
    pub fn build(self) -> Result<HttpTestRunner, SandboxError> {
        let sandbox = Sandbox::builder()
            .config(SandboxConfig::power_user())
            .pool(self.pool_config)
            .sdk(KvPack::default())
            .sdk(HttpPack::new(self.http_config))
            .sdk(AssertPack)
            .sdk(PmPack)
            .build()?;
        let mut runner = HttpTestRunner::new(sandbox);
        runner.env = self.env;
        runner.history = self.history;
        if let Some((src, mods)) = self.collection_pre_script {
            runner.set_collection_pre_script(src, mods);
        }
        if let Some((src, mods)) = self.collection_post_script {
            runner.set_collection_post_script(src, mods);
        }
        Ok(runner)
    }
}

// ─── Runner ───────────────────────────────────────────────────────────────────

/// HTTP test runner — wraps a [`Sandbox`] with three-phase test execution.
///
/// **Must be used on a `tokio::task::LocalSet`.**
pub struct HttpTestRunner {
    sandbox: Sandbox,
    /// Template variables for `{{key}}` substitution in request URLs/headers.
    pub env: HashMap<String, String>,
    /// Optional sink that receives every [`TestResult`] for persistence (F7).
    history: Option<Box<dyn HistorySink>>,
    /// Script that runs **before** the per-request pre-script on every test.
    ///
    /// Can return `{ url, method, headers, body }` to override the request.
    /// Set via [`HttpTestRunnerBuilder::collection_pre_script`] or
    /// [`HttpTestRunner::set_collection_pre_script`].
    collection_pre_script: Option<String>,
    /// Modules required by [`collection_pre_script`] (registered once per run).
    collection_pre_modules: Vec<(String, String)>,
    /// Script that runs **after** the per-request post-script on every test.
    ///
    /// Returns `{ pass, failures }` — ANDed with the per-request result.
    collection_post_script: Option<String>,
    /// Modules required by [`collection_post_script`] (registered once per run).
    collection_post_modules: Vec<(String, String)>,
    /// pm.environment state threaded between test cases (S4).
    ///
    /// Captured from `results()` after each post-script and re-injected as
    /// `_pm_env` before the next script run so `pm.environment.get/set` values
    /// persist across tests in a collection run.  Reset at `run_collection` start.
    pm_env: Value,
    /// pm.collectionVariables state (S4).
    pm_col_vars: Value,
    /// pm.globals state (S4). Not reset between collection runs by design.
    pm_globals: Value,
}

impl HttpTestRunner {
    /// Create an `HttpTestRunner` from a pre-built [`Sandbox`].
    ///
    /// The sandbox must have [`HttpPack`] registered. Registers `sandbox:test`
    /// automatically.  For `sandbox:assert` (F9) the sandbox must also have
    /// [`AssertPack`] in its SDK registry — use [`HttpTestRunner::builder`] to
    /// get this wired automatically.
    pub fn new(mut sandbox: Sandbox) -> Self {
        sandbox.register_module("sandbox:test", TEST_JS);
        sandbox.register_module("sandbox:test.d.ts", TEST_DTS);
        Self {
            sandbox,
            env: HashMap::new(),
            history: None,
            collection_pre_script: None,
            collection_pre_modules: vec![],
            collection_post_script: None,
            collection_post_modules: vec![],
            pm_env: Value::Object(Default::default()),
            pm_col_vars: Value::Object(Default::default()),
            pm_globals: Value::Object(Default::default()),
        }
    }

    /// Attach a history sink after construction.
    pub fn set_history(&mut self, sink: impl HistorySink) {
        self.history = Some(Box::new(sink));
    }

    /// Set a script that runs before the per-request pre-script on every test.
    ///
    /// `modules` is a list of `(specifier, source)` pairs for any `sandbox:`
    /// modules the script imports (populated by `runner::load_script_with_deps`).
    pub fn set_collection_pre_script(
        &mut self,
        src: impl Into<String>,
        modules: Vec<(String, String)>,
    ) {
        self.collection_pre_script = Some(src.into());
        self.collection_pre_modules = modules;
    }

    /// Set a script that runs after the per-request post-script on every test.
    pub fn set_collection_post_script(
        &mut self,
        src: impl Into<String>,
        modules: Vec<(String, String)>,
    ) {
        self.collection_post_script = Some(src.into());
        self.collection_post_modules = modules;
    }

    /// Convenience builder.
    pub fn builder() -> HttpTestRunnerBuilder {
        HttpTestRunnerBuilder::new()
    }

    /// Set a template variable for `{{key}}` interpolation.
    pub fn set_env(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.env.insert(key.into(), value.into());
    }

    /// Get the current value of a template variable.
    pub fn get_env(&self, key: &str) -> Option<&str> {
        self.env.get(key).map(|s| s.as_str())
    }

    /// Remove a template variable.
    pub fn remove_env(&mut self, key: &str) {
        self.env.remove(key);
    }

    /// Inject the current pm environment state into the sandbox before a script run (S4).
    fn inject_pm_state(&mut self) {
        self.sandbox.set_input("_pm_env", self.pm_env.clone());
        self.sandbox.set_input("_pm_col_vars", self.pm_col_vars.clone());
        self.sandbox.set_input("_pm_globals", self.pm_globals.clone());
    }

    /// Capture pm environment state from a post-script result into the runner (S4).
    fn capture_pm_state(&mut self, pr: &PostScriptResults) {
        if let Some(env) = &pr.pm_env {
            self.pm_env = env.clone();
        }
        if let Some(cv) = &pr.pm_col_vars {
            self.pm_col_vars = cv.clone();
        }
        if let Some(g) = &pr.pm_globals {
            self.pm_globals = g.clone();
        }
    }

    /// Execute one test case (pre → fetch → post).
    pub async fn run_test(&mut self, mut test: TestCase) -> Result<TestResult, SandboxError> {
        // Register any user-module deps discovered by load_script_with_deps.
        for (spec, src) in std::mem::take(&mut test.modules) {
            self.sandbox.register_module(spec, src);
        }

        // Register collection-script dep modules (idempotent across warm slots).
        for (spec, src) in &self.collection_pre_modules.clone() {
            self.sandbox.register_module(spec.clone(), src.clone());
        }
        for (spec, src) in &self.collection_post_modules.clone() {
            self.sandbox.register_module(spec.clone(), src.clone());
        }

        let mut effective_request =
            interpolate_request(std::mem::take(&mut test.request), &self.env);
        let mut all_logs: Vec<String> = Vec::new();
        let mut all_events: Vec<SandboxEvent> = Vec::new();
        let mut phase_timings = PhaseTimings::default();

        // Phase 0: collection pre-script (runs before per-request pre-script)
        if let Some(ref pre_script) = self.collection_pre_script.clone() {
            self.sandbox.set_input(
                "_request",
                serde_json::to_value(&effective_request)
                    .map_err(|e| SandboxError::Runtime(anyhow::anyhow!(e)))?,
            );
            self.inject_pm_state();
            let t = Instant::now();
            let result =
                self.sandbox.run_with_caps(pre_script, make_caps(&test, "collection-pre")).await?;
            phase_timings.collection_pre_ms = Some(t.elapsed().as_millis() as u64);
            all_logs.extend(result.logs);
            all_events.extend(result.events);
            merge_request(&mut effective_request, &result.value);
            effective_request = interpolate_request(effective_request, &self.env);
        }

        // Phase 1: pre_script
        if let Some(pre_script) = &test.pre_script {
            self.sandbox.set_input(
                "_request",
                serde_json::to_value(&effective_request)
                    .map_err(|e| SandboxError::Runtime(anyhow::anyhow!(e)))?,
            );
            self.inject_pm_state();
            let t = Instant::now();
            let result = self.sandbox.run_with_caps(pre_script, make_caps(&test, "pre")).await?;
            phase_timings.pre_ms = Some(t.elapsed().as_millis() as u64);
            all_logs.extend(result.logs);
            all_events.extend(result.events);
            merge_request(&mut effective_request, &result.value);
            effective_request = interpolate_request(effective_request, &self.env);
        }

        // Phase 2: fetch (or load body from response_file when set)
        let (body_bytes, http_response, mut last_metrics) = if let Some(ref path) =
            test.response_file
        {
            // Skip HTTP fetch — parse body from a previously saved HTTP message file.
            let bytes = std::fs::read(path).map_err(|e| {
                SandboxError::Runtime(anyhow::anyhow!(
                    "response_file: failed to read {:?}: {}",
                    path,
                    e
                ))
            })?;
            let (status, ok, headers, body_bytes) = parse_http_message(&bytes);
            let body = String::from_utf8_lossy(&body_bytes).into_owned();
            let resp = HttpResponse {
                status,
                ok,
                headers,
                body,
                response_time_ms: 0,
                redirected: false,
            };
            phase_timings.fetch_ms = Some(0);
            (body_bytes, resp, RunMetrics::default())
        } else {
            self.sandbox.set_input(
                "_request",
                serde_json::to_value(&effective_request)
                    .map_err(|e| SandboxError::Runtime(anyhow::anyhow!(e)))?,
            );
            let fetch_result =
                self.sandbox.run_with_caps(FETCH_SCRIPT, make_caps(&test, "fetch")).await?;
            all_logs.extend(fetch_result.logs);
            all_events.extend(fetch_result.events);
            let metrics = fetch_result.metrics;

            let raw: FetchScriptResult =
                serde_json::from_value(fetch_result.value).map_err(|e| {
                    SandboxError::Runtime(anyhow::anyhow!("failed to parse fetch result: {}", e))
                })?;

            use base64::Engine as _;
            let bytes =
                base64::engine::general_purpose::STANDARD.decode(&raw.body_b64).map_err(|e| {
                    SandboxError::Runtime(anyhow::anyhow!("base64 decode failed: {}", e))
                })?;
            let body = String::from_utf8_lossy(&bytes).into_owned();
            let resp = HttpResponse {
                status: raw.status,
                ok: raw.ok,
                headers: raw.headers,
                body,
                response_time_ms: raw.response_time_ms,
                redirected: raw.redirected,
            };
            phase_timings.fetch_ms = Some(raw.response_time_ms);
            (bytes, resp, metrics)
        };

        // Write the full HTTP response (status line + headers + blank line + body) to disk.
        let output_written = if let Some(ref path) = test.output_file {
            let dump = format_http_response(&http_response, &body_bytes);
            match std::fs::write(path, dump) {
                Ok(()) => Some(path.clone()),
                Err(e) => {
                    eprintln!("warning: failed to write output file {:?}: {}", path, e);
                    None
                },
            }
        } else {
            None
        };

        // Phase 3: post_script
        let mut passed = true;
        let mut failures = Vec::new();
        let mut visualizer_html: Option<String> = None;

        if let Some(post_script) = &test.post_script {
            // Prepend auto-import prelude for sandbox:test exports not already imported.
            let prelude = build_prelude(post_script);
            let post_source = if prelude.is_empty() {
                post_script.clone()
            } else {
                format!("{prelude}{post_script}")
            };
            let post_source = append_results_if_missing(post_source);

            self.sandbox.set_input(
                "_request",
                serde_json::to_value(&effective_request)
                    .map_err(|e| SandboxError::Runtime(anyhow::anyhow!(e)))?,
            );
            self.sandbox.set_input(
                "_response",
                serde_json::to_value(&http_response)
                    .map_err(|e| SandboxError::Runtime(anyhow::anyhow!(e)))?,
            );
            self.inject_pm_state();
            let t = Instant::now();
            let post_result =
                self.sandbox.run_with_caps(&post_source, make_caps(&test, "post")).await?;
            phase_timings.post_ms = Some(t.elapsed().as_millis() as u64);
            all_logs.extend(post_result.logs);
            all_events.extend(post_result.events);
            last_metrics = post_result.metrics;

            if let Ok(pr) = serde_json::from_value::<PostScriptResults>(post_result.value) {
                self.capture_pm_state(&pr);
                passed = pr.pass;
                failures = pr.failures;
                if let Some(viz) = pr.visualizer {
                    visualizer_html =
                        Some(render_visualizer_html(&test.name, &viz.template, &viz.data));
                }
            }
        }

        // Phase 4: collection post-script (runs after per-request post-script)
        if let Some(ref post_script) = self.collection_post_script.clone() {
            let prelude = build_prelude(post_script);
            let post_source = if prelude.is_empty() {
                post_script.clone()
            } else {
                format!("{prelude}{post_script}")
            };
            let post_source = append_results_if_missing(post_source);
            self.sandbox.set_input(
                "_request",
                serde_json::to_value(&effective_request)
                    .map_err(|e| SandboxError::Runtime(anyhow::anyhow!(e)))?,
            );
            self.sandbox.set_input(
                "_response",
                serde_json::to_value(&http_response)
                    .map_err(|e| SandboxError::Runtime(anyhow::anyhow!(e)))?,
            );
            self.inject_pm_state();
            let t = Instant::now();
            let col_result = self
                .sandbox
                .run_with_caps(&post_source, make_caps(&test, "collection-post"))
                .await?;
            phase_timings.collection_post_ms = Some(t.elapsed().as_millis() as u64);
            all_logs.extend(col_result.logs);
            all_events.extend(col_result.events);
            last_metrics = col_result.metrics;

            if let Ok(pr) = serde_json::from_value::<PostScriptResults>(col_result.value) {
                self.capture_pm_state(&pr);
                if !pr.pass {
                    passed = false;
                }
                failures.extend(pr.failures);
                visualizer_html = visualizer_html.or_else(|| {
                    pr.visualizer
                        .map(|viz| render_visualizer_html(&test.name, &viz.template, &viz.data))
                });
            }
        }

        let result = TestResult {
            name: test.name,
            passed,
            failures,
            request: effective_request,
            response: Some(http_response),
            logs: all_logs,
            events: all_events,
            metrics: last_metrics,
            phase_timings,
            visualizer_html,
            output_written,
        };

        if let Some(sink) = &self.history {
            sink.record(&result);
        }

        Ok(result)
    }

    /// Reset `pm.environment` and `pm.collectionVariables` state to empty.
    ///
    /// Called automatically at the start of each [`run_collection`] call so
    /// environment variables from one collection run do not bleed into the next.
    /// `pm.globals` is intentionally NOT reset here; call this method manually
    /// before a new run if you want a fully clean state.
    pub fn reset_pm_state(&mut self) {
        self.pm_env = Value::Object(Default::default());
        self.pm_col_vars = Value::Object(Default::default());
    }

    /// Run multiple test cases sequentially, sharing KV state across all of them.
    pub async fn run_collection(
        &mut self,
        tests: Vec<TestCase>,
    ) -> Result<CollectionResult, SandboxError> {
        self.reset_pm_state();
        let start = Instant::now();
        let mut results = Vec::new();
        let mut passed = 0usize;
        let mut failed = 0usize;

        for test in tests {
            let result = self.run_test(test).await?;
            if result.passed {
                passed += 1;
            } else {
                failed += 1;
            }
            results.push(result);
        }

        Ok(CollectionResult {
            passed,
            failed,
            results,
            total_duration: start.elapsed(),
        })
    }
}
