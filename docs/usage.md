# Usage Guide

This guide covers common workflows for `hello_client`. For the full flag
reference see [CLI reference](client/cli.md). For adapter-specific details see
[Adapters](adapters.md).

---

## Quick Start

```bash
# Run all requests in requests.http (default)
hello_client

# Run a specific file
hello_client my_tests.http

# Inject variables
hello_client my_tests.http -p base_url=https://api.example.com -p token=secret
```

Exit code is `0` when all tests pass, `1` if any fail or a runtime error occurs.

---

## 1. Writing `.http` Files

A `.http` file contains one or more request entries separated by `###` blocks.

```http
### Get user
GET https://{{base_url}}/users/{{user_id}}
Authorization: Bearer {{token}}

### Create user
POST https://{{base_url}}/users
Content-Type: application/json

{"name": "Alice", "email": "alice@example.com"}
```

`{{name}}` placeholders are replaced with values from `--param` flags or the
config file before the request is sent. Unknown placeholders are left unchanged.

**Multiple entries** in one file are separated by `###` lines. The text after
`###` becomes the test case name shown in results.

---

## 2. Variable Substitution

Three ways to supply variables, in order of precedence (highest wins):

### CLI flags (highest priority)

```bash
hello_client -p base_url=https://api.example.com -p token=abc123
```

### Config file

```bash
hello_client -c prod.env my_tests.http
```

Config file format (`prod.env`):

```ini
timeout = 60
verbose = true
base_url = https://api.example.com

param.token = my-secret
param.env   = production

collection_pre_script  = ./scripts/setup.js
collection_post_script = ./scripts/teardown.js
```

JSON format is also accepted:

```json
{
  "timeout": 60,
  "base_url": "https://api.example.com",
  "param": { "token": "my-secret", "env": "production" },
  "collection_pre_script":  "./scripts/setup.js",
  "collection_post_script": "./scripts/teardown.js"
}
```

CLI flags override config file values. `--collection-pre-script` /
`--collection-post-script` flags take priority over the config file values.

### Built-in dynamic variables

These are resolved at interpolation time without any external configuration:

| Placeholder              | Produces                                   |
| ------------------------ | ------------------------------------------ |
| `{{$guid}}`              | Random UUID v4                             |
| `{{$randomUUID}}`        | Alias for `$guid`                          |
| `{{$timestamp}}`         | Unix seconds (integer)                     |
| `{{$isoTimestamp}}`      | ISO 8601 UTC string                        |
| `{{$randomInt}}`         | Random integer 0–1000                      |
| `{{$randomBoolean}}`     | `true` or `false`                          |
| `{{base64 <arg>}}`       | Base64-encode a literal or `{{var}}`       |
| `{{base64url <arg>}}`    | URL-safe base64                            |
| `{{base64decode <arg>}}` | Base64-decode                              |
| `{{sha256 <arg>}}`       | SHA-256 hex digest                         |
| `{{md5 <arg>}}`          | MD5 hex digest                             |
| `{{hmacSha256 key msg}}` | HMAC-SHA256 hex, two args                  |
| `{{urlEncode <arg>}}`    | URL-encode                                 |
| `{{toUpper <arg>}}`      | Uppercase                                  |
| `{{toLower <arg>}}`      | Lowercase                                  |
| `{{concat a b ...}}`     | Concatenate arguments without separator    |
| `{{basicAuth user pwd}}` | `Basic <base64(user:pwd)>` header value    |

---

## 3. Pre- and Post-Scripts

Attach JavaScript to a request entry using `> {%  %}` blocks or file
references.

### Pre-script

Runs before the fetch. Can return an object to override the request:

```http
### Login
POST https://{{base_url}}/auth/login
Content-Type: application/json

{"user": "alice"}

> {%
  // Override the body with a timestamp-signed payload
  return {
    body: JSON.stringify({ user: "alice", ts: Date.now() })
  };
%}
```

Overrideable fields: `url`, `method`, `headers` (array of `[key, value]`
pairs), `body`. Unset fields keep their original values.

### Post-script

Runs after the fetch. Use `sandbox:pm` to write assertions:

```http
### Get user
GET https://{{base_url}}/users/1

> {%
  import { pm, test, expect, results } from "sandbox:pm";

  test("status is 200", function() {
    expect(pm.response.code).to.equal(200);
  });

  test("body has id", function() {
    const body = pm.response.json();
    expect(body.id).to.equal(1);
  });

  return results();
%}
```

