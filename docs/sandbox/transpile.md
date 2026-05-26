# TypeScript Transpiler

`src/transpile.rs` provides TypeScript-to-JavaScript transpilation using
`deno_ast` (backed by SWC).

## API

### `transpile`

```rust
pub fn transpile(
    specifier: &str,
    source: &str,
    force_ts: bool,
) -> Result<String, SandboxError>
```

Transpiles `source` to ES2022 JavaScript. If the source is already plain
JavaScript and `force_ts` is false, it is returned unchanged.

- `specifier` — used for error messages (e.g. `"sandbox:my-module"`)
- `source` — source code string
- `force_ts` — if `true`, always transpile as TypeScript regardless of
  heuristics

Returns `SandboxError::TranspileError(msg)` on syntax errors.

### `looks_like_typescript`

```rust
pub fn looks_like_typescript(source: &str) -> bool
```

Heuristic detector. Returns `true` if the source contains TypeScript-specific
syntax:

- Type annotations (`: Type`)
- Interfaces (`interface Foo`)
- Type aliases (`type Foo =`)
- Generics (`Array<T>`, `Promise<void>`)
- TypeScript-specific keywords (`as`, `keyof`, `readonly`, etc.)

This check is fast (no parsing) and may have false positives for edge cases. Use
`force_ts = true` in the loader to bypass it for known TypeScript files.

---

## What Gets Stripped

The transpiler removes:

- Type annotations on function parameters and return types
- `interface` and `type` declarations
- `import type` statements
- Generic type parameters
- TypeScript-specific syntax (`as`, `satisfies`, etc.)
- TSX (`.tsx` syntax is supported)

The output targets ES2022 — no downleveling to older JavaScript. `async/await`,
optional chaining, and nullish coalescing pass through unchanged.

---

## Examples

**Type annotations stripped:**

```typescript
function greet(name: string): string {
  return "Hello, " + name;
}
```

→

```javascript
function greet(name) {
  return "Hello, " + name;
}
```

**Interface removed:**

```typescript
interface User {
  id: number;
  name: string;
}
const u: User = { id: 1, name: "Alice" };
```

→

```javascript
const u = { id: 1, name: "Alice" };
```

---

## Dependency Versions

- `deno_ast = "0.51"` — uses `swc_common` 14.x (no `serde::__private` issues)
- Emit target: `deno_ast::EmitOptions { target: ES2022, ... }`

---

## Tests

`src/transpile.rs` contains 6 unit tests:

1. Plain JS passthrough (no transpilation)
2. Type annotation stripping
3. Interface removal
4. Invalid TypeScript returns `TranspileError`
5. TSX support
6. `force_ts = true` override

---

## Source

`src/transpile.rs` — 147 lines
