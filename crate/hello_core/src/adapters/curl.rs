//! Curl import/export adapter.
//!
//! [`CurlAdapter::import`] parses a `curl` command string into a [`TestCase`].
//! [`CurlAdapter::export`] serializes a [`TestCase`] back to a `curl` command.
//! [`CurlAdapter::to_http`] is a convenience wrapper that converts a `curl`
//! command directly to `.http` entry format (used by the `from-curl` CLI
//! subcommand).

use base64::Engine as _;
use clap::Parser;

use crate::types::{HttpRequest, TestCase};

// ─── Error ────────────────────────────────────────────────────────────────────

/// Errors returned by [`CurlAdapter`].
#[derive(Debug, thiserror::Error)]
pub enum CurlError {
    /// The input string had no tokens.
    #[error("empty curl command")]
    Empty,

    /// Parsing succeeded but no URL could be found.
    #[error("no URL found in curl command")]
    NoUrl,
}

// ─── Shell tokenizer ──────────────────────────────────────────────────────────

/// Tokenize a shell-like string respecting quoting and `\` line continuations.
///
/// Handles:
/// - Single-quoted strings: no escaping inside
/// - Double-quoted strings: `\` escapes the following character
/// - Bare `\` followed by `\n`: line continuation (collapsed to space)
/// - Bare `\` followed by any other char: that character, unescaped
pub(crate) fn tokenize_shell(input: &str) -> Vec<String> {
    let input = input.replace("\\\n", " ");
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                for c2 in chars.by_ref() {
                    if c2 == '\'' {
                        break;
                    }
                    current.push(c2);
                }
            },
            '"' => {
                while let Some(c2) = chars.next() {
                    if c2 == '"' {
                        break;
                    }
                    if c2 == '\\' {
                        if let Some(c3) = chars.next() {
                            current.push(c3);
                        }
                    } else {
                        current.push(c2);
                    }
                }
            },
            '\\' => {
                if let Some(c2) = chars.next()
                    && c2 != '\n'
                {
                    current.push(c2);
                }
            },
            c if c.is_whitespace() => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            },
            _ => current.push(c),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

// ─── Clap-based argument parser ───────────────────────────────────────────────

/// Clap struct that mirrors curl's common flags.
///
/// `ignore_errors = true` makes clap return partial results rather than hard-
/// failing on any unrecognised flag, so unusual or future curl flags don't
/// break parsing.
#[derive(Parser, Debug, Default)]
#[command(
    name = "curl",
    disable_help_flag = true,
    disable_version_flag = true,
    ignore_errors = true
)]
struct CurlArgs {
    // ── Request control ──────────────────────────────────────────────────────
    /// HTTP method (-X / --request)
    #[arg(short = 'X', long = "request", value_name = "METHOD")]
    method: Option<String>,

    /// Request header, repeatable (-H / --header)
    #[arg(short = 'H', long = "header", value_name = "HEADER", action = clap::ArgAction::Append)]
    header: Vec<String>,

