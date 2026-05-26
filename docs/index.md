# hello_client — Documentation Index

This workspace contains four crates with distinct responsibilities:

| Crate                             | Purpose                                                          |
| --------------------------------- | ---------------------------------------------------------------- |
| [`hello_sandbox`](#hello_sandbox) | V8/deno_core JavaScript sandbox engine                           |
| [`hello_core`](core/index.md)     | Shared parsers + adapters (.http, Bruno, OpenAPI, Postman, curl) |
| [`hello_client`](#hello_client)   | HTTP test runner, flow engine, CLI — depends on core + sandbox   |
| [`hello_server`](server/index.md) | Local HTTP mock server — depends on core only, no sandbox        |

Dependency order: `hello_sandbox` ← `hello_core` ← `hello_client` /
`hello_server`. The sandbox has no knowledge of HTTP formats; `hello_core` has
no runtime or sandbox.

---

## Getting Started

| Document                            | Description                                                       |
| ----------------------------------- | ----------------------------------------------------------------- |
| [Usage Guide](usage.md)             | Common workflows: running requests, variables, scripts, CI        |
| [Development Guide](development.md) | Workspace layout, build commands, conventions, adding features    |
| [Roadmap](roadmap.md)               | Planned features: GraphQL, data-driven tests, auth, LSP, and more |

---

## hello_client

| Document                             | Description                                                         |
| ------------------------------------ | ------------------------------------------------------------------- |
| [CLI](client/cli.md)                 | Command-line interface reference                                    |
| [HTTP Runner](client/http_runner.md) | `HttpTestRunner`, `TestCase`, `CollectionResult`, `SecurityProfile` |
| [Runner Bridge](client/runner.md)    | `.http` file → `TestCase` bridge (`runner.rs`)                      |
| [Parser](client/parser.md)           | `.http` file format, `client_parser`, `http_request`, `metadata`    |
| [Adapters](adapters.md)              | Postman, Bruno, curl, OpenCollection, and OpenAPI import/export     |

---

## Features & Design

| Document                                                      | Description                                                                         |
| ------------------------------------------------------------- | ----------------------------------------------------------------------------------- |
| [sandbox-core.md](features/sandbox-core.md)                   | hello_sandbox core architecture: component layers, security model, design decisions |
| [sandbox-advanced.md](features/sandbox-advanced.md)           | hello_sandbox advanced features: metrics, SQLite, timers, streaming, V8 inspector   |
| [http-client.md](features/http-client.md)                     | hello_client feature reference: F1–F9, all implemented capabilities                 |
| [http-test-runner.md](features/http-test-runner.md)           | HTTP test runner implementation notes: three-phase execution, sandbox:test design   |
| [script-module-imports.md](features/script-module-imports.md) | ES module `import` support for pre/post scripts                                     |
| [postman-compat.md](features/postman-compat.md)               | Postman `pm.*` compatibility via `sandbox:pm`, migration notes                      |

---

## hello_sandbox

| Document                                | Description                                                       |
| --------------------------------------- | ----------------------------------------------------------------- |
| [Sandbox & Builder](sandbox/sandbox.md) | `Sandbox`, `SandboxBuilder`, `SandboxResult` — public entry point |
| [Pool](sandbox/pool.md)                 | `RuntimePool`, `PoolConfig`, `RuntimeKind`                        |
| [Runtime](sandbox/runtime.md)           | `SharedRuntime`, `RunState` — core V8 logic                       |
| [Config](sandbox/config.md)             | `SandboxConfig`, `RunCapabilities`, `RunMetrics`, `MetricsSink`   |
| [Error](sandbox/error.md)               | `SandboxError` variants                                           |
| [Isolation](sandbox/isolation.md)       | `IsolationLevel`, child-process worker, seccomp                   |
| [Module Loader](sandbox/loader.md)      | `AllowlistModuleLoader`, `CodeCache`                              |
| [Transpiler](sandbox/transpile.md)      | TypeScript → JavaScript transpilation                             |
| [Snapshot](sandbox/snapshot.md)         | V8 pre-baked snapshot for fast cold starts                        |
| **SDK Packs**                           |                                                                   |
| [SDK Overview](sandbox/sdk/overview.md) | `SdkExtension` trait, `SdkRegistry`, adding new packs             |
| [CorePack](sandbox/sdk/core.md)         | `console`, `sandbox.readInput`, `sandbox.emit`, `sandbox.tags`    |
| [KvPack](sandbox/sdk/kv.md)             | Key-value store: `kv.get/set/delete/list`                         |
| [HttpPack](sandbox/sdk/http.md)         | Outbound HTTP fetch with allowlist                                |
| [CryptoPack](sandbox/sdk/crypto.md)     | Hash, random bytes, UUID                                          |
| [SqlitePack](sandbox/sdk/sqlite.md)     | Embedded SQLite: `db.query/execute`                               |
| [TimerPack](sandbox/sdk/timer.md)       | `setTimeout`, `setInterval`, `clearTimeout`, `clearInterval`      |
| [AssertPack](sandbox/sdk/assert.md)     | Formal assertions tracked in `RunMetrics`                         |
| [PmPack](sandbox/sdk/pm.md)             | Postman/Bruno `pm.test()` compatibility                           |
