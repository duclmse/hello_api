# Collection Adapters

`hello_client` can import test collections from multiple formats and export to
all supported formats.

## Module Layout

```
src/adapters/
  mod.rs            re-exports
  postman.rs        Postman Collection v2.0 / v2.1 import + export
  bruno.rs          Bruno .bru file format import + export
  curl.rs           curl command import + export
  opencollection.rs OpenCollection v1.0.0 JSON import + export
  openapi.rs        OpenAPI 3.x / Swagger 2.0 import + export
  bru_parser.rs     (pub(crate)) shared .bru section/KV parser
```

Public types re-exported from `hello_client`:

- `PostmanAdapter`, `PostmanCollection`, `PostmanError`
- `BrunoAdapter`, `BrunoError`
- `CurlAdapter`, `CurlError`
- `OpenCollectionAdapter`, `OpenCollection`, `OpenCollectionError`
- `OpenApiAdapter`, `OpenApiCollection`, `OpenApiError`

---

## Postman Adapter

**Source:** `src/adapters/postman.rs`

### Import

```rust
use hello_client::{PostmanAdapter, PostmanCollection};

let json = std::fs::read_to_string("collection.json")?;
let PostmanCollection { name, variables, tests } = PostmanAdapter::import(&json)?;

// `tests` is a Vec<TestCase> ready to pass to HttpTestRunner
let result = runner.run_collection(tests).await?;
```

`PostmanAdapter::import` handles both **v2.0** and **v2.1** schema formats.
Nested folders are flattened to `"folder/item"` names.

### Export

```rust
use std::collections::HashMap;
use hello_client::PostmanAdapter;

let json_str = PostmanAdapter::export("My Collection", &test_cases, &variables);
std::fs::write("output.json", &json_str)?;
```

Generates a Postman v2.1 collection JSON with structured URL objects and event
scripts.

### What Is Imported

| Postman Field              | Maps To                                                                          |
| -------------------------- | -------------------------------------------------------------------------------- |
| Item name (+ folder path)  | `TestCase::name`                                                                 |
| Request method             | `TestCase::request.method`                                                       |
| URL `.raw`                 | `TestCase::request.url`                                                          |
| Headers (non-disabled)     | `TestCase::request.headers`                                                      |
| Auth (bearer/basic/apikey) | Added to headers                                                                 |
| Body (raw mode)            | `TestCase::request.body`                                                         |
| Body (urlencoded mode)     | Encoded and set as body                                                          |
| `prerequest` event script  | `TestCase::pre_script` (with `sandbox:pm` import preamble)                       |
| `test` event script        | `TestCase::post_script` (with `sandbox:pm` import preamble + `return results()`) |

**Not supported:** `formdata` body, OAuth2, query parameter variables (appended
to URL instead), collection-level auth inheritance beyond one level.

### Serde Types

The full Postman JSON schema is represented as:

```
PostmanCollectionRaw
  PostmanInfo          { name, schema, description }
  PostmanItem[]        { name, item?, request?, event[] }
  PostmanRequest       { method, url, header[], body?, auth? }
  PostmanUrl           Raw(str) | Structured { raw, query[], variable[] }
  PostmanBody          { mode, raw?, formdata?, urlencoded? }
  PostmanAuth          { type, bearer?, basic?, apikey?, oauth2? }
  PostmanEvent         { listen, script, disabled }
```

---

## Bruno Adapter

**Source:** `src/adapters/bruno.rs`

Bruno uses a custom text format (`.bru` files) with named sections.

### Import — Single File

```rust
use hello_client::{BrunoAdapter, TestCase};

let content = std::fs::read_to_string("get-user.bru")?;
let test_case: TestCase = BrunoAdapter::import(&content)?;
```

### Import — Directory

```rust
let test_cases: Vec<TestCase> = BrunoAdapter::import_dir(Path::new("./bruno-collection"))?;
```

Reads all `.bru` files in a directory, sorts them by their `seq` field
(ascending), and returns `Vec<TestCase>`. Files without a `seq` field sort to
the end.

### Export

```rust
let bru_str = BrunoAdapter::export(&test_case);
std::fs::write("output.bru", &bru_str)?;
```