    /// Request body (-d / --data and common aliases)
    #[arg(
        short = 'd',
        long = "data",
        aliases = ["data-raw", "data-binary", "data-ascii"],
        value_name = "DATA"
    )]
    data: Option<String>,

    /// URL-encoded body data, repeatable
    #[arg(long = "data-urlencode", value_name = "DATA", action = clap::ArgAction::Append)]
    data_urlencode: Vec<String>,

    /// JSON body — implies POST, Content-Type: application/json, Accept: application/json (curl 7.82+)
    #[arg(long = "json", value_name = "DATA")]
    json_body: Option<String>,

    /// Multipart form field: name=value or name=@file (repeatable)
    #[arg(short = 'F', long = "form", value_name = "NAME=VALUE", action = clap::ArgAction::Append)]
    form: Vec<String>,

    // ── Auth / convenience headers ───────────────────────────────────────────
    /// Basic-auth credentials user:password (-u / --user)
    #[arg(short = 'u', long = "user", value_name = "USER:PASS")]
    user: Option<String>,

    /// User-Agent header (-A / --user-agent)
    #[arg(short = 'A', long = "user-agent", value_name = "AGENT")]
    user_agent: Option<String>,

    /// Referer header (-e / --referer)
    #[arg(short = 'e', long = "referer", value_name = "URL")]
    referer: Option<String>,

    /// Cookie header (-b / --cookie)
    #[arg(short = 'b', long = "cookie", value_name = "COOKIE")]
    cookie: Option<String>,

    // ── URL ──────────────────────────────────────────────────────────────────
    /// Explicit URL flag (--url)
    #[arg(long = "url", value_name = "URL")]
    url_flag: Option<String>,

    // ── Value-consuming ignored flags ────────────────────────────────────────
    #[arg(short = 'o', long = "output", value_name = "FILE")]
    output: Option<String>,

    #[arg(short = 'w', long = "write-out", value_name = "FORMAT")]
    write_out: Option<String>,

    #[arg(short = 'm', long = "max-time", value_name = "SECS")]
    max_time: Option<String>,

    #[arg(long = "connect-timeout", value_name = "SECS")]
    connect_timeout: Option<String>,

    #[arg(long = "retry", value_name = "NUM")]
    retry: Option<String>,

    #[arg(long = "retry-delay", value_name = "SECS")]
    retry_delay: Option<String>,

    #[arg(long = "retry-max-time", value_name = "SECS")]
    retry_max_time: Option<String>,

    #[arg(long = "limit-rate", value_name = "RATE")]
    limit_rate: Option<String>,

    #[arg(short = 'x', long = "proxy", value_name = "URL")]
    proxy: Option<String>,

    #[arg(long = "cacert", value_name = "FILE")]
    cacert: Option<String>,

    #[arg(long = "capath", value_name = "DIR")]
    capath: Option<String>,

    #[arg(short = 'E', long = "cert", value_name = "CERT")]
    cert: Option<String>,

    #[arg(long = "key", value_name = "FILE")]
    key: Option<String>,

    #[arg(long = "pass", value_name = "PHRASE")]
    pass: Option<String>,

    #[arg(long = "trace", value_name = "FILE")]
    trace: Option<String>,

    #[arg(long = "trace-ascii", value_name = "FILE")]
    trace_ascii: Option<String>,

    #[arg(long = "keepalive-time", value_name = "SECS")]
    keepalive_time: Option<String>,

    #[arg(long = "resolve", value_name = "HOST:PORT:ADDR")]
    resolve: Option<String>,

    #[arg(long = "dns-servers", value_name = "SERVERS")]
    dns_servers: Option<String>,

    #[arg(long = "interface", value_name = "NAME")]
    interface: Option<String>,

    // ── Boolean ignored flags ────────────────────────────────────────────────
    #[arg(short = 'L', long = "location")]
    location: bool,

    #[arg(long = "compressed")]
    compressed: bool,

    #[arg(short = 'k', long = "insecure")]
    insecure: bool,

    #[arg(short = 's', long = "silent")]
    silent: bool,

    #[arg(short = 'S', long = "show-error")]
    show_error: bool,

    #[arg(short = 'v', long = "verbose")]
    verbose: bool,

    #[arg(short = 'i', long = "include")]
    include: bool,

    #[arg(short = 'I', long = "head")]
    head: bool,

    #[arg(short = 'g', long = "globoff")]
    globoff: bool,

    #[arg(long = "no-keepalive")]
    no_keepalive: bool,

    #[arg(long = "no-progress-meter")]
    no_progress_meter: bool,

    #[arg(long = "no-buffer")]
    no_buffer: bool,

    #[arg(long = "fail")]
    fail: bool,

    #[arg(long = "fail-early")]
    fail_early: bool,

    #[arg(long = "tcp-nodelay")]
    tcp_nodelay: bool,

    // HTTP version flags (dots in long names are valid in clap string attrs)
    #[arg(long = "http1.0")]
    http10: bool,

    #[arg(long = "http1.1")]
    http11: bool,

    #[arg(long = "http2")]
    http2: bool,

    #[arg(long = "http3")]
    http3: bool,

    // ── Positional URL ───────────────────────────────────────────────────────
    /// URL positional argument(s) — first non-flag token wins
    #[arg(value_name = "URL")]
    url_positional: Vec<String>,
}

