# hello_server — Documentation Index

`hello_server` is a local HTTP mock server that reads an API specification file
and immediately starts serving matching routes with example responses. No code
generation, no extra tooling — point it at a file, get a running server.

---

## Supported input formats

| Format                  | Extensions                  | Source of responses                                       |
| ----------------------- | --------------------------- | --------------------------------------------------------- |
| `.http` file            | `.http`                     | `### @response` metadata blocks or sidecar `.mock.json`   |
| Bruno                   | `.bru`, directory of `.bru` | `response` blocks inside each request                     |
| OpenAPI 3.x             | `.yaml`, `.yml`, `.json`    | `responses[status].content.*.examples` / `schema.example` |
| Postman Collection v2.1 | `.json`                     | Saved example responses inside each request item          |

---

## Documents

| Document                         | Description                                                           |
| -------------------------------- | --------------------------------------------------------------------- |
| [Specification](spec.md)         | Data model, adapter contracts, server behaviour, CLI flags, admin API |
| [Implementation Steps](steps.md) | Ordered build plan: phases, files, types, test strategy               |

---

## Quick orientation

```
hello_server
  └── hello_core          ← parsers + adapters (Bruno, OpenAPI, Postman, .http)

hello_client
  ├── hello_core          ← same shared layer
  └── hello_sandbox       ← V8 sandbox (server never touches this)
```

`hello_core` is a new crate that holds all the spec-parsing and adapter code
extracted from `hello_client`. `hello_server` depends only on `hello_core` and
never pulls in the V8 sandbox. See [docs/core/index.md](../core/index.md) for
the full `hello_core` specification.
