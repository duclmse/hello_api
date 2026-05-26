# AssertPack — Formal Assertions

`src/sdk/assert_sdk.rs` + `sdk-ts/src/assert.js`

AssertPack provides a formal assertion API whose pass/fail counts flow into `RunMetrics` automatically, without requiring a `results()` call.

---

## Registration

```rust
use hello_sandbox::AssertPack;

let sandbox = SandboxBuilder::new()
    .sdk(AssertPack)
    .build()?;
```

---

## JavaScript API

```js
import { assert } from "sandbox:assert";

assert.ok(value);                      // truthy check
assert.equal(actual, expected, msg?);  // strict equality (===)
assert.notEqual(actual, expected);     // strict inequality (!==)
assert.contains(str, substr);         // string includes check
assert.throws(() => fn(), msg?);      // function throws
```

All methods:
- Increment `RunMetrics::assertions_passed` on success
- Increment `RunMetrics::assertions_failed` on failure
- **Never throw** — failures are collected silently

This is different from `sandbox:test` (`expect(...).toBe(...)`) which throws on failure. Use `AssertPack` when you want to count pass/fail without stopping execution on the first failure.

---

## Difference from `sandbox:test`

| | `sandbox:assert` | `sandbox:test` |
|--|-----------------|----------------|
| Source | Rust op | Pure JS library |
| On failure | Silent, counts | Throws exception |
| Return value | None needed | Must call `results()` |
| Metrics | Automatic in `RunMetrics` | Via `results()` return value |
| Warm slot reset | Automatic (RunState replaced) | Must call `results()` to reset |

---

## RunMetrics Integration

Assertion counts are available in `SandboxResult::metrics` after the run:

```rust
let result = sandbox.run(script).await?;
println!("passed: {}", result.metrics.assertions_passed);
println!("failed: {}", result.metrics.assertions_failed);
```

---

## Op

| Op | Description |
|----|-------------|
| `op_assert(pass: bool, message: String)` | Increment `assert_passed` or `assert_failed` in `RunState` |

---

## TypeScript Declarations

```typescript
// sdk-ts/types/assert.d.ts
export declare const assert: {
    ok(value: unknown, message?: string): void;
    equal(actual: unknown, expected: unknown, message?: string): void;
    notEqual(actual: unknown, expected: unknown, message?: string): void;
    contains(str: string, substr: string, message?: string): void;
    throws(fn: () => unknown, message?: string): void;
};
```

---

## Source

`src/sdk/assert_sdk.rs`
`sdk-ts/src/assert.js`
`sdk-ts/types/assert.d.ts`