// ─── Internals ────────────────────────────────────────────────────────────────

/// Shell-quote `s` with single quotes, escaping embedded `'` as `'\''`.
fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    // Fast path: no single-quotes → wrap directly.
    if !s.contains('\'') {
        return format!("'{s}'");
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Build the resolved headers list from parsed [`CurlArgs`].
fn build_headers(args: &CurlArgs) -> Vec<(String, String)> {
    let mut headers: Vec<(String, String)> = Vec::new();

    for raw in &args.header {
        if let Some((k, v)) = raw.split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }

    if let Some(creds) = &args.user {
        let encoded = base64::engine::general_purpose::STANDARD.encode(creds.as_bytes());
        headers.push(("Authorization".to_string(), format!("Basic {encoded}")));
    }

    if let Some(agent) = &args.user_agent {
        headers.push(("User-Agent".to_string(), agent.clone()));
    }
    if let Some(ref_url) = &args.referer {
        headers.push(("Referer".to_string(), ref_url.clone()));
    }
    if let Some(cookie) = &args.cookie {
        headers.push(("Cookie".to_string(), cookie.clone()));
    }

    headers
}

/// Resolve body from args. Priority: `--data` > `--json` > `--data-urlencode` > `--form`.
///
/// `--data @file` and `--json @file` (curl `@filename` convention) are converted to
/// the `.http` `< path` file-reference syntax so the runner can load them at run time.
/// `--form name=@file` is stored as `name=<file:path>` as a placeholder.
fn build_body(args: &CurlArgs) -> (Option<String>, bool) {
    if let Some(d) = &args.data {
        let body = if let Some(path) = d.strip_prefix('@') {
            format!("< {}", path.trim())
        } else {
            d.clone()
        };
        return (Some(body), false);
    }

    if let Some(j) = &args.json_body {
        let body = if let Some(path) = j.strip_prefix('@') {
            format!("< {}", path.trim())
        } else {
            j.clone()
        };
        return (Some(body), true); // `true` = came from --json
    }

    if !args.data_urlencode.is_empty() {
        return (Some(args.data_urlencode.join("&")), false);
    }

    if !args.form.is_empty() {
        let parts: Vec<String> = args
            .form
            .iter()
            .map(|entry| {
                if let Some((k, v)) = entry.split_once('=') {
                    if let Some(path) = v.strip_prefix('@') {
                        format!("{}=<file:{}>", k, path)
                    } else {
                        entry.clone()
                    }
                } else {
                    entry.clone()
                }
            })
            .collect();
        return (Some(parts.join("&")), false);
    }

    (None, false)
}

/// Auto-inject `Content-Type: application/json` when body looks like JSON and no CT header is set.
fn maybe_inject_content_type(headers: &mut Vec<(String, String)>, body: &Option<String>) {
    if let Some(b) = body {
        let has_ct = headers.iter().any(|(k, _)| k.to_lowercase() == "content-type");
        if !has_ct {
            let trimmed = b.trim_start();
            if trimmed.starts_with('{') || trimmed.starts_with('[') {
                headers.push(("Content-Type".to_string(), "application/json".to_string()));
            }
        }
    }
}

/// (method, url, headers, body) returned by [`parse_tokens`].
type ParsedCurl = (String, String, Vec<(String, String)>, Option<String>);

/// Parse a tokenised curl command into its components.
fn parse_tokens(tokens: &[String]) -> Result<ParsedCurl, CurlError> {
    // Strip leading "curl" token for clap.
    let args_slice: &[String] = if tokens.first().map(|s| s.as_str()) == Some("curl") {
        &tokens[1..]
    } else {
        tokens
    };

    // Build an iterator starting with the program name clap expects.
    let iter = std::iter::once("curl".to_string()).chain(args_slice.iter().cloned());
    let args = CurlArgs::parse_from(iter);

    let url = args
        .url_flag
        .clone()
        .or_else(|| args.url_positional.first().cloned())
        .ok_or(CurlError::NoUrl)?;

    let mut headers = build_headers(&args);
    let (body, from_json) = build_body(&args);

    if from_json {
        // --json implies Content-Type and Accept: application/json
        if !headers.iter().any(|(k, _)| k.to_lowercase() == "content-type") {
            headers.push(("Content-Type".to_string(), "application/json".to_string()));
        }
        if !headers.iter().any(|(k, _)| k.to_lowercase() == "accept") {
            headers.push(("Accept".to_string(), "application/json".to_string()));
        }
    } else if !args.form.is_empty() {
        if !headers.iter().any(|(k, _)| k.to_lowercase() == "content-type") {
            headers.push(("Content-Type".to_string(), "multipart/form-data".to_string()));
        }
    } else {
        maybe_inject_content_type(&mut headers, &body);
    }

    let method = args.method.clone().unwrap_or_else(|| {
        if body.is_some() || from_json {
            "POST".to_string()
        } else {
            "GET".to_string()
        }
    });

    Ok((url, method, headers, body))
}

// ─── Adapter ─────────────────────────────────────────────────────────────────

/// Curl import/export adapter.
///
/// ```
/// use hello_core::adapters::CurlAdapter;
///
/// let test = CurlAdapter::import(
///     r#"curl -X POST https://api.example.com/users -H 'Content-Type: application/json' -d '{"name":"alice"}'"#,
///     Some("Create User"),
/// ).unwrap();
/// assert_eq!(test.request.method, "POST");
///
/// let cmd = CurlAdapter::export(&test);
/// assert!(cmd.contains("POST"));
/// ```
pub struct CurlAdapter;

impl CurlAdapter {
    /// Parse a `curl` command string into a [`TestCase`].
    ///
    /// `name` overrides the entry name; when `None` the URL path is used.
    pub fn import(curl_str: &str, name: Option<&str>) -> Result<TestCase, CurlError> {
        let tokens = tokenize_shell(curl_str.trim());
        if tokens.is_empty() {
            return Err(CurlError::Empty);
        }
        let (url, method, headers, body) = parse_tokens(&tokens)?;
        let entry_name = name.map(|s| s.to_string()).unwrap_or_else(|| name_from_url(&url));
        Ok(TestCase {
            name: entry_name,
            request: HttpRequest {
                url,
                method,
                headers,
                body,
            },
            ..Default::default()
        })
    }

    /// Serialize a [`TestCase`] to a `curl` command string.
    ///
    /// Multi-header / body requests are formatted with ` \\\n  ` line
    /// continuations for readability.
    pub fn export(test: &TestCase) -> String {
        let req = &test.request;
        let mut parts: Vec<String> = Vec::new();

        if req.method != "GET" {
            parts.push(format!("-X {}", shell_quote(&req.method)));
        }

        for (k, v) in &req.headers {
            // C4: reconstruct --user from Authorization: Basic <base64>
            if k.eq_ignore_ascii_case("authorization")
                && let Some(b64) = v.strip_prefix("Basic ")
                && let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64.trim())
                && let Ok(creds) = std::str::from_utf8(&bytes)
            {
                parts.push(format!("-u {}", shell_quote(creds)));
                continue;
            }
            parts.push(format!("-H {}", shell_quote(&format!("{k}: {v}"))));
        }

        if let Some(body) = &req.body {
            parts.push(format!("-d {}", shell_quote(body)));
        }

        parts.push(shell_quote(&req.url));

        if parts.len() <= 1 {
            format!("curl {}", parts.join(" "))
        } else {
            let joined = parts.join(" \\\n  ");
            format!("curl \\\n  {joined}")
        }
    }

    /// Convert a `curl` command string to `.http` entry format.
    ///
    /// This is what the `from-curl` CLI subcommand uses to write output files.
    pub fn to_http(curl_str: &str, name: Option<&str>) -> Result<String, CurlError> {
        let test = Self::import(curl_str, name)?;
        let req = &test.request;

        let mut out = String::new();
        out.push_str(&format!("### {}\n\n", test.name));
        out.push_str(&format!("{} {}\n", req.method, req.url));
        for (k, v) in &req.headers {
            out.push_str(&format!("{k}: {v}\n"));
        }
        if let Some(body) = &req.body {
            out.push('\n');
            out.push_str(body);
            out.push('\n');
        }
        Ok(out)
    }
}

