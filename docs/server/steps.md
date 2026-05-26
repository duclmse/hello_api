# hello_server — Implementation Steps

Work through phases in order. Each phase produces a compilable, testable unit
before the next begins. Do not start a later phase until the current one
compiles cleanly (`cargo build -p hello_server`) and its associated tests pass.

---

## Phase 0 — Scaffold

**Goal**: empty crate compiles; all dependencies are declared.

### 0a. Root `Cargo.toml` — add workspace deps

```toml
[workspace.dependencies]
axum        = { version = "0.8", features = ["macros"] }
tower       = { version = "0.5", features = ["full"] }
tower-http  = { version = "0.6", features = ["trace", "cors", "timeout", "compression-gzip"] }
http        = "1"
matchit     = "0.8"
arc-swap    = "1"
notify      = "6"
```

### 0b. Create `crate/hello_core`

`hello_core` is a new library crate that holds all the spec-parsing and adapter
code. Both `hello_client` and `hello_server` depend on it; neither depends on
the other.

**Workspace `Cargo.toml`** — add to `[workspace] members`:

```toml
"crate/hello_core",
```

**`crate/hello_core/Cargo.toml`**:

```toml
[package]
name    = "hello_core"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"

[dependencies]
serde      = { workspace = true }
serde_json = { workspace = true }
serde_yaml = { workspace = true }
anyhow     = { workspace = true }
thiserror  = { workspace = true }
nom        = { workspace = true }
log        = { workspace = true }
```

**Move from `hello_client` to `hello_core`** (git mv each file):

| Source path in hello_client  | Destination in hello_core |
| ---------------------------- | ------------------------- |
| `src/client_parser.rs`       | `src/client_parser.rs`    |
| `src/http_request.rs`        | `src/http_request.rs`     |
| `src/metadata.rs`            | `src/metadata.rs`         |
| `src/adapters/` (entire dir) | `src/adapters/`           |

Update `hello_core/src/lib.rs` to declare and re-export all moved modules.
Update `hello_client/Cargo.toml` to add
`hello_core = { path = "../hello_core" }` and remove the moved source files.
Update all `use crate::` paths in `hello_client` that referenced the moved
modules to `use hello_core::`.

Verify: `cargo build` at workspace root passes before continuing.

### 0c. `crate/hello_server/Cargo.toml`

```toml
[package]
name    = "hello_server"
version = "0.1.0"
edition = "2024"

[[bin]]
name = "hello_server"
path = "src/main.rs"

[dependencies]
hello_core = { path = "../hello_core" }
axum       = { workspace = true }
tower      = { workspace = true }
tower-http = { workspace = true }
http       = { workspace = true }
matchit    = { workspace = true }
arc-swap   = { workspace = true }
notify     = { workspace = true }
tokio      = { workspace = true }
serde      = { workspace = true }
serde_json = { workspace = true }
serde_yaml = { workspace = true }
anyhow     = { workspace = true }
thiserror  = { workspace = true }
clap       = { workspace = true }
rand       = { workspace = true }
log        = { workspace = true }
```

Note: `hello_sandbox` is **not** listed. The server has no JS runtime.

### 0d. `src/main.rs`

Minimal skeleton — just `fn main() {}`. Verify: `cargo build -p hello_server`.

---

## Phase 1 — Data model

**Files**: `src/model.rs`

