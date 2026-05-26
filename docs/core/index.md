# hello_core — Documentation Index

`hello_core` is a shared library crate that contains the spec-parsing and
adapter layer used by both `hello_client` and `hello_server`. It has no
dependency on `hello_sandbox` and no I/O beyond reading files — no HTTP runtime,
no JS execution, no CLI.

---

## Workspace position

```
hello_sandbox   (V8 sandbox — no knowledge of HTTP formats)
hello_core      (parsers + adapters — no sandbox, no runtime)
  ├── hello_client  (test runner, flow engine, CLI — adds sandbox)
  └── hello_server  (mock server — adds axum, no sandbox)
hello_tui       (depends on hello_client)
```

`hello_core` is the lowest layer that understands HTTP spec formats. Nothing
above it needs to re-implement format parsing.

---

## What lives here

### Parsers (moved from `hello_client`)

| Module          | File                   | Responsibility                                                                             |
| --------------- | ---------------------- | ------------------------------------------------------------------------------------------ |
| `http_request`  | `src/http_request.rs`  | `RequestEntry`, `HttpRequest`, `Url`, `Body`, `Script` — AST types output by the parser    |
| `client_parser` | `src/client_parser.rs` | nom parser for the `.http` file format; produces `Vec<RequestEntry<'_>>`                   |
| `metadata`      | `src/metadata.rs`      | `### comment-block` metadata parser; produces tag maps used by both runner and mock server |

### Adapters (moved from `hello_client`)

| Adapter                 | File                                      | Input               | Output       |
| ----------------------- | ----------------------------------------- | ------------------- | ------------ |
| `BrunoAdapter`          | `src/adapters/bruno.rs` + `bru_parser.rs` | `.bru` text         | `Collection` |
| `OpenApiAdapter`        | `src/adapters/openapi.rs`                 | OpenAPI 3 YAML/JSON | `Collection` |
| `PostmanAdapter`        | `src/adapters/postman.rs`                 | Postman v2.1 JSON   | `Collection` |
| `CurlAdapter`           | `src/adapters/curl.rs`                    | curl command string | `Collection` |
| `OpenCollectionAdapter` | `src/adapters/opencollection.rs`          | OpenCollection JSON | `Collection` |

All adapters implement a common `Adapter` trait and return a `Collection` of
request items. Errors are typed per adapter.

---

## What does NOT live here

| Concern                           | Lives in                                    |
| --------------------------------- | ------------------------------------------- |
| HTTP execution (`reqwest`)        | `hello_client::http_runner`                 |
| JavaScript sandbox (V8/deno_core) | `hello_sandbox`                             |
| Mock server routing + axum        | `hello_server`                              |
| Flow runner, script executor      | `hello_client`                              |
| CLI entry points                  | `hello_client`, `hello_server`, `hello_tui` |

---

## Public API

`hello_core/src/lib.rs` re-exports:

```rust
// Parsers
pub use client_parser::*;
pub use http_request::{Body, HttpRequest, MultipartPart, RequestEntry, Script, Url, UrlSegment};
pub use metadata::Metadata;

// Adapters
pub use adapters::{
    BrunoAdapter, BrunoCollection, BrunoError,
    CurlAdapter, CurlCollection, CurlError,
    OpenApiAdapter, OpenApiCollection, OpenApiError,
    OpenCollectionAdapter, OpenCollectionError,
    PostmanAdapter, PostmanCollection, PostmanError,
};
```

Consumers import `use hello_core::*;` or use the explicit paths above.

---

## Dependencies

`hello_core` is intentionally lean:

| Crate                                 | Use                           |
| ------------------------------------- | ----------------------------- |
| `nom`                                 | `.http` file parser           |
| `serde` + `serde_json` + `serde_yaml` | adapter deserialisation       |
| `anyhow` + `thiserror`                | error handling                |
| `log`                                 | debug/warn logging in parsers |

No async, no tokio, no reqwest, no deno_core.

---

## Creating the crate (first-time setup)

See [server/steps.md — Phase 0b](../server/steps.md#phase-0--scaffold) for the
exact steps: workspace member registration, Cargo.toml, file moves from
`hello_client`, and import path updates.

---

## Adding a new format adapter

1. Create `src/adapters/<format>.rs`.
2. Define a `<Format>Collection` type and a `<Format>Error` type.
3. Implement the `Adapter` trait.
4. Re-export from `src/adapters/mod.rs` and `src/lib.rs`.
5. Add unit tests inline with fixture strings — no file I/O in tests.