`results()` **must** be the final `return` statement — it collects test
outcomes and resets module state for the next run.

### File references

Point to an external `.js` file instead of an inline block:

```http
### Create order
POST https://{{base_url}}/orders

> pre_auth.js
> post_validate.js
```

Files are resolved relative to the `.http` file's directory.

---

## 4. Collection-Level Scripts

Apply a script to every request in a collection without editing each entry.

```bash
hello_client requests.http \
  --collection-pre-script  ./scripts/set_auth.js \
  --collection-post-script ./scripts/log_response.js
```

The collection pre-script runs before every request's own pre-script; the
collection post-script runs after every request's own post-script.

---

## 5. Assertions with `sandbox:pm`

`sandbox:pm` is a Postman/Bruno-compatible scripting API available in all
post-scripts.

```js
import { pm, test, expect, results } from "sandbox:pm";

// Status code
test("200 ok", () => pm.expect(pm.response.code).to.equal(200));

// JSON body
test("has items", () => {
  const body = pm.response.json();
  pm.expect(body.items).to.be.an("array");
  pm.expect(body.items.length).to.be.above(0);
});

// Response header
test("json content-type", () => {
  pm.expect(pm.response.headers.get("content-type")).to.include("application/json");
});

// Response time
test("fast enough", () => {
  pm.expect(pm.response.responseTime).to.be.below(500);
});

return results();
```

**Environment variables** — persist across requests within a collection run:

```js
// In request 1's post-script — save the token
pm.environment.set("token", pm.response.json().access_token);

// In request 2's post-script — read it back
const tok = pm.environment.get("token");
```

**Globals** — persist across multiple `run_collection` calls on the same runner
instance (useful for stateful multi-pass testing in library usage).

See [sandbox:pm reference](client/pm-scripting.md) for the full API.

---

## 6. Output Formats

### `pretty` (default)

```
3 passed, 1 failed  (423ms)

  ✓ get-user    (141ms)
  ✓ list-users  (98ms)
  ✗ delete-user (184ms)
      - expected status 204, got 403
  ✓ health      (0ms)
```

### `plain` — CI-friendly

```bash
hello_client -f plain my_tests.http
```

```
PASS get-user
PASS list-users
FAIL delete-user
  expected status 204, got 403
PASS health
passed=3 failed=1
```

### `json` — machine-readable

```bash
hello_client -f json my_tests.http
```

```json
{
  "passed": 3,
  "failed": 1,
  "total_ms": 423,
  "results": [
    { "name": "get-user", "passed": true, "status": 200, "response_time_ms": 141, "failures": [] },
    { "name": "delete-user", "passed": false, "status": 403, "failures": ["expected status 204, got 403"] }
  ]
}
```

---

## 7. Collection Conversion

Convert between collection formats with `--to`. The input format is detected
automatically from the file extension and content.

| Input                              | Detected as     |
| ---------------------------------- | --------------- |
| `*.http`, `*.rest`                 | HTTP            |
| `*.json` (with `"schema"` key)     | Postman v2.1    |
| `*.json` (with `"opencollection"`) | OpenCollection  |
| `*.bru` or a directory of `.bru`   | Bruno           |
| `*.yaml` / `*.yml` (openapi/swagger) | OpenAPI       |
| `*.yaml` / `*.yml` (other)         | OpenCollection  |

### Examples

```bash
# Postman → .http (auto-detected from .http extension)
hello_client collection.json --to requests.http

# .http → Postman (auto-detected from .json extension)
hello_client requests.http --to collection.json

# Bruno directory → Postman (auto-detected from .json extension)
hello_client ./my_api/ --to collection.json

# .http → split files, one per request in a directory
hello_client requests.http --to ./split/ --format http --split

# .http → curl commands (auto-detected from .sh extension)
hello_client requests.http --to commands.sh

# OpenAPI spec → .http (auto-detected from .http extension)
hello_client openapi.yaml --to requests.http

# .http → OpenCollection (auto-detected from .yaml extension)
hello_client requests.http --to collection.yaml
```

Bruno export always requires a directory as the destination (e.g. `--to ./my-bruno-collection/`).

---

## 8. Importing Postman and Bruno Collections

### Postman

```bash
# Run a Postman collection directly
hello_client collection.json -p base_url=https://api.example.com

# Merge a Postman environment file into variables before running
# (use --to http first to extract, then run)
hello_client collection.json --to http -o requests.http
hello_client requests.http -p token=...
```

