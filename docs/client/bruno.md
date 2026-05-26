# 2. Bruno Adapter

**Source:** `src/adapters/bruno.rs`

### 2.1 Import

```rust
pub fn BrunoAdapter::import(content: &str) -> Result<TestCase, BrunoError>
```

Parses a single `.bru` file string into a `TestCase`.

```rust
pub fn BrunoAdapter::import_dir(dir: &Path) -> Result<Vec<TestCase>, BrunoError>
```

Reads all `.bru` files from a directory, sorts them by `seq` field (ascending),
and returns the list. Files without a `seq` are placed at the end.

### 2.2 Export

```rust
pub fn BrunoAdapter::export(test: &TestCase) -> String
```

Exports a `TestCase` to a `.bru` format string with `seq: 1`.

### 2.3 Section reference

A `.bru` file consists of named sections delimited by `{ ... }` braces. The
parser uses a brace-counting state machine to correctly handle nested JSON in
body sections.

**Full section list:**

| Section                                                          | Description                                                                |
| ---------------------------------------------------------------- | -------------------------------------------------------------------------- |
| `meta`                                                           | Metadata: `name`, `type`, `seq`                                            |
| `get` / `post` / `put` / `delete` / `patch` / `head` / `options` | Method + URL                                                               |
| `headers`                                                        | Request headers (`key: value`)                                             |
| `params:query`                                                   | Query parameters appended to URL                                           |
| `body:json`                                                      | JSON request body → `Content-Type: application/json`                       |
| `body:text`                                                      | Plain text body → `Content-Type: text/plain`                               |
| `body:xml`                                                       | XML body → `Content-Type: application/xml`                                 |
| `body:form-urlencoded`                                           | Form fields → percent-encoded body + `Content-Type`                        |
| `body:form-data`                                                 | Alias for `body:multipart-form`                                            |
| `body:file`                                                      | Raw file reference; stored as `< path` body                                |
| `auth:bearer`                                                    | Bearer token → `Authorization: Bearer <token>` header                      |
| `auth:basic`                                                     | Username + password → `Authorization: Basic <b64>` header                  |
| `auth:apikey`                                                    | API key → `<key>: <value>` header (in=header only)                         |
| `auth:oauth2`                                                    | Generates `client_credentials` token-exchange pre-script                   |
| `script:pre-request`                                             | Pre-request JS/TS                                                          |
| `script:post-response`                                           | Post-response JS/TS                                                        |
| `script:tests`                                                   | Alias for `script:post-response`                                           |
| `test`                                                           | Inline test assertions                                                     |
| `assert`                                                         | Bruno assert DSL (converted to JS `test()` calls)                          |
| `vars:pre-request`                                               | Generates `bru.setEnvVar()` / `bru.setVar()` pre-script calls             |
| `docs`                                                           | Documentation — ignored                                                    |

**KV line format within sections:**

```
key: value          # active entry
~key: value         # disabled (prefixed with ~, skipped on import)
# comment           # comment line (skipped)
// comment          # JS-style comment (skipped)
```

**Example `.bru` file:**

```bru
meta {
  name: Get User
  type: http
  seq: 3
}

get {
  url: https://api.example.com/users/{{userId}}
  body: none
}

headers {
  Accept: application/json
  ~X-Debug: true
}

auth:bearer {
  token: {{token}}
}

params:query {
  include: roles
}

script:pre-request {
  bru.setEnvVar("startTime", Date.now().toString());
}

assert {
  res.status: eq 200
  res.body: contains "id"
}
```

### 2.4 Assert DSL

The `assert` section provides a declarative assertion syntax. Each line is
converted to a `test()` call using the `sandbox:pm` assertion library.

**Syntax:** `<path>: <operator> [<value>]`

The path is resolved at runtime via `_bruGet(res, "path")` — a dot-notation
traversal helper:

```js
function _bruGet(obj, path) {
  return path.split(".").reduce(function (o, k) {
    return o && o[k];
  }, obj);
}
```

**Operator reference:**

| Operator            | JS equivalent                         | Notes           |
| ------------------- | ------------------------------------- | --------------- |
| `eq <val>`          | `expect(x).to.equal(<val>)`           | Strict equality |
| `neq <val>`         | `expect(x).to.not.equal(<val>)`       |                 |
| `gt <n>`            | `expect(x).to.be.above(<n>)`          | Numeric         |
| `gte <n>`           | `expect(x).to.be.at.least(<n>)`       | Numeric         |
| `lt <n>`            | `expect(x).to.be.below(<n>)`          | Numeric         |
| `lte <n>`           | `expect(x).to.be.at.most(<n>)`        | Numeric         |
| `contains <val>`    | `expect(x).to.include(<val>)`         | String or array |
| `notContains <val>` | `expect(x).to.not.include(<val>)`     |                 |
| `isDefined`         | `expect(x) !== null && !== undefined` |                 |
| `isNull`            | `expect(x).to.be.null`                |                 |
| `isTruthy`          | `expect(!!x).to.be.true`              |                 |
| `isFalsy`           | `expect(!!x).to.be.false`             |                 |
| `startsWith <s>`    | `expect(x).to.startsWith(<s>)`        |                 |
| `endsWith <s>`      | `expect(x).to.endsWith(<s>)`          |                 |

Values that parse as integers or floats are emitted as numeric JS literals. All
other values are wrapped in double quotes with internal quotes escaped.

**Example assert section and generated JS:**

```bru
assert {
  res.status: eq 200
  res.body: contains "userId"
  res.status: gt 100
}
```

Generated:

```js
import { pm, res, bru, test, expect, results } from "sandbox:pm";
function _bruGet(obj, path) {
  return path.split(".").reduce(function (o, k) {
    return o && o[k];
  }, obj);
}
test("assert: res.status eq 200", function () {
  expect(_bruGet(res, "res.status")).to.equal(200);
});
test('assert: res.body contains "userId"', function () {
  expect(_bruGet(res, "res.body")).to.include("userId");
});
test("assert: res.status gt 100", function () {
  expect(_bruGet(res, "res.status")).to.be.above(100);
});
return results();
```

### 2.5 Directory import with environment

```rust
pub fn BrunoAdapter::import_dir_with_env(dir: &Path, env_name: &str) -> Result<Vec<TestCase>, BrunoError>
```

Loads `<dir>/environments/<env_name>.bru`, parses its `vars` section, and
injects `bru.setEnvVar()` calls into every test case's pre-script. This is the
equivalent of selecting an environment in the Bruno UI before running a
collection.

**Environment file format** (`environments/staging.bru`):

```bru
vars {
  base_url: https://staging.api.example.com
  token: my-staging-token
}
```

Each active variable (`key: value`, not prefixed with `~`) is injected as a
pre-script preamble across all imported test cases:

```js
// Auto-generated from environments/staging.bru
bru.setEnvVar("base_url", "https://staging.api.example.com");
bru.setEnvVar("token", "my-staging-token");
```

**Folder-level scripts:** `.bru` files with `meta { type: folder }` or
`meta { type: collection }` in the directory are treated as folder-level script
files. Their `script:pre-request` and `script:post-response` sections are
prepended and appended (respectively) to every imported request's scripts within
that folder. This mirrors Bruno's folder-level script inheritance behavior.