**Goal**: all types from [spec §2](spec.md#2-internal-data-model) compile with
`serde` derives. No logic yet.

Steps:

1. Create `src/model.rs`.
2. Define `MockCollection`, `MockRoute`, `RouteMethod`, `Matcher`, `Pattern`,
   `MockResponse`, `ResponseBody`, `SelectionStrategy`.
3. Add `#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]` to each
   type.
4. `Pattern::Regex` holds a `String` in the serialised form; deserialise to
   `regex::Regex` lazily (add `regex` to deps).
5. Add `mod model;` + `pub use model::*;` to `src/lib.rs` (create `lib.rs` —
   `hello_server` exposes a thin library so integration tests can import types
   directly).
6. Verify: `cargo check -p hello_server`.

No tests for this phase — pure data types.

---

## Phase 2 — Ingestion: OpenAPI

**Files**: `src/ingest/mod.rs`, `src/ingest/openapi.rs`

**Goal**: parse a minimal OpenAPI 3 document into `MockCollection`. Start here
because OpenAPI has the richest example data and validates the model design.

Steps:

1. Create `src/ingest/mod.rs`:
   - Declare
     `pub trait IngestAdapter { fn ingest(&self, source: &str) -> anyhow::Result<MockCollection>; }`
   - Declare `pub mod openapi;`
2. Create `src/ingest/openapi.rs`:
   - Define minimal serde structs for OpenAPI AST: `OpenApiDoc`, `PathItem`,
     `Operation`, `Response`, `MediaType`, `Example`. Only the fields you need —
     use `#[serde(flatten)] extra: serde_json::Value` for the rest.
   - Implement `OpenApiAdapter` with `IngestAdapter`.
   - Path param normalisation: `{param}` already matches internal format — pass
     through.
   - Example extraction priority: `examples.*` → `example` → `schema.example` →
     `description`.
   - Multiple status codes per operation → `SelectionStrategy::MatchStatus`.
3. Write unit tests in `src/ingest/openapi.rs` (inline `#[cfg(test)]`):
   - `test_petstore_routes` — parse a minimal inline YAML string, assert route
     count, path, status codes, body.
   - `test_path_param_normalisation` — `/pets/{petId}` stays as `/pets/{petId}`.
   - `test_empty_spec` — empty `paths: {}` → zero routes, no panic.

---

## Phase 3 — Ingestion: Postman

**File**: `src/ingest/postman.rs`

**Goal**: flatten a Postman v2.1 collection tree into `MockCollection`.

Steps:

1. Add `pub mod postman;` to `src/ingest/mod.rs`.
2. Implement `PostmanAdapter`:
   - Use `hello_core::adapters::PostmanAdapter` to get the parsed
     `PostmanCollection`.
   - Walk the item tree depth-first; collect `RequestItem`s.
   - Normalise path variable syntax: `:param` → `{param}`.
   - Map each saved `response` to `MockResponse`.
   - Multiple responses → `SelectionStrategy::RoundRobin`.
3. Tests:
   - `test_flat_collection` — two requests, one response each.
   - `test_nested_folders` — three levels of folders, all requests extracted.
   - `test_round_robin_selection` — three saved responses, assert strategy.

---

## Phase 4 — Ingestion: Bruno

**File**: `src/ingest/bruno.rs`

Steps:

1. Add `pub mod bruno;` to `src/ingest/mod.rs`.
2. Implement `BrunoAdapter`:
   - Accept `source: &str` for a single file; implement a separate
     `BrunoAdapter::ingest_dir(path: &Path)` that reads all `.bru` files
     recursively and merges collections.
   - Use `hello_core::adapters::BrunoAdapter`.
   - Normalise `:param` → `{param}`.
3. Tests:
   - `test_single_file` — parse a `.bru` fixture string.
   - `test_response_blocks` — multiple response blocks → `RoundRobin`.

---

## Phase 5 — Ingestion: `.http` file

**File**: `src/ingest/http_file.rs`

Steps:

1. Add `pub mod http_file;` to `src/ingest/mod.rs`.
2. Implement `HttpFileAdapter`:
   - Use `hello_core`'s `client_parser` to parse `Vec<RequestEntry<'_>>`.
   - For each entry, read metadata tags `@response`, `@response-body`,
     `@response-header` to build `MockResponse`.
   - If no `@response` tag exists, emit
     `MockResponse { status: 204, body: Empty }` and log `warn!()`.
3. Tests:
   - `test_basic_entry` — single GET with `@response 200` and `@response-body`.
   - `test_no_response_tag` — falls back to 204, no panic.
   - `test_multiple_entries` — two entries → two routes.

---

## Phase 6 — Sidecar `.mock.json`

**File**: `src/ingest/sidecar.rs`

Steps:

1. Define `SidecarFile` serde struct matching the JSON schema in
   [spec §3.5](spec.md#35-sidecar-mockjson).
2. `fn load_sidecar(spec_path: &Path) -> anyhow::Result<Option<SidecarFile>>`:
   - Check for `<spec_path>.mock.json`; return `None` if absent.
3. `fn apply_sidecar(collection: &mut MockCollection, sidecar: SidecarFile)`:
   - For each override: find existing route by `(method, path)`; replace
     responses. If not found, push new route.
4. Tests:
   - `test_override_existing` — sidecar replaces responses for existing route.
   - `test_add_new_route` — sidecar adds a route not in the base collection.

---

## Phase 7 — Auto-detection

**File**: `src/detect.rs`

Steps:

1. Implement
   `pub fn detect_format(path: &Path, hint: Option<&str>) -> anyhow::Result<Format>`.
2. `Format` enum: `HttpFile`, `Bruno`, `OpenApi`, `Postman`.
3. Logic as described in [spec §7](spec.md#7-auto-detection-logic).
4. Tests cover each extension + content-sniff branch. Use inline temp strings —
   no file I/O needed for the sniff logic.

---

## Phase 8 — Route registry

**File**: `src/registry.rs`

Steps:

1. Add `matchit` to imports.
2. Implement `RouteRegistry::build(collection: MockCollection) -> Self`:
   - Sort: longest path first; exact before parameterised; specific method
     before `Any`.
   - Log `warn!` for any shadowed route.
3. Implement `RouteRegistry::match_request(...)` as per
   [spec §4](spec.md#4-route-registry).
4. Tests:
   - `test_exact_before_param` — `/users/me` matches before `/users/{id}`.
   - `test_method_wildcard` — `Any` method route matches when no specific method
     matches.
   - `test_header_matcher` — route with `Header` guard only fires when header
     present with correct value.
   - `test_query_matcher` — same for query param guard.
   - `test_no_match` — unregistered path returns `None`.

---

## Phase 9 — Response rendering

**File**: `src/render.rs`

Steps:

1. Implement `render_response(...)` as per
   [spec §5](spec.md#5-response-rendering).
2. `SelectionStrategy::RoundRobin` state: pass `&Arc<AtomicUsize>` into the
   function; the registry owns the `AtomicUsize` per route.
3. `ResponseBody::Template` rendering: write a private
   `fn render_template(t: &str, params: &HashMap<String, String>) -> String` —
   simple `str::replace` loop over params.
4. Tests:
   - `test_first_strategy` — always returns first response.
   - `test_round_robin_wraps` — counter wraps at `responses.len()`.
   - `test_template_substitution` — `{{id}}` filled from path params.
   - `test_delay` — delay_ms > 0 produces observable latency (use a small value
     like 10 ms in tests; assert elapsed >= delay).
   - `test_content_type_inference` — Json body gets `application/json` when no
     explicit header.

---

## Phase 10 — Axum server

**File**: `src/server.rs`

Steps:

1. Define `ServerConfig` (port, host, timeout, cors, history_size).
2. Define `ServerState` with `ArcSwap<RouteRegistry>`, `Arc<Mutex<RingBuffer>>`,
   `Arc<ServerConfig>`.
3. Implement the catch-all handler.
4. Implement admin routes as a separate `Router` merged via `.merge()`.
5. Assemble middleware stack.
6. `pub async fn serve(state: Arc<ServerState>, config: ServerConfig) -> anyhow::Result<()>`:
   - Build `TcpListener`, bind, call `axum::serve`.
7. Integration tests in `tests/server_tests.rs`:
   - Spawn server on a random port with a hand-built `MockCollection`.
   - Use `reqwest::Client` to call routes and assert responses.
   - `test_matched_route` — 200 + correct body.
   - `test_unmatched_route` — 404 JSON.
   - `test_admin_health` — `/_mock/health` returns 200.
   - `test_admin_routes` — `/_mock/routes` lists the route.
   - `test_admin_history` — matched request appears in history.
   - `test_cors_headers` — response has `Access-Control-Allow-Origin: *`.

---

## Phase 11 — CLI

**File**: `src/main.rs`

Steps:

1. Define `Args` struct with `clap::Parser` matching [spec §8](spec.md#8-cli).
2. `main()`:
   1. Parse args.
   2. Call `detect::detect_format`.
   3. Read file to `String`.
   4. Dispatch to the correct `IngestAdapter::ingest`.
   5. Optionally apply sidecar.
   6. Build `RouteRegistry`.
   7. Print startup banner to stderr.
   8. Call `server::serve`.
3. Map exit codes per spec.
4. Manual test: `cargo run -p hello_server -- samples/petstore.yaml --port 3001`
   and `curl http://localhost:3001/pets`.

---

## Phase 12 — Hot reload (`--watch`)

**File**: `src/watcher.rs`

Steps:

1. Add `notify` and confirm `arc-swap` are in deps (both added in Phase 0).
2. Implement
   `fn start_watcher(spec_path: PathBuf, state: Arc<ServerState>, format: Format)`.
3. Spawn a dedicated `std::thread` (not a tokio task) for the `notify` watcher,
   since notify's callback is synchronous.
4. On file event: re-read → re-ingest → build new registry →
   `state.registry.store(...)`.
5. Integration test `test_hot_reload`:
   - Write a temp spec file.
   - Start server with `--watch`.
   - Assert route A exists.
   - Overwrite spec file with a different route B.
   - Wait up to 2 s for reload notification.
   - Assert route B now exists and route A returns 404.

---

## Phase 13 — Final checks

1. `cargo clippy -p hello_server -- -D warnings` — fix all lints.
2. `cargo test -p hello_server` — all tests pass.
3. `cargo build --release -p hello_server` — release build succeeds.
4. Update `docs/index.md`: add `hello_core` and `hello_server` rows to the
   crates table and link to `docs/core/index.md` and `docs/server/index.md`.

---

## File layout after all phases

```
crate/hello_core/                ← new crate (Phase 0b)
  Cargo.toml
  src/
    lib.rs
    client_parser.rs             moved from hello_client
    http_request.rs              moved from hello_client
    metadata.rs                  moved from hello_client
    adapters/                    moved from hello_client
      mod.rs
      bruno.rs  bru_parser.rs
      openapi.rs  postman.rs
      curl.rs  opencollection.rs

crate/hello_server/
  Cargo.toml
  src/
    main.rs          CLI (Phase 11)
    lib.rs           pub use re-exports for integration tests
    model.rs         Data types (Phase 1)
    detect.rs        Format auto-detection (Phase 7)
    registry.rs      RouteRegistry, matching (Phase 8)
    render.rs        Response selection + rendering (Phase 9)
    server.rs        Axum server, admin API (Phase 10)
    watcher.rs       File-watch hot reload (Phase 12)
    ingest/
      mod.rs         IngestAdapter trait
      openapi.rs     (Phase 2)
      postman.rs     (Phase 3)
      bruno.rs       (Phase 4)
      http_file.rs   (Phase 5)
      sidecar.rs     (Phase 6)
  tests/
    server_tests.rs  End-to-end integration tests (Phase 10)
```

---

## Testing strategy summary

| Phase | Test location                    | Type                                |
| ----- | -------------------------------- | ----------------------------------- |
| 2–6   | `src/ingest/*.rs` `#[cfg(test)]` | Unit — parse fixture strings        |
| 7     | `src/detect.rs` `#[cfg(test)]`   | Unit — extension + sniff            |
| 8     | `src/registry.rs` `#[cfg(test)]` | Unit — routing logic                |
| 9     | `src/render.rs` `#[cfg(test)]`   | Unit — selection + template         |
| 10    | `tests/server_tests.rs`          | Integration — live server + reqwest |
| 12    | `tests/server_tests.rs`          | Integration — file write + reload   |

No mocking frameworks needed — the data model is plain structs and the server
can be started on an ephemeral port in each test.