### .bru File Format

A `.bru` file is made of named sections delimited by `{ }`:

```
meta {
  name: Get User
  type: http
  seq: 1
}

get {
  url: https://api.example.com/users/{{user_id}}
  body: none
}

headers {
  Authorization: Bearer {{token}}
  Accept: application/json
}

body:json {
  {"filter": "active"}
}

body:form-urlencoded {
  username: alice
  role: admin
}

body:multipart-form {
  title: File upload demo
  payload@: ./upload_payload.json
}

body:file {
  filename: ./payload.json
  contentType: application/json
}

script:pre-request {
  // pre-request JS
}

test {
  // post-response test JS
}

assert {
  res.status: eq 200
  res.body.id: isDefined
}
```

### Section Mapping

| Bruno Section                    | Maps To                                                     |
| -------------------------------- | ----------------------------------------------------------- |
| `meta.name`                      | `TestCase::name`                                            |
| `get/post/put/...` url           | `TestCase::request.url` and method                          |
| `headers`                        | `TestCase::request.headers`                                 |
| `params:query`                   | Appended to URL as `?k=v&...`                               |
| `body:json`                      | `TestCase::request.body` + `Content-Type: application/json` |
| `body:text`                      | `TestCase::request.body` + `Content-Type: text/plain`       |
| `body:xml`                       | `TestCase::request.body` + `Content-Type: application/xml`  |
| `body:form-urlencoded`           | URL-encoded body + Content-Type header                      |
| `body:multipart-form`            | Multipart body; fields with `@` suffix are file parts       |
| `body:file`                      | Raw file body with `filename` + `contentType`               |
| `auth:bearer`                    | `Authorization: Bearer <token>` header                      |
| `auth:basic`                     | `Authorization: Basic <base64>` header                      |
| `auth:apikey` (header placement) | Custom header                                               |
| `script:pre-request`             | `TestCase::pre_script`                                      |
| `script:post-response` / `test`  | `TestCase::post_script`                                     |
| `assert`                         | Converted to JS test statements appended to post_script     |
| `docs`, `vars:pre-request`       | Ignored                                                     |

### Assert Section

Bruno `assert` blocks are converted to JavaScript:

```
res.status: eq 200
res.body.name: contains Alice
res.body.count: gt 0
```

Becomes:

```js
test("assert: res.status eq 200", function () {
  expect(_bruGet(res, "res.status")).to.equal(200);
});
test("assert: res.body.name contains Alice", function () {
  expect(_bruGet(res, "res.body.name")).to.include("Alice");
});
```

Supported operators: `eq`, `neq`, `gt`, `gte`, `lt`, `lte`, `contains`,
`notContains`, `isDefined`, `isNull`, `isTruthy`, `isFalsy`, `startsWith`,
`endsWith`.

---

## Curl Adapter

**Source:** `src/adapters/curl.rs`

Converts between `curl` command strings and `TestCase` / `.http` format.

### Import

```rust
use hello_client::{CurlAdapter, TestCase};

let test_case: TestCase = CurlAdapter::import(
    "curl -X POST https://api.example.com/users \
     -H 'Content-Type: application/json' \
     -d '{\"name\":\"Alice\"}'",
    Some("create-user"),
)?;
```

- The second argument is an optional name; if `None`, a name is derived from the URL.
- `curl` flags supported: `-X`/`--request`, `-H`/`--header`, `-d`/`--data`/`--data-raw`,
  `--data-urlencode`, `-u`/`--user` (Basic auth), `-G`/`--get`, `-L`/`--location`,
  `-s`/`--silent`, `--compressed`.
- `-d` without an explicit `-X` flag implies `POST`.

### Export

```rust
let curl_str = CurlAdapter::export(&test_case);
// → "curl -X POST 'https://api.example.com/users' \
//      -H 'Content-Type: application/json' \
//      -d '{\"name\":\"Alice\"}'"
```

### Convert Directly to `.http`

```rust
let http_str: String = CurlAdapter::to_http(
    "curl https://api.example.com/users",
    Some("get-users"),
)?;
```

This is equivalent to `import` followed by serializing to the `.http` text format.