For the `PostmanAdapter` Rust API with environment merging, see
[postman.md](client/postman.md).

### Bruno

```bash
# Run a Bruno collection directory
hello_client ./my_api/ -p base_url=https://api.example.com

# Convert a Bruno directory to .http for review
hello_client ./my_api/ --to http -o all.http
```

Environment variables defined in Bruno's `environments/<name>.bru` can be
loaded via the `BrunoAdapter::import_dir_with_env` Rust API — see
[bruno.md](client/bruno.md).

---

## 9. Replaying Recorded Responses

Skip the real HTTP fetch and run scripts against a synthetic response loaded
from a file. Useful for offline testing, fixtures, and reproducible CI.

```bash
hello_client tests.http --offline fixtures/response.json
```

The response file must be a JSON object matching the internal response shape:

```json
{
  "status": 200,
  "ok": true,
  "headers": [["content-type", "application/json"]],
  "body": "{\"id\": 1, \"name\": \"Alice\"}",
  "response_time_ms": 5
}
```

Pre- and post-scripts run as normal; only the fetch phase is replaced.

---

## 10. Saving Response Bodies

Write the HTTP response body to disk after each request. Useful for capturing
API responses, building fixture files, or debugging.

```bash
# Single request — save to a specific file
hello_client request.http -o response.json

# Collection — save each response into a directory
hello_client tests.http -o ./snapshots/
```

For multiple test cases the path is used as a directory prefix: each response
body is written to `<path>/<sanitized-test-name>`. For a single request the
path is used verbatim.

Per-request `### @param output <path>` annotations in the `.http` file take
precedence over `-o`.

```http
### capture-token
POST https://{{base_url}}/auth/token
### @param output ./tokens/auth.json

Content-Type: application/json

{"client_id": "{{client_id}}"}
```

---

## 11. Visualizer Output

Post-scripts can generate HTML reports with `pm.visualizer.set(template, data)`.
Export them to files with `--visualize-dir`:

```bash
hello_client tests.http --visualize-dir ./reports/
```

One `<test-name>.html` file is written per test that called
`pm.visualizer.set(...)`. The `--visualize-dir` directory is created if absent.

```js
// Example post-script
import { pm, results } from "sandbox:pm";

pm.visualizer.set(`
  <h1>Report for {{name}}</h1>
  <p>Status: {{status}}</p>
`, { name: "get-user", status: pm.response.code });

return results();
```

---

## 12. File Management Subcommands

### Split a `.http` file

```bash
hello_client split collection.http -o ./split/
```

Writes one `.http` file per request entry. Request names with `/` separators
produce nested directories (e.g. `users/get-user` → `split/users/get_user.http`).

### Merge `.http` files

```bash
# Merge a directory of .http files
hello_client merge ./split/ -o merged.http

# Merge specific files
hello_client merge auth.http users.http orders.http -o all.http
```

### Import from curl (`--format curl`)

```bash
# Inline curl command → runs immediately, output in pretty format
hello_client --format curl "curl https://api.example.com/users -H 'Authorization: Bearer tok'"

# From clipboard (macOS) → convert to .http file
pbpaste | hello_client --format curl --to request.http

# Import curl and convert directly to Postman collection JSON
hello_client --format curl "curl https://..." --to collection.json
```

---

## 13. CI Integration

```yaml
# GitHub Actions example
- name: Run API tests
  run: |
    hello_client tests.http \
      --param base_url=${{ secrets.API_URL }} \
      --param token=${{ secrets.API_TOKEN }} \
      --format plain \
      --timeout 30
```

- Use `-f plain` for clean log output without ANSI codes.
- Exit code `1` fails the CI step automatically.
- Use `-f json` and pipe to `jq` for custom reporting.

```bash
hello_client -f json tests.http | jq '.results[] | select(.passed == false) | .name'
```

---

## See Also

| Document                              | Description                            |
| ------------------------------------- | -------------------------------------- |
| [CLI Reference](client/cli.md)        | Every flag and subcommand              |
| [Adapters](adapters.md)               | Postman, Bruno, curl, OpenAPI details  |
| [sandbox:pm](client/pm-scripting.md)  | Full pm.* scripting API reference      |
| [HTTP Runner](client/http_runner.md)  | Rust API for embedding in your code    |
| [Parser](client/parser.md)            | `.http` file syntax reference          |
