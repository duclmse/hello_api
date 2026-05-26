# hello_server — Technical Specification

## 1. Goals

- Accept one spec file (or directory) and start a local HTTP server with no
  further configuration required.
- Unified internal model: all format-specific knowledge lives in the ingestion
  layer; the router and server know nothing about `.http`, Bruno, OpenAPI, or
  Postman.
- Hot reload: re-read the spec and swap routes without restarting the process.
- Minimal dependencies: no code generation, no database, no external services.

---

## 2. Internal Data Model

All spec formats are normalised to this model before the server starts.
`model.rs` owns all types. No format-specific types leak past the ingestion
layer.

### `MockCollection`

```rust
pub struct MockCollection {
    pub name: String,
    pub routes: Vec<MockRoute>,
}
```

### `MockRoute`

```rust
pub struct MockRoute {
    pub id: String,                      // stable debug name / operationId
    pub method: RouteMethod,
    pub path: String,                    // "/users/{id}" — RFC 6570 {param} style
    pub matchers: Vec<Matcher>,          // optional guards evaluated after path match
    pub responses: Vec<MockResponse>,
    pub selection: SelectionStrategy,
}
```

### `RouteMethod`

```rust
pub enum RouteMethod {
    Specific(http::Method),
    Any,                                 // matches every method
}
```

### `Matcher`

Optional guards checked after path matching. A route only fires if all matchers
pass.

```rust
pub enum Matcher {
    Header { name: String, value_pattern: Pattern },
    QueryParam { name: String, value_pattern: Pattern },
}

pub enum Pattern {
    Exact(String),
    Contains(String),
    Regex(regex::Regex),
    Any,                                 // header/param must be present but any value
}
```

### `MockResponse`

```rust
pub struct MockResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: ResponseBody,
    pub delay_ms: u64,
}
```

### `ResponseBody`

```rust
pub enum ResponseBody {
    Empty,
    Text(String),
    Json(serde_json::Value),
    Template(String),   // {{param}} placeholders, filled from path/query params
}
```

When `Content-Type` is not set in `MockResponse.headers`, it is inferred:

- `Json` → `application/json`
- `Text` → `text/plain`
- `Template` → `text/plain` (override in headers to change)
- `Empty` → no `Content-Type` header

### `SelectionStrategy`

Controls which `MockResponse` is returned when a route has multiple responses.

```rust
pub enum SelectionStrategy {
    First,                              // always responses[0]
    RoundRobin,                         // cycle, state stored in Arc<AtomicUsize>
    Random,                             // rand::thread_rng
    MatchStatus(u16),                   // pick response whose status matches
                                        // the incoming X-Mock-Status header
}
```

Default when a route has one response: `First`. Default when a route has
multiple responses: `RoundRobin`.

---

## 3. Ingestion Adapters

Each adapter implements:

```rust
pub trait IngestAdapter {
    fn ingest(&self, source: &str) -> anyhow::Result<MockCollection>;
}
```

`source` is the full text content of the spec file (the caller handles file
I/O). For directory inputs (Bruno), the caller concatenates files before
passing, or the adapter receives a `&Path` variant — see §3.4.

### 3.1 `.http` file adapter

**Source**: `hello_core`'s `client_parser` parses the raw text into
`Vec<RequestEntry<'_>>`.

**Route extraction**:

- One `MockRoute` per `RequestEntry`.
- `method` from `RequestLine`.
- `path` from `Url`: strip scheme+host, keep path + query template.
- Response comes from the first `### @response <STATUS>` metadata block that
  follows the entry, or from a sidecar `.mock.json` file (§3.5).
- If no response is found, emit a single
  `MockResponse { status: 204, body: Empty }` and log a `WARN`.

**Metadata extension** (new key, no parser changes to `hello_client` needed):

```
### @response 200
### @response-body {"id": 1, "name": "Alice"}
### @response-header Content-Type application/json
```

These keys are read by the adapter from the `metadata.tags` map after parsing.

### 3.2 Bruno adapter

**Source**: `hello_core::adapters::BrunoAdapter` parses `.bru` files.

**Route extraction**:

- Iterate `Collection.requests`.
- `method` + `url` → route.
- `response` blocks inside `.bru` → one `MockResponse` each.
- Multiple `response` blocks → `SelectionStrategy::RoundRobin`.
- Bruno path variables (`:param`) are normalised to `{param}`.

**Directory input**: pass the directory `&Path`; the adapter uses
`std::fs::read_dir` to collect all `.bru` files recursively, then delegates each
file to `BrunoAdapter::ingest`.

### 3.3 OpenAPI adapter

**Source**: detected by `openapi:` or `swagger:` top-level key in the YAML/JSON.
Parse with `serde_yaml` / `serde_json` directly into the OpenAPI AST (no
external OpenAPI crate required — a minimal local struct suffices).

**Route extraction**:

- For each `paths[path][method]`:
  - Collect `responses` map.
  - For each status code, extract example body from (in priority order):
    1. `responses[status].content["application/json"].examples.*` (first entry)
    2. `responses[status].content["application/json"].example`
    3. `responses[status].content["application/json"].schema.example`
    4. `responses[status].description` as a `Text` body
  - One `MockResponse` per status code.
  - Multiple status codes → `SelectionStrategy::MatchStatus` (caller sets
    `X-Mock-Status: 404` to get a 404 back).
- `operationId` → `MockRoute.id`.
- OpenAPI path params `{param}` already match the internal format.
- `parameters` with `in: query` and `example`/`schema.example` → populate
  `Matcher::QueryParam` guards only when `required: true`.

### 3.4 Postman adapter

**Source**: `hello_core::adapters::PostmanAdapter` parses `.json` into a
`Collection`.

**Route extraction**:

- Flatten the item tree (nested folders) depth-first.
- For each `RequestItem`:
  - `request.method` + `request.url.path` → route.
  - `response[]` array: each saved example response → one `MockResponse`.
  - Multiple examples → `SelectionStrategy::RoundRobin`.
  - Postman path variables (`:param`) are normalised to `{param}`.
  - `response[*].header[]` → `MockResponse.headers`.
  - `response[*].body` → parsed as JSON if valid, otherwise `Text`.

### 3.5 Sidecar `.mock.json`

A file `<spec-file>.mock.json` alongside the spec can add or override route
responses. It is not tied to any one format.

```json
{
  "overrides": [
    {
      "method": "GET",
      "path": "/users/{id}",
      "responses": [
        {
          "status": 200,
          "headers": { "Content-Type": "application/json" },
          "body": { "id": "{{id}}", "name": "Alice" },
          "delay_ms": 100
        }
      ],
      "selection": "round-robin"
    }
  ]
}
```

When a sidecar is present, its entries are merged last: if a `(method, path)`
pair already exists in the collection, the sidecar responses replace it; if not,
the sidecar route is appended.

---

## 4. Route Registry

`registry.rs` builds an ordered route list and matches incoming requests.

### Building

```rust
pub struct RouteRegistry {
    routes: Vec<RegisteredRoute>,    // insertion-ordered, specific before wildcard
}

struct RegisteredRoute {
    route: Arc<MockRoute>,
    counter: Arc<AtomicUsize>,       // used by RoundRobin
}
```

`RouteRegistry::build(collection: MockCollection) -> Self`:

1. Sort routes: exact paths before parameterised paths; method-specific before
   `Any`; longer paths before shorter.
2. Log `WARN` for any route that is completely shadowed by a preceding entry.

### Matching

```rust
pub fn match_request(
    &self,
    method: &http::Method,
    path: &str,
    headers: &HeaderMap,
    query: Option<&str>,
) -> Option<(Arc<MockRoute>, HashMap<String, String>)>
```

Steps:

1. Iterate routes in order.
2. Try `matchit` path matching: skip if no match.
3. Check `RouteMethod`: skip if method does not match.
4. Evaluate `Matcher` guards: skip if any guard fails.
5. Return first passing route + extracted path params.

`matchit 0.8` is used for path pattern matching. It is already an indirect
dependency of axum; adding it explicitly to `hello_server/Cargo.toml` costs
nothing.

---

## 5. Response Rendering

`render.rs` produces the final HTTP response from a matched route.

```rust
pub async fn render_response(
    route: &MockRoute,
    path_params: HashMap<String, String>,
    query_params: HashMap<String, String>,
    counter: &AtomicUsize,
) -> (StatusCode, HeaderMap, Bytes)
```

Steps:

1. Select `MockResponse` via `SelectionStrategy`:
   - `First` → `responses[0]`
   - `RoundRobin` → `counter.fetch_add(1, Relaxed) % responses.len()`
   - `Random` → `rand::random::<usize>() % responses.len()`
   - `MatchStatus(code)` → find response whose `status == code`, fallback to
     `responses[0]`
2. If `delay_ms > 0` → `tokio::time::sleep(Duration::from_millis(delay_ms))`.
3. Render `ResponseBody`:
   - `Template(t)` → replace `{{key}}` with values from `path_params` then
     `query_params`; unknown keys left as-is.
   - Others → serialize directly.
4. Build `HeaderMap` from `MockResponse.headers`; infer `Content-Type` if
   absent.
5. Return `(StatusCode::from_u16(status)?, headers, body_bytes)`.

---

## 6. HTTP Server

`server.rs` wires axum and exposes the mock + admin endpoints.

### Architecture

A single catch-all handler receives all requests and delegates to the registry.
This avoids rebuilding the axum `Router` on hot reload — only the
`Arc<RouteRegistry>` is swapped.

```rust
pub struct ServerState {
    registry: ArcSwap<RouteRegistry>,  // arc-swap for lock-free hot reload
    history: Arc<Mutex<RingBuffer<HistoryEntry>>>,
    config: Arc<ServerConfig>,
}
```