### `CurlError`

```rust
pub enum CurlError {
    Empty,          // input string was empty after tokenization
    NoUrl,          // no URL found in the curl command
    ParseError(String),
}
```

### CLI

The `from-curl` subcommand wraps `CurlAdapter::to_http`:

```bash
hello_client from-curl "curl https://api.example.com" -o request.http
# Or read from stdin:
pbpaste | hello_client from-curl -o request.http
```

---

## OpenCollection Adapter

**Source:** `src/adapters/opencollection.rs`

OpenCollection is the [OpenCollection v1.0.0](https://schema.opencollection.com/opencollection/v1.0.0.json)
portable JSON format for HTTP test collections. YAML is also accepted on import
(auto-detected by the leading `{` character).

### Schema

```json
{
  "opencollection": "1.0.0",
  "info": { "name": "My Collection", "version": "1.0.0" },
  "config": {
    "environments": [{
      "name": "default",
      "selected": true,
      "variables": [
        { "name": "base_url", "value": "https://httpbin.org" },
        { "name": "token", "value": "secret" }
      ]
    }]
  },
  "items": [
    {
      "info": { "name": "Get anything", "sequence": 1 },
      "http": {
        "method": "GET",
        "url": "{{base_url}}/anything",
        "headers": [{ "name": "Accept", "value": "application/json" }]
      },
      "auth": { "type": "bearer", "bearer": { "token": "{{token}}" } },
      "runtime": {
        "scripts": [
          { "type": "tests", "code": "pm.test(\"ok\", () => pm.expect(pm.response.code).to.equal(200));" }
        ],
        "assertions": [
          { "expression": "status", "operator": "equals", "value": 200 }
        ]
      }
    },
    {
      "info": { "name": "subfolder", "sequence": 2 },
      "items": [
        {
          "info": { "name": "nested request", "sequence": 1 },
          "http": { "method": "GET", "url": "{{base_url}}/anything" }
        }
      ]
    }
  ]
}
```

Scripts in `runtime.scripts[].code` are raw JavaScript — the adapter adds
the `sandbox:pm` import preamble and `return results();` automatically when
importing.

### Import

```rust
use hello_client::{OpenCollectionAdapter, OpenCollection};

let json = std::fs::read_to_string("collection.json")?;
let OpenCollection { name, env, tests } = OpenCollectionAdapter::import(&json)?;
```

### Export

```rust
use std::collections::HashMap;
use hello_client::OpenCollectionAdapter;

let json = OpenCollectionAdapter::export("My Collection", &test_cases, &env);
std::fs::write("collection.json", &json)?;
```

The exporter strips the `sandbox:pm` preamble and the trailing
`return results();` suffix from scripts before writing so the output stays
readable.

### What Is Imported

| Field                                         | Maps To                                                                           |
| --------------------------------------------- | --------------------------------------------------------------------------------- |
| `info.name`                                   | Collection name (defaults to `"Unnamed Collection"`)                              |
| `config.environments[selected].variables`     | `OpenCollection::env` (first selected env, or first env)                          |
| `items[].info.name` (+ folder path)           | `TestCase::name` (folders flattened as `"folder/item"`)                           |
| `items[].http.method`                         | `TestCase::request.method` (uppercased)                                           |
| `items[].http.url`                            | `TestCase::request.url`                                                           |
| `items[].http.headers` (non-disabled)         | `TestCase::request.headers`                                                       |
| `items[].auth` (bearer/basic/apiKey)          | Added to headers (`Authorization: Bearer …` etc.)                                 |
| `items[].http.body.type: raw`                 | `TestCase::request.body` + `Content-Type` header                                  |
| `items[].http.body.type: formUrlEncoded`      | URL-encoded body + `Content-Type: application/x-www-form-urlencoded`              |
| `items[].http.body.type: multipartForm`       | Joined `name=value` string (boundary not constructed)                             |
| `runtime.scripts[type: before-request]`       | `TestCase::pre_script` (wrapped with `sandbox:pm` import)                         |
| `runtime.scripts[type: after-response\|tests]`| `TestCase::post_script` (wrapped + `return results()` appended)                   |
| `runtime.assertions[]`                        | Converted to `pm.test()` JS assertions, appended to post_script                   |

Items with an `items` array are **folders** and are flattened recursively.
Items with an `http` object are HTTP requests. Other types (graphql, grpc,
websocket) are skipped silently.

### Assertion Operators

| Operator                      | Generated JS                                           |
| ----------------------------- | ------------------------------------------------------ |
| `equals` / `eq`               | `.to.equal(value)`                                     |
| `notEquals` / `neq`           | `.to.not.equal(value)`                                 |
| `contains`                    | `.to.include(value)`                                   |
| `notContains`                 | `.to.not.include(value)`                               |
| `greaterThan` / `gt`          | `.to.be.above(value)`                                  |
| `greaterThanOrEqual` / `gte`  | `.to.be.at.least(value)`                               |
| `lessThan` / `lt`             | `.to.be.below(value)`                                  |
| `lessThanOrEqual` / `lte`     | `.to.be.at.most(value)`                                |
| `isDefined` / `exists`        | `.to.exist`                                            |
| `isNull` / `null`             | `.to.be.null`                                          |
| `isEmpty` / `empty`           | `.to.be.empty`                                         |
| `isNotEmpty`                  | `.to.not.be.empty`                                     |
| `startsWith`                  | `.to.satisfy(v => v.startsWith(value))`                |
| `endsWith`                    | `.to.satisfy(v => v.endsWith(value))`                  |
| `matches`                     | `.to.satisfy(v => new RegExp(value).test(v))`          |

Expression paths: `status` → `pm.response.code`, `body.field` →
`pm.response.json()?.field`, `headers.X-Foo` → `pm.response.headers.get("X-Foo")`.

---

## OpenAPI / Swagger Adapter

**Source:** `src/adapters/openapi.rs`

Imports OpenAPI 3.x or Swagger 2.0 specs (YAML or JSON) into test cases, and
exports test cases to an approximate OpenAPI 3.0 YAML spec.

The adapter auto-detects format version by the presence of the `openapi` (3.x)
or `swagger` (2.0) top-level key.

### Import

```rust
use hello_client::{OpenApiAdapter, OpenApiCollection};

let yaml = std::fs::read_to_string("api-spec.yaml")?;
let OpenApiCollection { name, tests } = OpenApiAdapter::import(&yaml)?;
// Each path × method combination becomes one TestCase.
```

Each imported `TestCase`:
- **Name**: `operationId`, then `summary`, then `METHOD /path`
- **URL**: `servers[0].url + path` (3.x) or `scheme://host+basePath + path` (2.0)
- **Path parameters**: converted from `{name}` → `{{name}}`
- **Body**: extracted from the operation's `example` or `schema.example` field
- **Content-Type header**: set when a body is present

No scripts are generated — test assertions must be added separately.

### Export

```rust
use hello_client::OpenApiAdapter;

let yaml = OpenApiAdapter::export("My API", &test_cases);
std::fs::write("spec.yaml", &yaml)?;
```

Generates an OpenAPI 3.0 YAML spec with:
- One `server` entry per distinct base URL in the test cases
- One path item per unique URL path, one operation per HTTP method
- Request bodies with `example` schemas derived from the test case body
- Path parameters derived from `{{param}}` segments in the URL
- Minimal `200 OK` responses as placeholders

### `OpenApiError`

```rust
pub enum OpenApiError {
    Parse(serde_yaml::Error),  // invalid YAML/JSON
    NotOpenApi,                // missing 'openapi' or 'swagger' key
}
```

---

## Script Preamble

All adapters wrap imported scripts with a `sandbox:pm` import preamble so
Postman/Bruno `pm.*` API calls work without modification.

**Pre-script preamble:**

```js
import { pm, bru, req } from "sandbox:pm";
// ... original script ...
```

**Post-script preamble + auto-appended suffix:**

```js
import { pm, res, bru, test, expect, results } from "sandbox:pm";
// ... original script ...
return results();
```

The `return results();` suffix is appended automatically if the script's last
non-empty line does not already start with `return`. This means scripts that
explicitly call `return results();` at the end are not modified.