/// Derive a human-readable name from a URL path (e.g. `GET /users/profile`).
fn name_from_url(url: &str) -> String {
    // Strip scheme + host, keep path. No '/' after host → empty path.
    let path = if let Some(after) = url.find("://").map(|i| &url[i + 3..]) {
        after.find('/').map(|j| &after[j..]).unwrap_or("")
    } else {
        url
    };
    // Drop query string.
    let path = path.split('?').next().unwrap_or(path);
    if path.is_empty() || path == "/" {
        "Curl Request".to_string()
    } else {
        path.trim_start_matches('/').to_string()
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── tokenize_shell ────────────────────────────────────────────────────────

    #[test]
    fn tokenize_simple() {
        let t = tokenize_shell("curl https://example.com -H 'Auth: Bearer tok'");
        assert_eq!(t, ["curl", "https://example.com", "-H", "Auth: Bearer tok"]);
    }

    #[test]
    fn tokenize_double_quotes() {
        let t = tokenize_shell(r#"curl "https://example.com" -d "{\"key\":\"val\"}""#);
        assert_eq!(t[0], "curl");
        assert_eq!(t[1], "https://example.com");
        assert_eq!(t[3], r#"{"key":"val"}"#);
    }

    #[test]
    fn tokenize_line_continuation() {
        let t = tokenize_shell("curl https://example.com \\\n  -H 'X-Foo: bar'");
        assert_eq!(t, ["curl", "https://example.com", "-H", "X-Foo: bar"]);
    }

    // ── import ────────────────────────────────────────────────────────────────

    #[test]
    fn import_simple_get() {
        let tc = CurlAdapter::import("curl https://api.example.com/users", None).unwrap();
        assert_eq!(tc.request.method, "GET");
        assert_eq!(tc.request.url, "https://api.example.com/users");
    }

    #[test]
    fn import_post_json() {
        let tc = CurlAdapter::import(
            r#"curl -X POST https://api.example.com/users -d '{"name":"alice"}'"#,
            Some("Create User"),
        )
        .unwrap();
        assert_eq!(tc.request.method, "POST");
        assert_eq!(tc.name, "Create User");
        assert!(tc.request.body.as_deref().unwrap_or("").contains("alice"));
        assert!(tc.request.headers.iter().any(|(k, _)| k == "Content-Type"));
    }

    #[test]
    fn import_headers() {
        let tc = CurlAdapter::import(
            "curl https://api.example.com -H 'Authorization: Bearer tok' -H 'Accept: application/json'",
            None,
        )
        .unwrap();
        assert!(tc.request.headers.iter().any(|(k, v)| k == "Authorization" && v == "Bearer tok"));
        assert!(tc.request.headers.iter().any(|(k, v)| k == "Accept" && v == "application/json"));
    }

    #[test]
    fn import_basic_auth() {
        let tc =
            CurlAdapter::import("curl -u alice:password https://api.example.com", None).unwrap();
        assert!(
            tc.request.headers.iter().any(|(k, v)| k == "Authorization" && v.starts_with("Basic "))
        );
    }

    #[test]
    fn import_infers_post_from_data() {
        let tc = CurlAdapter::import("curl https://api.example.com -d 'name=alice'", None).unwrap();
        assert_eq!(tc.request.method, "POST");
    }

    #[test]
    fn import_combined_method_flag() {
        let tc =
            CurlAdapter::import("curl -XDELETE https://api.example.com/items/1", None).unwrap();
        assert_eq!(tc.request.method, "DELETE");
    }

    #[test]
    fn import_ignores_silent_and_location_flags() {
        let tc =
            CurlAdapter::import("curl -s -L --compressed https://api.example.com", None).unwrap();
        assert_eq!(tc.request.url, "https://api.example.com");
    }

    #[test]
    fn import_data_urlencode_joined() {
        let tc = CurlAdapter::import(
            "curl -X POST https://example.com --data-urlencode 'name=Alice' --data-urlencode 'city=NY'",
            None,
        )
        .unwrap();
        let body = tc.request.body.unwrap_or_default();
        assert!(body.contains("name=Alice"), "body: {body}");
        assert!(body.contains("city=NY"), "body: {body}");
    }

    #[test]
    fn import_http11_flag_does_not_break_parsing() {
        let tc = CurlAdapter::import(
            "curl --http1.1 -X GET https://api.example.com -H 'Accept: application/json'",
            None,
        )
        .unwrap();
        assert_eq!(tc.request.method, "GET");
        assert_eq!(tc.request.url, "https://api.example.com");
        assert!(tc.request.headers.iter().any(|(k, _)| k == "Accept"));
    }

    // ── to_http ───────────────────────────────────────────────────────────────

    #[test]
    fn to_http_get() {
        let out = CurlAdapter::to_http("curl https://api.example.com/users", None).unwrap();
        assert!(out.contains("GET https://api.example.com/users"));
        assert!(out.contains("### Curl Request") || out.contains("### users"));
    }

    #[test]
    fn to_http_post_json() {
        let out = CurlAdapter::to_http(
            r#"curl -X POST https://api.example.com/users -d '{"name":"alice"}'"#,
            Some("Create User"),
        )
        .unwrap();
        assert!(out.contains("POST https://api.example.com/users"));
        assert!(out.contains("Content-Type: application/json"));
        assert!(out.contains(r#"{"name":"alice"}"#));
        assert!(out.contains("### Create User"));
    }

    #[test]
    fn to_http_basic_auth() {
        let out =
            CurlAdapter::to_http("curl -u alice:password https://api.example.com", None).unwrap();
        assert!(out.contains("Authorization: Basic"));
    }

    // ── export ────────────────────────────────────────────────────────────────

    #[test]
    fn export_get() {
        let tc = CurlAdapter::import("curl https://api.example.com/users", None).unwrap();
        let cmd = CurlAdapter::export(&tc);
        assert!(cmd.starts_with("curl"), "cmd: {cmd}");
        assert!(cmd.contains("api.example.com/users"));
        assert!(!cmd.contains("-X"), "GET should not emit -X: {cmd}");
    }

    #[test]
    fn export_post_json() {
        let tc = CurlAdapter::import(
            r#"curl -X POST https://api.example.com -H 'Content-Type: application/json' -d '{"x":1}'"#,
            None,
        )
        .unwrap();
        let cmd = CurlAdapter::export(&tc);
        assert!(cmd.contains("-X 'POST'"));
        assert!(cmd.contains("-H 'Content-Type: application/json'"));
        assert!(cmd.contains(r#"-d '{"x":1}'"#));
    }

    #[test]
    fn export_shell_quote_preserves_special_chars() {
        let cmd = shell_quote("hello's \"world\"");
        assert_eq!(cmd, r#"'hello'\''s "world"'"#);
    }

    #[test]
    fn roundtrip_get() {
        let original = "curl https://api.example.com/users";
        let tc = CurlAdapter::import(original, None).unwrap();
        let cmd = CurlAdapter::export(&tc);
        // Re-import the exported command and verify it produces the same request.
        let tc2 = CurlAdapter::import(&cmd, None).unwrap();
        assert_eq!(tc.request.url, tc2.request.url);
        assert_eq!(tc.request.method, tc2.request.method);
    }

    #[test]
    fn roundtrip_post() {
        let original =
            r#"curl -X POST https://api.example.com -H 'Authorization: Bearer tok' -d 'body'"#;
        let tc = CurlAdapter::import(original, None).unwrap();
        let cmd = CurlAdapter::export(&tc);
        let tc2 = CurlAdapter::import(&cmd, None).unwrap();
        assert_eq!(tc.request.method, tc2.request.method);
        assert_eq!(tc.request.url, tc2.request.url);
        assert_eq!(tc.request.body, tc2.request.body);
    }

    // ── name_from_url ─────────────────────────────────────────────────────────

    #[test]
    fn name_from_url_extracts_path() {
        assert_eq!(name_from_url("https://example.com/users/profile"), "users/profile");
    }

    #[test]
    fn name_from_url_root_falls_back() {
        assert_eq!(name_from_url("https://example.com/"), "Curl Request");
        assert_eq!(name_from_url("https://example.com"), "Curl Request");
    }

    #[test]
    fn name_from_url_drops_query() {
        assert_eq!(name_from_url("https://example.com/search?q=rust"), "search");
    }
}