### Catch-all handler

```rust
async fn handle_any(
    State(state): State<Arc<ServerState>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response
```

Path: any `/*path` (registered once; axum does not re-match once caught).

If no route matches → `404 application/json` with body:

```json
{ "error": "no mock route matched", "method": "GET", "path": "/users/99" }
```

### Middleware stack (tower-http)

Applied in order (outermost first):

| Layer                     | Config                             |
| ------------------------- | ---------------------------------- |
| `TraceLayer`              | logs method, path, status, latency |
| `CorsLayer::permissive()` | enabled unless `--no-cors`         |
| `TimeoutLayer`            | `config.timeout` (default 30 s)    |
| `CompressionLayer`        | gzip response bodies               |

### Admin API

All admin routes are prefixed `/_mock/` to minimise collision with real API
paths.

| Method   | Path             | Description                                                        |
| -------- | ---------------- | ------------------------------------------------------------------ |
| `GET`    | `/_mock/health`  | `{"status":"ok","routes":<N>}`                                     |
| `GET`    | `/_mock/routes`  | JSON array of all registered routes (method, path, response count) |
| `GET`    | `/_mock/history` | Last N matched requests (default N=100)                            |
| `DELETE` | `/_mock/history` | Clear history ring buffer                                          |
| `POST`   | `/_mock/reload`  | Re-read spec file from disk, rebuild registry                      |

`/_mock/history` entry shape:

```json
{
  "ts": "2026-05-25T10:00:00Z",
  "method": "GET",
  "path": "/users/42",
  "status": 200,
  "matched_route": "/users/{id}",
  "latency_ms": 5
}
```

---

## 7. Auto-detection Logic

`detect.rs` determines the format from the file extension and content.

Priority:

1. CLI `--format` flag overrides everything.
2. Extension `.bru` → Bruno.
3. Extension `.http` → `.http` file.
4. Extension `.yaml` / `.yml` → read first 4 kB; if contains `openapi:` or
   `swagger:` key → OpenAPI; otherwise error.
5. Extension `.json` → read first 4 kB:
   - Contains `"openapi"` key at top level → OpenAPI.
   - Contains `"info"` + `"schema"` keys with a Postman schema URL → Postman.
   - Otherwise error.
6. Path is a directory → Bruno (walk for `.bru` files).

---

## 8. CLI

```
hello_server [OPTIONS] <SPEC_FILE>

Arguments:
  <SPEC_FILE>           Path to spec file or directory

Options:
  -p, --port <PORT>     Bind port [default: 3000]
  -H, --host <HOST>     Bind host [default: 127.0.0.1]
  -f, --format <FMT>    Force format: http|bruno|openapi|postman
  -s, --strategy <STR>  Response selection: first|round-robin|random [default: auto]
      --delay <MS>      Add global latency to every response [default: 0]
      --no-cors         Disable permissive CORS headers
      --watch           Reload spec on file change
      --history <N>     History ring buffer size [default: 100]
  -v, --verbose         Log each matched route + rendered response body
```

Exit codes:

- `0` — server started and ran until SIGINT/SIGTERM.
- `1` — spec file could not be parsed or no routes were loaded.
- `2` — bind address already in use.

Startup output (stderr):

```
hello_server 0.1.0
Loaded 12 routes from petstore.yaml (OpenAPI)
Listening on http://127.0.0.1:3000
Admin:    http://127.0.0.1:3000/_mock/routes
```

---

## 9. Hot Reload

When `--watch` is set:

1. Spawn a `tokio::task` wrapping a `notify::RecommendedWatcher`.
2. Watch the spec file (or directory) for `Create` / `Modify` events.
3. On event: re-read file → run ingestion → build new `RouteRegistry`.
4. Swap via `ArcSwap::store(Arc::new(new_registry))`.
5. Print to stderr: `[reload] 14 routes loaded`.
6. The axum server is not restarted; in-flight requests finish against the old
   registry.

Dependency: `notify 6`, `arc-swap 1` — both to be added to the workspace
`Cargo.toml`.

---

## 10. Dependency additions

All new workspace-level entries:

| Crate        | Version | Features                                       |
| ------------ | ------- | ---------------------------------------------- |
| `axum`       | `0.8`   | `macros`                                       |
| `tower`      | `0.5`   | `full`                                         |
| `tower-http` | `0.6`   | `trace`, `cors`, `timeout`, `compression-gzip` |
| `http`       | `1`     | —                                              |
| `matchit`    | `0.8`   | —                                              |
| `arc-swap`   | `1`     | —                                              |
| `notify`     | `6`     | —                                              |

`hello_server/Cargo.toml` references all of the above plus
`hello_core = { path = "../hello_core" }`. It does **not** depend on
`hello_client` or `hello_sandbox`.
