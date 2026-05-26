# 1. Postman Adapter

**Source:** `src/adapters/postman.rs`

### 1.1 Import

```rust
pub fn PostmanAdapter::import(json: &str) -> Result<PostmanCollection, PostmanError>
```

Parses a Postman Collection JSON string (v2.0 or v2.1) and returns a
`PostmanCollection` with all request items flattened into a `Vec<TestCase>`.

**`PostmanCollection` fields:**

| Field       | Type                      | Description                      |
| ----------- | ------------------------- | -------------------------------- |
| `name`      | `String`                  | Collection name from `info.name` |
| `variables` | `HashMap<String, String>` | Collection-level variables       |
| `tests`     | `Vec<TestCase>`           | Flattened request items          |

**Version detection:** The schema URL in `info.schema` is not used for dispatch.
Both v2.0 and v2.1 are handled transparently — the difference (array vs object
auth format) is detected per-field by `auth_get()`.

**Folder flattening:** Nested folders are flattened with `/`-separated paths:

```
Folder "Users"
  └── Item "Get All"   →  name: "Users/Get All"
  └── Folder "Admin"
        └── Item "Delete"  →  name: "Users/Admin/Delete"
```

**Auth inheritance:** If a folder has collection-level auth and an item has no
auth, the folder auth is inherited. Item-level auth takes priority.

### 1.2 Export

```rust
pub fn PostmanAdapter::export(
    name: &str,
    tests: &[TestCase],
    variables: &HashMap<String, String>,
) -> String
```

Produces a Postman Collection v2.1 JSON string (pretty-printed). Each `TestCase`
becomes one top-level item. URLs are split into `host` and `path` arrays in the
structured URL object.

**Example output structure:**

```json
{
  "info": {
    "name": "My API",
    "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"
  },
  "item": [
    {
      "name": "Get Users",
      "request": {
        "method": "GET",
        "header": [{ "key": "Accept", "value": "application/json" }],
        "url": {
          "raw": "https://api.example.com/users",
          "host": ["api", "example", "com"],
          "path": ["users"]
        }
      },
      "event": [
        {
          "listen": "prerequest",
          "script": { "type": "text/javascript", "exec": ["..."] }
        },
        {
          "listen": "test",
          "script": { "type": "text/javascript", "exec": ["..."] }
        }
      ]
    }
  ],
  "variable": [{ "key": "base_url", "value": "https://api.example.com" }]
}
```

### 1.3 Auth handling

| Postman auth type           | Mapped to                                         |
| --------------------------- | ------------------------------------------------- |
| `bearer`                    | `Authorization: Bearer <token>` header            |
| `basic`                     | `Authorization: Basic <base64(user:pass)>` header |
| `apikey` (in=header)        | `<key>: <value>` header                           |
| `apikey` (in=query)         | Appended to URL as `?key=value`                   |
| `oauth2` (with accessToken) | `Authorization: Bearer <accessToken>` header      |

Both v2.0 (plain object) and v2.1 (array of `{key, value}` entries) auth formats
are handled:

```jsonc
// v2.1 format
"bearer": [{ "key": "token", "value": "my-token" }]

// v2.0 format
"basic": { "username": "user", "password": "pass" }
```

### 1.4 Script wrapping

Imported scripts are wrapped with `sandbox:pm` import preambles automatically,
so they run correctly in the hello_sandbox engine without modification.

**Pre-request script** (`listen: "prerequest"`):

```js
import { pm, bru, req } from "sandbox:pm";
// ... original script body ...
```

**Test script** (`listen: "test"`):

```js
import { pm, res, bru, test, expect, results } from "sandbox:pm";
// ... original script body ...
return results();
```

The `return results();` suffix is appended automatically. If the original script
already ends with `results()`, it will be called twice — the second call returns
an empty result (state was reset by the first). This is harmless but can be
avoided by stripping the existing call before import if needed.

**`exec` array joining:** Postman stores script lines as an array of strings. On
import, lines are joined with `\n`. On export, script strings are split by `\n`
back into arrays.

### 1.5 Supported features

| Feature                                                | Import                          | Export            |
| ------------------------------------------------------ | ------------------------------- | ----------------- |
| GET / POST / PUT / DELETE / PATCH / HEAD / OPTIONS     | ✓                               | ✓                 |
| Raw URL string                                         | ✓                               | —                 |
| Structured URL object (`raw`, `host`, `path`, `query`) | ✓                               | ✓                 |
| Request headers (enabled only)                         | ✓                               | ✓                 |
| Raw body                                               | ✓                               | ✓                 |
| URL-encoded form body                                  | ✓ (decoded to string)           | —                 |
| Multipart form body                                    | ✓ (key=value or key=<file:src>) | —                 |
| Bearer auth                                            | ✓ → header                      | ✓ from header     |
| Basic auth                                             | ✓ → header                      | ✓ from header     |
| API Key auth (header)                                  | ✓ → header                      | —                 |
| API Key auth (in=query)                                | ✓ → URL param                   | —                 |
| OAuth2 (with pre-configured accessToken)               | ✓ → header                      | —                 |
| Pre-request scripts                                    | ✓                               | ✓                 |
| Test scripts                                           | ✓                               | ✓                 |
| Collection variables                                   | ✓                               | ✓                 |
| Folder nesting                                         | ✓ (flattened)                   | ✓ (reconstructed) |
| Auth inheritance from folder                           | ✓                               | —                 |
| Disabled items                                         | skipped                         | —                 |
| Dynamic variables (`{{$timestamp}}` etc.)              | ✓ (pre-script resolver)         | —                 |

### 1.6 Environment files

```rust
pub fn PostmanAdapter::import_with_env(
    collection_json: &str,
    env_json: &str,
) -> Result<PostmanCollection, PostmanError>
```

Merges enabled environment variables from a standard Postman environment JSON
file into `PostmanCollection.variables`. Environment variables override
same-name collection variables.

The Postman environment JSON format expected:

```json
{
  "values": [
    { "key": "base_url", "value": "https://api.example.com", "enabled": true },
    { "key": "token", "value": "secret", "enabled": true },
    { "key": "debug_flag", "value": "1", "enabled": false }
  ]
}
```

Only entries with `"enabled": true` are merged. Disabled entries are ignored.
The merge result is equivalent to calling `import(collection_json)` and then
overriding `collection.variables` with the enabled env vars.
