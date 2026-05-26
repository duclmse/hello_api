# Module Loader & Code Cache

`src/loader.rs` provides the module loader that resolves `sandbox:*` specifiers
and enforces the module allowlist.

## AllowlistModuleLoader

The module loader implements deno_core's `ModuleLoader` trait. It:

1. **Blocks** `ext:` specifiers (deno internal modules — hidden from scripts)
2. **Blocks** `node:` specifiers (Node.js built-ins — not available in sandbox)
3. **Blocks** arbitrary absolute URLs (prevents fetching external modules)
4. **Allows** `sandbox:*` specifiers registered by SDK packs or user code
5. **Transpiles** TypeScript source on-demand before evaluation

### Specifier Resolution Rules

```
import "ext:something"      → ModuleNotFound (blocked)
import "node:fs"            → ModuleNotFound (blocked)
import "https://cdn.com/x"  → ModuleNotFound (blocked)
import "sandbox:kv"         → resolved if KvPack is registered
import "sandbox:my-lib"     → resolved if registered via register_module()
import "./relative"         → resolved relative to the importing module
```

### Registered Modules

SDK packs contribute their ESM files during construction via
`SdkExtension::esm_files()`. User code can register modules via
`Sandbox::register_module()` or `SandboxBuilder::module()`.

Each module is stored as `(specifier, source_code)`. TypeScript sources are
transpiled lazily on first load.

---

## AllowlistModuleLoaderBuilder

Fluent builder for constructing an `AllowlistModuleLoader`. Must be `Clone` so
`RuntimePool` can create a fresh loader for each new pool slot.

```rust
let loader = AllowlistModuleLoaderBuilder::default()
    .register("sandbox:utils", utils_js_source)
    .with_code_cache(shared_cache.clone())
    .build();
```

### Methods

| Method                         | Description                |
| ------------------------------ | -------------------------- |
| `.register(specifier, source)` | Add a module source        |
| `.with_code_cache(cache)`      | Enable V8 bytecode caching |

---

## CodeCache

V8 bytecode cache keyed by the SHA-256 hash of the module source. Bytecode is
generated on first load and reused on subsequent loads of the same source.

```rust
let cache = CodeCache::new_shared();  // Arc<Mutex<HashMap>>

// Share across pool slots
let loader = AllowlistModuleLoaderBuilder::default()
    .with_code_cache(cache.clone())
    .build();
```

### Methods

| Method                    | Description                                         |
| ------------------------- | --------------------------------------------------- |
| `CodeCache::new_shared()` | Create a new shared cache (returns `Arc<Mutex<_>>`) |
| `.len()`                  | Number of cached entries                            |
| `.is_empty()`             | Whether the cache is empty                          |
| `.purge()`                | Clear all cached bytecode                           |

### Cache Key

The cache key is the SHA-256 hash of the module's source code. If two modules
have identical source, they share the same cache entry. If a module's source
changes, its hash changes and a new bytecode entry is generated.

### Performance

With the code cache warm, module evaluation skips JS parsing and compilation.
Cold start time for a 10-module SDK is reduced from ~50ms to ~5ms.

A persistent code cache (across process restarts) is not supported — the cache
is in-memory only. For persistent startup acceleration, use the
[V8 snapshot](snapshot.md).

---

## TypeScript Transpilation

The loader detects TypeScript source via `looks_like_typescript()` (heuristic:
presence of type annotations, interfaces, generics, etc.) and transpiles it
using `deno_ast` before evaluation. See [Transpiler](transpile.md) for details.

---

## Source

`src/loader.rs`
