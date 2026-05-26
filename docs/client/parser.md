# .http File Format & Parser

The `.http` file format lets you define one or more HTTP requests in a single
text file, with optional pre/post scripts and metadata. The parser is built with
[nom](https://github.com/rust-bakery/nom).

## Relevant Modules

| Module          | File                   | Visibility   |
| --------------- | ---------------------- | ------------ |
| `client_parser` | `src/client_parser.rs` | `pub(crate)` |
| `http_request`  | `src/http_request.rs`  | `pub(crate)` |
| `metadata`      | `src/metadata.rs`      | `pub(crate)` |

---

## File Format

### Single Request

```http
### Optional description
### @param name value
### #hashtag

GET https://{{base_url}}/users/{{user_id}}
Authorization: Bearer {{token}}
Content-Type: application/json

{"filter": "active"}

> {%
  // pre-request script (inline)
  const req = sandbox.readInput("_request");
  req.headers["X-Timestamp"] = Date.now().toString();
  return req;
%}

> post_script.js
```

### Multiple Requests

Separate entries with a `###` metadata block:

```http
### First request
GET https://{{base_url}}/health

###
### @param endpoint /users
POST https://{{base_url}}{{endpoint}}
Content-Type: application/json

{"name": "Alice"}

> {%
  const res = sandbox.readInput("_response");
  // ...
  return results();
%}
```

---

## Metadata Blocks

Metadata lines start with `### ` and appear before a request entry.

| Syntax                      | Type        | Effect                                       |
| --------------------------- | ----------- | -------------------------------------------- |
| `### Some description text` | Description | Human-readable label                         |
| `### @param name value`     | Param       | Sets a template variable for this entry      |
| `### #hashtag`              | Hashtag     | Tagging / filtering (no runner effect today) |

Multiple metadata lines are allowed before a single request. The `### `
separator that starts a new entry resets the metadata context.

### Parsed Type: `Metadata<'a>`

```rust
pub struct Metadata<'a> {
    pub description: Vec<&'a str>,
    pub hashtags: HashSet<&'a str>,
    pub params: HashMap<&'a str, &'a str>,
}
```

Source: `src/metadata.rs`

---

## Request Line

```
METHOD URL [HTTP/version]
```

- `METHOD` — any uppercase HTTP method (`GET`, `POST`, `PUT`, `DELETE`, `PATCH`,
  `HEAD`, `OPTIONS`, ...)
- `URL` — raw or structured (see below)
- `HTTP/version` — optional (`HTTP/1.1`, `HTTP/2`); stored but not sent to
  reqwest

### URL Syntax

URLs may be raw strings or structured with `{{variable}}` placeholders:

```
https://api.example.com/users/{{user_id}}?filter={{filter}}&page=1
```

The parser represents URLs as `Url::Segments { host, path, query_params }` where
each segment is either `UrlSegment::Text` or `UrlSegment::Variable`. Raw URLs
without placeholders are stored as `Url::Raw`.

### Parsed Types: `Url<'a>`, `UrlSegment<'a>`

```rust
pub enum Url<'a> {
    Raw(&'a str),
    Segments {
        host: Vec<UrlSegment<'a>>,
        path: Vec<UrlSegment<'a>>,
        query_params: Vec<UrlSegment<'a>>,
    },
}

pub enum UrlSegment<'a> {
    Text(&'a str),
    Variable(&'a str),   // matches {{name}}
}
```

---

## Headers

One header per line, `Key: Value`, immediately after the request line. Blank
line ends the header block.

```http
GET https://example.com/api
Authorization: Bearer token123
Accept: application/json
X-Custom: {{custom_header}}
```

---

## Body

Appears after a blank line following headers. The body ends at the next `>`
script marker or `###` separator.

### Inline (Raw)

```http
POST https://example.com/api
Content-Type: application/json

{
  "name": "Alice",
  "role": "admin"
}
```

### File Reference

A single-line body starting with `<` loads the file content at run time:

```http
POST https://example.com/upload
Content-Type: application/json

< ./payload.json
```

### Multipart Form-Data

A body that starts with `--boundary` is parsed as multipart/form-data. Each part
can contain inline text or a `< file` reference:

```http
POST https://example.com/upload
Content-Type: multipart/form-data; boundary=----Boundary

------Boundary
Content-Disposition: form-data; name="title"

Hello World
------Boundary
Content-Disposition: form-data; name="file"; filename="data.json"
Content-Type: application/json

< ./data.json
------Boundary--
```

### Body Types: `Body`, `MultipartPart`, `PartContent`

```rust
pub enum Body {
    /// Plain text, JSON, XML, or URL-encoded form — sent as-is.
    Raw(String),
    /// `< path/to/file` — entire file content sent as the body at run time.
    File(String),
    /// Standard multipart/form-data body (`--boundary` blocks).
    Multipart { boundary: String, parts: Vec<MultipartPart> },
}

pub struct MultipartPart {
    pub headers: Vec<(String, String)>,
    pub content: PartContent,
}

pub enum PartContent {
    Text(String),
    File(String),   // `< path/to/file` inside a part
}
```

Source: `src/http_request.rs`

---

## Scripts

Scripts run inside the sandbox. Two positions are supported:

### Pre-request Script (`> {% ... %}` or `> file.js`)

Runs **before** the HTTP request is sent. The script can read
`sandbox.readInput("_request")` and return a modified request object to override
`url`, `method`, `headers`, and/or `body`.

**Inline:**

```js
> {%
  const req = sandbox.readInput("_request");
  req.headers["X-Nonce"] = Math.random().toString(36).slice(2);
  return req;
%}
```

**File reference:**

```
> ./scripts/add_auth.js
```

### Post-response Script

Runs **after** the HTTP response is received. Reads
`sandbox.readInput("_response")` and must return `results()` from
`sandbox:test`.

```js
> {%
  import { expect, wrapResponse, results } from "sandbox:test";
  const res = wrapResponse(sandbox.readInput("_response"));
  expect(res.status).toBe(200);
  const body = res.json();
  expect(body.id).not.toBe(null);
  return results();
%}
```

### Script Enum

```rust
pub enum Script<'a> {
    Inline(&'a str),
    File(&'a str),
}
```

---

## Full AST Types

### `RequestEntry<'a>`

Top-level parsed unit for one request:

```rust
pub struct RequestEntry<'a> {
    pub(crate) metadata: Metadata<'a>,
    pub(crate) pre_script: Option<Script<'a>>,
    pub(crate) post_script: Option<Script<'a>>,
    pub(crate) request: HttpRequest<'a>,
}
```

### `HttpRequest<'a>`

```rust
pub struct HttpRequest<'a> {
    pub(crate) request_line: RequestLine<'a>,
    pub(crate) headers: HashMap<&'a str, &'a str>,
    pub(crate) body: Option<Body>,
}
```

### `RequestLine<'a>`

```rust
pub struct RequestLine<'a> {
    pub method: &'a str,
    pub url: Url<'a>,
    pub http_version: Option<&'a str>,
}
```

`RequestLine::get_verbatim_endpoint()` reconstructs the URL with
`{{placeholders}}` intact (used internally by `runner.rs`).

---

## Parser Entry Points (`client_parser.rs`)

| Function                    | Input             | Output              |
| --------------------------- | ----------------- | ------------------- |
| `request_collection(input)` | Full file content | `Vec<RequestEntry>` |
| `request_entry(input)`      | Single entry text | `RequestEntry`      |
| `metadata(input)`           | Comment block     | `Metadata`          |
| `http_request(input)`       | Request text      | `HttpRequest`       |

All parsers use the nom `IResult` type and are `pub(crate)`.
