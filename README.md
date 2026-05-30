# hello_api

An HTTP testing and mock-server toolkit written in Rust. Run `.http` request
files as test suites, convert between collection formats, and attach JavaScript
pre/post scripts powered by a sandboxed V8 engine.

## Workspace

| Crate           | Role                                                                           |
| --------------- | ------------------------------------------------------------------------------ |
| `hello_sandbox` | V8/deno_core JavaScript sandbox — isolated execution with SDK packs            |
| `hello_core`    | Shared parsers and adapters (.http, Bruno, OpenAPI, Postman, curl)             |
| `hello_client`  | HTTP test runner, flow engine, CLI — depends on `hello_core` + `hello_sandbox` |
| `hello_server`  | Local HTTP mock server — depends on `hello_core` only                          |
| `hello_tui`     | Terminal UI                                                                    |

Dependency order: `hello_sandbox` ← `hello_core` ← `hello_client` /
`hello_server`.

## Quick Start

```bash
cargo build --release

# Run all requests in requests.http
./target/release/hello_client

# Run with variable substitution
./target/release/hello_client tests.http -p base_url=https://api.example.com -p token=secret

# Convert a Postman collection to .http
./target/release/hello_client collection.json --to requests.http
```

Exit code `0` when all tests pass, `1` on any failure.

## .http File Format

```http
### Get user
### @param user_id 42

GET https://{{base_url}}/users/{{user_id}}
Authorization: Bearer {{token}}

### Create user
POST https://{{base_url}}/users
Content-Type: application/json

{"name": "Alice", "email": "alice@example.com"}

> {%
  import { pm, test, expect, results } from "sandbox:pm";
  test("201 created", () => expect(pm.response.code).to.equal(201));
  return results();
%}
```

- `{{var}}` — substituted from `--param` flags or config file
- `### description` — separates entries and sets the test name
- `< script.js` or `< {% ... %}` — pre-request script (runs before fetch)
- `> script.js` or `> {% ... %}` — post-response script (runs after fetch)

## Scripts

Pre-request scripts run before the HTTP call and can override request fields:

```http
GET https://{{base_url}}/protected

< {%
  return { headers: [["Authorization", "Bearer " + kv.get("token")]] };
%}
```

Post-response scripts use the `sandbox:pm` Postman-compatible API:

```js
import { pm, test, expect, results } from "sandbox:pm";

test("status is 200", () => expect(pm.response.code).to.equal(200));
test("body has id", () => expect(pm.response.json().id).to.be.a("number"));

return results();
```

## Supported Formats

| Format                  | Import | Export |
| ----------------------- | :----: | :----: |
| `.http` / REST          |   ✓    |   ✓    |
| Postman v2.0/v2.1       |   ✓    |   ✓    |
| Bruno (`.bru`)          |   ✓    |   ✓    |
| OpenAPI 2/3 (YAML/JSON) |   ✓    |   ✓    |
| OpenCollection          |   ✓    |   ✓    |
| curl commands           |   ✓    |   ✓    |

```bash
# Auto-detected from file extension and content
hello_client openapi.yaml --to requests.http
hello_client ./bruno-collection/ --to collection.json
hello_client requests.http --to commands.sh
```

## CLI Reference

```
hello_client [OPTIONS] [INPUT]

Arguments:
  [INPUT]    .http file, collection file, or directory (default: requests.http)

Options:
  -p, --param <KEY=VAL>              Variable substitution (repeatable)
  -c, --config <FILE>                Config file (TOML/JSON/INI)
  -t, --timeout <SECS>               Request timeout (default: 30)
  -f, --format <pretty|plain|json>   Output format (default: pretty)
  -v, --verbose                      Print request/response details
  -o, --output <PATH>                Save response bodies to file or directory
      --offline <FILE>               Replay a recorded response instead of fetching
      --to <PATH>                    Convert collection to another format
      --collection-pre-script <FILE> Script to run before every request
      --visualize-dir <DIR>          Write pm.visualizer HTML reports here

Subcommands:
  split   Split a .http file into one file per request
  merge   Merge multiple .http files into one
```

## Dynamic Variables

Resolved at interpolation time without any configuration:

| Placeholder              | Produces                             |
| ------------------------ | ------------------------------------ |
| `{{$guid}}`              | Random UUID v4                       |
| `{{$timestamp}}`         | Unix seconds                         |
| `{{$isoTimestamp}}`      | ISO 8601 UTC string                  |
| `{{$randomInt}}`         | Random integer 0–1000                |
| `{{base64 <arg>}}`       | Base64-encode a literal or `{{var}}` |
| `{{sha256 <arg>}}`       | SHA-256 hex digest                   |
| `{{hmacSha256 key msg}}` | HMAC-SHA256 hex                      |
| `{{basicAuth user pwd}}` | `Basic <base64(user:pwd)>` value     |

## Build & Test

```bash
# Build everything
cargo build

# Run all hello_client tests (unit + integration)
cargo test -p hello_client

# Run hello_sandbox tests
cargo test -p hello_sandbox

# Run hello_core tests
cargo test -p hello_core

# Lint
cargo clippy -- -D warnings

# Release build
cargo build --release
```

## Documentation

Full documentation lives in [`docs/`](docs/index.md):

- [Usage Guide](docs/usage.md) — workflows, variables, scripts, CI
- [Adapters](docs/adapters.md) — Postman, Bruno, OpenAPI, curl
- [Sandbox](docs/sandbox/) — V8 sandbox architecture and SDK packs
- [Features](docs/features/) — detailed feature references (F1–F9)
- [Roadmap](docs/roadmap.md) — planned features
