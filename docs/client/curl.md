# 3. Curl Adapter

**Source:** `src/adapters/curl.rs`

Parses `curl` command strings into `TestCase` values and serializes them back.
The shell tokenizer handles single/double quoting and `\` line continuations, so
multi-line `curl` commands copied from terminal output work without
modification.

### 3.1 Import

```rust
pub fn CurlAdapter::import(curl_str: &str, name: Option<&str>) -> Result<TestCase, CurlError>
```

Tokenizes and parses a `curl` command string. `name` overrides the auto-derived
entry name; when `None`, the name is derived from the URL path (e.g.
`"GET /users/profile"`).

```rust
use hello_client::{CurlAdapter, TestCase};

let tc: TestCase = CurlAdapter::import(
    r#"curl -X POST https://api.example.com/users \
       -H 'Content-Type: application/json' \
       -d '{"name":"Alice"}'  "#,
    Some("create-user"),
)?;

assert_eq!(tc.request.method, "POST");
assert_eq!(tc.request.url,    "https://api.example.com/users");
```

### 3.2 Export

```rust
pub fn CurlAdapter::export(test: &TestCase) -> String
```

Serializes a `TestCase` back to a `curl` command string, using shell-quoted
single quotes for all values. `GET` requests omit `-X`. Multi-option commands
use `\\\n  ` line continuations for readability.

```rust
let cmd = CurlAdapter::export(&tc);
// → "curl \\\n  -X POST \\\n  -H 'Content-Type: application/json' \\\n  -d '{\"name\":\"Alice\"}' \\\n  'https://api.example.com/users'"
```

**Export mapping:**

| `TestCase` field                       | curl output                        |
| -------------------------------------- | ---------------------------------- |
| `request.method` (non-GET)             | `-X <METHOD>`                      |
| `request.headers`                      | `-H 'Key: Value'` (one per header) |
| `request.headers` (`Authorization: Basic <b64>`) | `-u 'user:pass'`       |
| `request.body`                         | `-d '<body>'`                      |
| `request.url`                          | `'<url>'` (positional, last)       |
| `pre_script`, `post_script`, `modules` | Not exported                       |

### 3.3 `to_http` convenience

```rust
pub fn CurlAdapter::to_http(curl_str: &str, name: Option<&str>) -> Result<String, CurlError>
```

Converts a `curl` command directly to `.http` entry text. Calls `import` then
serializes the `TestCase` to the `.http` format. Used by the `from-curl` CLI
subcommand.

```rust
let http_text = CurlAdapter::to_http(
    "curl https://api.example.com/users",
    Some("get-users"),
)?;
// →
// ### get-users
//
// GET https://api.example.com/users
```

### 3.4 Flag mapping

**Mapped — affect the `TestCase`:**

| Flag(s)                                                                  | Maps to                                                         |
| ------------------------------------------------------------------------ | --------------------------------------------------------------- |
| `-X <METHOD>` / `--request <METHOD>`                                     | `request.method`                                                |
| `-H <K: V>` / `--header <K: V>` (repeatable)                             | `request.headers`                                               |
| `-d <DATA>` / `--data` / `--data-raw` / `--data-binary` / `--data-ascii` | `request.body`                                                  |
| `-d @file.json`                                                          | body as `< file.json` (file reference, read at run time)        |
| `--data-urlencode <DATA>` (repeatable)                                   | `request.body` — values joined with `&`                         |
| `-u <USER:PASS>` / `--user <USER:PASS>`                                  | `Authorization: Basic <base64>` header                          |
| `-A <AGENT>` / `--user-agent <AGENT>`                                    | `User-Agent` header                                             |
| `-e <URL>` / `--referer <URL>`                                           | `Referer` header                                                |
| `-b <COOKIE>` / `--cookie <COOKIE>`                                      | `Cookie` header                                                 |
| `--json <DATA>`                                                          | `request.body` + injects `Content-Type: application/json` + `Accept: application/json`; implies `POST` |
| `-F <name=value>` / `--form <name=value>`                                | multipart body; `name=@file.png` → `name=<file:file.png>`       |
| `--url <URL>`                                                            | `request.url` (alternative to positional URL)                   |
| `<URL>` (positional)                                                     | `request.url` (first positional wins; `--url` takes precedence) |

**Ignored — silently accepted, no effect on `TestCase`:**

| Flag(s)                                                      | Reason ignored                             |
| ------------------------------------------------------------ | ------------------------------------------ |
| `-L` / `--location`                                          | Redirect handling is not modelled          |
| `--compressed`                                               | Decompression handled by reqwest           |
| `-k` / `--insecure`                                          | TLS validation not exposed in TestCase     |
| `-s` / `--silent`, `-S` / `--show-error`, `-v` / `--verbose` | Output control                             |
| `-o <FILE>` / `--output <FILE>`                              | Use `TestCase::output_file` instead        |
| `-w <FMT>` / `--write-out <FMT>`                             | curl-specific output format                |
| `-m <SECS>` / `--max-time <SECS>`, `--connect-timeout`       | Use `TestCase::timeout_override`           |
| `--retry`, `--retry-delay`, `--retry-max-time`               | No retry logic in runner                   |
| `--limit-rate`                                               | Bandwidth control not modelled             |
| `-x <URL>` / `--proxy <URL>`                                 | Proxy not modelled                         |
| `--cacert`, `--capath`, `--cert`, `--key`, `--pass`          | TLS client certs not modelled              |
| `--trace`, `--trace-ascii`                                   | curl debug output                          |
| `--http1.0`, `--http1.1`, `--http2`, `--http3`               | HTTP version stored in `.http` parser only |
| Various boolean flags (`--fail`, `--tcp-nodelay`, etc.)      | Curl behaviour flags with no equivalent    |

### 3.5 Auto-inference rules

| Condition                                             | Result                                    |
| ----------------------------------------------------- | ----------------------------------------- |
| No `-X` and body present                              | `method = "POST"`                         |
| No `-X` and no body                                   | `method = "GET"`                          |
| `--json` used (no `-X`)                               | `method = "POST"`                         |
| Body starts with `{` or `[`, no `Content-Type` header | `Content-Type: application/json` injected |
| `--data-urlencode` used (no `--data`)                 | Values joined with `&` as body            |

### 3.6 Error types

```rust
pub enum CurlError {
    /// Input string had no tokens after tokenization.
    Empty,
    /// Tokens parsed but no URL could be found.
    NoUrl,
}
```

Both implement `std::error::Error` and `Display` via `thiserror`.
