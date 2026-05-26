# CryptoPack — Hashing, Random Bytes, UUID

`src/sdk/crypto_sdk.rs` + `sdk-ts/src/crypto.js`

CryptoPack provides cryptographic primitives: hashing, secure random bytes, and UUID generation.

---

## Registration

```rust
use hello_sandbox::sdk::crypto_sdk::CryptoPack;

let sandbox = SandboxBuilder::new()
    .sdk(CryptoPack)
    .build()?;
```

---

## JavaScript API

```js
import { crypto } from "sandbox:crypto";

// Hash a string or byte array
const sha256 = await crypto.hash("sha256", "hello world");
// → "b94d27b9934d3e08a52e52d7da7dabfac484efe04294e576..."  (hex string)

const sha512 = await crypto.hash("sha512", "hello world");

// Hash binary data (Uint8Array)
const bytes = new Uint8Array([1, 2, 3, 4]);
const digest = await crypto.hash("sha256", bytes);

// Generate secure random bytes
const randomBytes = await crypto.randomBytes(32);
// → Uint8Array(32) [...]

// Generate a UUID v4
const id = await crypto.uuid();
// → "550e8400-e29b-41d4-a716-446655440000"
```

---

## Algorithms

| Algorithm | `crypto.hash()` argument |
|-----------|--------------------------|
| SHA-256 | `"sha256"` |
| SHA-512 | `"sha512"` |

---

## Ops

| Op | Description |
|----|-------------|
| `op_crypto_hash(algorithm, data)` | Hash data, return hex string |
| `op_crypto_random_bytes(n)` | Generate n random bytes, return `Vec<u8>` |
| `op_crypto_uuid()` | Generate UUID v4 string |

---

## TypeScript Declarations

```typescript
// sdk-ts/types/crypto.d.ts
export declare const crypto: {
    hash(algorithm: "sha256" | "sha512", data: string | Uint8Array): Promise<string>;
    randomBytes(n: number): Promise<Uint8Array>;
    uuid(): Promise<string>;
};
```

---

## Source

`src/sdk/crypto_sdk.rs`
`sdk-ts/src/crypto.js`
`sdk-ts/types/crypto.d.ts`
