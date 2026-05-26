# HttpPack — Outbound HTTP Fetch

`src/sdk/http_sdk.rs` + `sdk-ts/src/http.js`

HttpPack exposes a `fetch`-like API for making outbound HTTP requests from scripts. All requests are subject to an allowlist enforced in Rust before any network call.

---

## Registration

```rust
use hello_sandbox::sdk::http_sdk::{HttpPack, HttpConfig};

let pack = HttpPack::new(HttpConfig {
    allowed_prefixes: vec![
        "https://api.example.com".to_string(),
        "https://auth.example.com".to_string(),
    ],
    timeout: Duration::from_secs(30),
    max_response_bytes: 1024 * 1024,  // 1 MiB
});

let sandbox = SandboxBuilder::new()
    .sdk(pack)
    .build()?;
```

---

## JavaScript API

```js
import { fetch } from "sandbox:http";

const response = await fetch("https://api.example.com/users/42", {
    method: "GET",
    headers: { "Authorization": "Bearer token" },
});

console.log(response.status);     // 200
console.log(response.ok);         // true (2xx)

const body = await response.text();
const json = await response.json();
const bytes = await response.arrayBuffer();  // returns Uint8Array
```

### `SandboxResponse`

```js
class SandboxResponse {
    readonly status: number;
    readonly ok: boolean;       // status 200–299
    readonly redirected: boolean;
    readonly url: string;
    readonly headers: Headers;  // case-insensitive .get(name)

    text(): Promise<string>
    json(): Promise<unknown>
    arrayBuffer(): Promise<ArrayBuffer>
}
```

---

## HttpConfig

```rust
pub struct HttpConfig {
    /// URL prefix allowlist. Requests to URLs not matching any prefix are blocked.
    pub allowed_prefixes: Vec<String>,

    /// Per-request timeout (default: 30s).
    pub timeout: Duration,

    /// Maximum response body size in bytes (default: 1 MiB).
    /// Responses larger than this limit return an error.
    pub max_response_bytes: usize,
}
```

---

## Allowlist Enforcement

The allowlist is checked **before** any network call or `await`. A request to a URL that doesn't match any allowed prefix returns `SandboxError::CapabilityDenied`.

Per-run allowlists in `RunCapabilities::http_allowed_prefixes` are intersected with the pack-level allowlist (a URL must match both to proceed).

**Example:**
- Pack-level: `["https://api.example.com"]`
- Run-level: `["https://api.example.com/v2"]`
- Allowed: `"https://api.example.com/v2/users"` ✓
- Blocked: `"https://api.example.com/v1/users"` ✗ (run-level blocks it)
- Blocked: `"https://other.com"` ✗ (pack-level blocks it)

---

## Per-Slot State

`HttpState` is stored in `OpState` per pool slot:
- A single `reqwest::Client` (with connection pooling)
- The `HttpConfig` for this pack instance

The client is reused across runs within the same slot, benefiting from connection keep-alive.

---

## RunCapabilities Interaction

| Capability | Effect |
|------------|--------|
| `http_enabled = Some(false)` | All fetch calls return `CapabilityDenied` |
| `http_allowed_methods = Some(["GET", "POST"])` | Non-matching methods blocked |
| `http_allowed_prefixes = Some([...])` | Per-run URL allowlist (intersects with pack-level) |
| `http_calls_limit = Some(n)` | `RateLimitExceeded` after n fetch calls |

---

## Response Body

The response body is returned from Rust to JavaScript as a base64-encoded string (`body_b64`), then decoded in the JS shim. This avoids the need for a `TextDecoder` in the sandbox environment.

---

## Ops

| Op | Description |
|----|-------------|
| `op_http_fetch(url, method, headers_json, body?)` | Execute HTTP request, return `FetchResult` |

---

## TypeScript Declarations

```typescript
// sdk-ts/types/http.d.ts
export declare function fetch(url: string, init?: RequestInit): Promise<SandboxResponse>;

export declare class SandboxResponse {
    readonly status: number;
    readonly ok: boolean;
    readonly redirected: boolean;
    text(): Promise<string>;
    json(): Promise<unknown>;
    arrayBuffer(): Promise<ArrayBuffer>;
}
```

---

## Source

`src/sdk/http_sdk.rs`
`sdk-ts/src/http.js`
`sdk-ts/types/http.d.ts`
