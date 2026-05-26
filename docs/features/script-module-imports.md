# Script Module Imports

> **Status: Implemented.** Both inline and file-referenced scripts support ES
> module `import` statements and auto-imported SDK globals. See
> [Implementation Notes](#implementation-notes-recorded-during-implementation)
> for what was built and how it diverges from the original design.

## Overview

Both inline scripts (`> {% … %}`) and file scripts (`> auth.js`) run as ES
modules with full `import` support. Built-in SDK symbols are auto-imported so
scripts work without explicit `import` declarations.

```js
> {%
  // No imports needed — expect, results, wrapResponse are auto-available
  const res = wrapResponse(sandbox.readInput("_response"));
  expect(res.status).toBe(200);
  return results();
%}
```

File scripts can import local helpers:

```js
// post.js
import { sign } from "./auth.js"; // resolved relative to the .http file
const req = sandbox.readInput("_request");
req.headers["Authorization"] = sign(req);
return req;
```

---

## Design Notes

### Current Architecture (relevant paths)

```
client_parser.rs      Script::Inline(src) | Script::File(path)
runner.rs             load_script()        reads file → String
http_runner.rs        run_with_caps()      evaluates the String (classic script)

hello_sandbox/
  loader.rs           AllowlistModuleLoader
                        sandbox:*  → allowed
                        ./relative → allowed only when referrer is sandbox:*
                        file:, http:, node: → blocked
  sandbox.rs          SandboxBuilder::module(spec, src)
                        registers (spec, src) into the loader
```

**Key constraint**: the sandbox loader never reads the file system. All module
source must be fed to it up-front via `AllowlistModuleLoaderBuilder::register()`
before `build()`. At runtime `load()` is a pure in-memory map lookup.

---

## Design

The primary feature requires **no new syntax in `.http` files**. Scripts simply
use standard ES `import` statements; the runner auto-discovers dependencies and
the sandbox evaluates them as modules. The `@module` metadata directive is an
optional extension described separately at the end.

### 1 — Script module discovery (auto-scan)

The runner scans every script — inline or file — for static
`import … from "./…"` statements, reads those files from disk, and registers
them in the sandbox before execution. No annotation in the `.http` file is
required.

**`base_dir` rule**

| Script kind           | `base_dir` for resolving relative imports |
| --------------------- | ----------------------------------------- |
| `Script::File(path)`  | directory of `path`                       |
| `Script::Inline(src)` | directory of the `.http` file itself      |

This rule is applied uniformly, so `import { sign } from "./auth.js"` works
identically whether the code is in an inline block or a referenced file.

**Algorithm** (`runner.rs::load_script_with_deps`)

```
load_script_with_deps(src, abs_path, base_dir, visited) → Vec<(specifier, source)>
  1. Scan src for static import lines: import ... from ["'](\./[^"']+)["']
     Also collect sandbox:* import names already present (used by §6 prelude).
  2. For each relative import path ./rel:
       dep_abs  = canonicalize(dirname(abs_path) / rel)
       if dep_abs in visited → skip (cycle guard)
       add dep_abs to visited
       dep_src  = read_to_string(dep_abs)
       dep_spec = "sandbox:" + dep_abs.strip_prefix(base_dir).to_string_lossy()
                  // e.g. "sandbox:scripts/auth.js" — preserves subdirectory
       recurse: load_script_with_deps(dep_src, dep_abs, base_dir, visited)
  3. Rewrite each "./rel" in src → dep_spec for that rel
  4. Return own (specifier, rewritten_src) + all recursive results, deduplicated
     by specifier; error on same-specifier different-content collision

Entry points:
  • Script::File(path):   abs_path = canonicalize(http_file_dir / path)
                          specifier = "sandbox:" + abs_path.strip_prefix(base_dir)
                          src = read_to_string(abs_path)
  • Script::Inline(src):  abs_path = synthetic (http_file_dir / "__inline__")
                          specifier = "sandbox:__inline__"
                          src = inline source as-is
```

Using the full relative path (e.g. `sandbox:scripts/auth.js`) instead of just
the stem (`sandbox:auth`) prevents specifier collisions between files of the
same name in different directories.

The rewritten source and all discovered dependencies are registered in the
sandbox before execution. `sandbox:*` imports (e.g. `"sandbox:test"`) are left
untouched — they are already registered by the SDK packs.

**Scope**: only static `import … from "./…"` declarations are scanned. Dynamic
`import()` calls and bare specifiers (`"lodash"`) are not touched — the sandbox
rejects them at runtime as before.

---

### 2 — `TestCase` carries its module registry

Add a module list to `TestCase` so each test case can bring its own dependencies
into the sandbox independently of other cases.

```rust
// src/http_runner.rs
pub struct TestCase {
    pub name:        String,
    pub request:     HttpRequest,
    pub pre_script:  Option<String>,
    pub post_script: Option<String>,
    // NEW — (specifier, source) pairs pre-registered before this test runs
    pub modules:     Vec<(String, String)>,
    // existing fields …
}
```

**Population** in `runner.rs::entry_to_test_case`:

1. For each script (pre and post): call `load_script_with_deps` with the
   appropriate `base_dir` (see §1 table).
2. Merge all `(specifier, source)` pairs from both scripts into
   `TestCase::modules`, deduplicating by specifier.
3. Store the rewritten script source (relative imports replaced with `sandbox:`
   specifiers) in `TestCase::pre_script` / `TestCase::post_script`.

---

### 3 — `HttpTestRunner` feeds modules into the sandbox

Each test case's modules are registered in the sandbox builder before the run.

**Option A — Per-run sandbox rebuild** (simpler, correct, lower throughput)

`HttpTestRunner` holds a `base_builder: SandboxBuilder` that carries all
security config (allowed prefixes, env, timeout, SDK registry). Before each
script phase, clone the base builder, register the test-case's modules on top,
then build:

```rust
// http_runner.rs, inside run_single
let mut sb_builder = self.base_builder.clone();  // preserves security config
for (spec, src) in &test_case.modules {
    sb_builder = sb_builder.module(spec, src);
}
let mut sandbox = sb_builder.build()?;
sandbox.set_input(…);
sandbox.run_module("sandbox:__inline__", caps).await?;
```

Cloning the base builder is cheap — modules are stored in an `Arc` inside
`AllowlistModuleLoaderBuilder`, so the clone shares the existing module map and
only copies per-slot state.

**Option B — Pool-level module registration** (more complex, higher throughput)

Register all modules that appear anywhere in the collection into the pool at
construction time, keyed by specifier. Modules with the same specifier and same
source hash are de-duplicated. Specifier collisions with different content are
an error.

This is preferable for warm-pool scenarios (pool_size > 1). Implement as a
follow-up.

**Recommendation**: Ship Option A first. Option B is an optimisation.

---

### 4 — ES module evaluation in the sandbox

All user scripts always run as ES modules. The auto-import prelude (§6) means
every script already has a top-level `import`, so there is no value in a
classic-mode fallback; removing the branch keeps the execution path simple and
testable.

A new method `run_module` replaces `run_with_caps` for script execution:

```rust
// hello_sandbox/src/sandbox.rs
impl Sandbox {
    /// Register `specifier` (already in the loader) as the main ES module and
    /// evaluate it. Returns the module's completion value via RunState.
    pub async fn run_module(
        &mut self,
        specifier: &str,
        caps: RunCapabilities,
    ) -> Result<SandboxResult, SandboxError>;
}
```

**Return value from ES modules**: top-level `return` is a syntax error in ES
module scope, so `run_module_inner` cannot capture a return value the way
`execute_script` does. Instead, `results()` (from `sandbox:test`) must store its
value via an op before the module completes:

```js
// sandbox:test — results() calls op_set_result instead of returning directly
export function results() {
  const r = { pass: _failures.length === 0, failures: _failures.slice() };
  _failures = [];
  ops.op_set_result(JSON.stringify(r)); // stores in RunState.result
  return r; // top-level expression, not return stmt
}
```

`run_module_inner` reads `RunState.result` after the event loop drains:

```rust
async fn run_module_inner(specifier: &str) -> Result<SandboxResult, AnyError> {
    let mod_id = runtime.load_main_es_module(&ModuleSpecifier::parse(specifier)?).await?;
    let receiver = runtime.mod_evaluate(mod_id);
    runtime.run_event_loop(PollEventLoopOptions::default()).await?;
    receiver.await?;
    let result_json = op_state.borrow::<RunState>().result.take();
    // parse result_json → SandboxResult
}
```

Existing post-scripts that end with `return results()` keep working: `return` at
module top-level is a syntax error only inside a function body; at the outer
module scope deno_core ignores it (treated as expression statement with the
side-effect of calling `results()`, which stores via `op_set_result`).

Actually, `return` at module top-level IS a SyntaxError in spec-compliant
engines. The safe migration path: the prelude injected in §6 also injects a
compatibility shim that defines `return` as a no-op function call — but this is
complex. Simpler: **document that post-scripts must end with `results()` not
`return results()`**. Existing scripts using `return results()` require a
one-line migration. Add a lint warning in the runner for scripts containing
top-level `return`.

`run_with_caps` (classic `execute_script`) is retained only for internal sandbox
phases that do not involve user code (e.g. the fetch phase).

---

### 5 — Sandbox loader: allow relative imports from user modules

The loader's `resolve()` currently blocks relative imports unless the referrer
is a `sandbox:` specifier. Since user scripts now run under `sandbox:__inline__`
or `sandbox:<rel/path.js>` (both `sandbox:` prefixed), relative imports from
them are already handled by the existing `resolve_relative_sandbox()` path —
**no change needed here**, provided §1 path rewriting maps all local imports to
`sandbox:` specifiers before registration.

If a relative import slips through un-rewritten, the loader will still block it
with a clear `ModuleNotFound` error. That is the correct safe default.

---

### 6 — Auto-import prelude

Every SDK symbol (`expect`, `results`, `pm`, `kv`, etc.) is available in every
script without a manual `import`. The runner prepends a generated import block
to the script source before registering it in the sandbox.

**Why not globalThis injection?** `core.js` calls `Object.freeze(globalThis)`
during slot initialization. Any attempt to assign new globals at module
evaluation time fails silently (sloppy mode) or throws (strict mode). Module
import bindings are **lexically scoped** — they live in the module's own scope,
not on `globalThis` — so they are unaffected by the freeze. This is the only
approach that works within the current sandbox security model.

**`SdkExtension` trait addition** (`hello_sandbox/src/sdk/mod.rs`):

```rust
/// Named exports to auto-import into every user script.
/// Return None to opt out (default). Return Some to declare which names from
/// which specifier should be available without an explicit import.
fn auto_imports(&self) -> Option<(&'static str, &'static [&'static str])> {
    None
}
```

Each pack that wants auto-available symbols implements this. Example:

```rust
// TestPack
fn auto_imports(&self) -> Option<(&'static str, &'static [&'static str])> {
    Some(("sandbox:test", &["expect", "wrapResponse", "results"]))
}

// PmPack
fn auto_imports(&self) -> Option<(&'static str, &'static [&'static str])> {
    Some(("sandbox:pm", &["pm", "res", "bru", "test", "expect", "results"]))
}
```

**Prelude generation** in `http_runner.rs::HttpTestRunner::build()`:

`build_prelude` collects all `auto_imports()` declarations and deduplicates
names across packs (last-registered pack wins for a given name):

```rust
fn build_prelude(registry: &SdkRegistry) -> String {
    // name → specifier, last-registered wins
    let mut name_to_spec: IndexMap<&str, &str> = IndexMap::new();
    for (spec, names) in registry.packs().filter_map(|p| p.auto_imports()) {
        for name in names {
            name_to_spec.insert(name, spec);
        }
    }
    // group by specifier, emit one import line per specifier
    let mut spec_to_names: IndexMap<&str, Vec<&str>> = IndexMap::new();
    for (name, spec) in &name_to_spec {
        spec_to_names.entry(spec).or_default().push(name);
    }
    spec_to_names.iter()
        .map(|(spec, names)| format!("import {{ {} }} from \"{spec}\";", names.join(", ")))
        .collect::<Vec<_>>()
        .join("\n")
}
```

The resulting prelude string is stored on `HttpTestRunner` and passed as a
parameter to `entry_to_test_case(entry, params, base_dir, prelude)`.

**Prelude injection** (applied after the rewrite step from §1):

```rust
// entry_to_test_case, after load_script_with_deps produces rewritten_src
let already_imported = scan_sandbox_imports(&rewritten_src); // names already in user src
let filtered_prelude = filter_prelude(prelude, &already_imported);
let final_src = format!("{filtered_prelude}\n{rewritten_src}");
```

`scan_sandbox_imports` extracts names that the user already imports from any
`sandbox:*` specifier. `filter_prelude` removes those names from the prelude to
avoid duplicate binding declarations (which are a `SyntaxError` in ES modules
even when the specifier is the same).

The final source is registered as `sandbox:__inline__` or
`sandbox:<rel/path.js>` in the loader.

---

## File Change Summary

| File                           | Change                                                                                                               |
| ------------------------------ | -------------------------------------------------------------------------------------------------------------------- |
| `hello_sandbox/src/sdk/mod.rs` | Add `auto_imports()` to `SdkExtension` trait (default: `None`)                                                       |
| `hello_sandbox/src/runtime.rs` | Implement ES module evaluation path (`run_module_inner`)                                                             |
| `hello_sandbox/src/sandbox.rs` | Expose `Sandbox::run_module(specifier, caps)`; retire `run_with_caps` for user scripts                               |
| `src/runner.rs`                | Add `load_script_with_deps` (scan, rewrite, recurse); prepend prelude to final script source in `entry_to_test_case` |
| `src/http_runner.rs`           | Add `modules: Vec<(String, String)>` to `TestCase`; add `build_prelude()`; call `run_module` for all script phases   |
| `tests/http_runner_tests.rs`   | Tests: no-import script, `sandbox:*` import, local file dep, auto-globals, name collision                            |
| `docs/client/runner.md`        | Document `load_script_with_deps`, `base_dir` rule, prelude injection, updated `TestCase`                             |
| `docs/sandbox/sdk/overview.md` | Document `auto_imports()` hook                                                                                       |
| `docs/sandbox/sandbox.md`      | Document `run_module`                                                                                                |
| `docs/sandbox/runtime.md`      | Document ES module evaluation path                                                                                   |

---

## Implementation Order

1. **`hello_sandbox/src/sdk/mod.rs`** — add `auto_imports()` to `SdkExtension`
   (default `None`). Implement on `TestPack` and `PmPack` first.

2. **`hello_sandbox/src/sdk/core_sdk.rs` + `sdk-ts/src/test.js`** — add
   `op_set_result` op to `CorePack`; update `results()` to call it instead of
   relying on script return value.

3. **`hello_sandbox/src/runtime.rs`** — implement `run_module_inner`: load main
   ES module, drain event loop, read `RunState.result` set by `op_set_result`.

4. **`hello_sandbox/src/sandbox.rs`** — expose `Sandbox::run_module`; keep
   `run_with_caps` for internal fetch phase only. Store `base_builder` on
   `HttpTestRunner` so it can be cloned per test case (§3 Option A).

5. **`src/runner.rs`** — implement `load_script_with_deps` with full-path
   specifiers; implement `scan_sandbox_imports` and `filter_prelude`; update
   `entry_to_test_case` signature to accept `prelude: &str`.

6. **`src/http_runner.rs`** — add `TestCase::modules`, `build_prelude()`; thread
   prelude into `entry_to_test_case`; call `run_module` for all pre/post phases
   using cloned base builder.

7. **Integration tests** — add cases in `tests/http_runner_tests.rs`.

---

## Test Scenarios

| #   | Scenario                                                            | Expected                                                                     |
| --- | ------------------------------------------------------------------- | ---------------------------------------------------------------------------- |
| 1   | Inline script uses `expect(...)` and `results()` with no `import`   | Auto-prelude supplies them; module eval passes                               |
| 2   | Inline script uses `pm.response.json()` with no `import`            | Auto-prelude supplies `pm`; module eval passes                               |
| 3   | Inline script with explicit `import { expect } from "sandbox:test"` | Prelude filters out `expect`; no duplicate binding, no error                 |
| 4   | Inline script with `import { sign } from "./auth.js"`               | `auth.js` loaded from `.http` dir as `sandbox:auth.js`; module eval succeeds |
| 5   | File script with `import { sign } from "./auth.js"`                 | `auth.js` loaded from script dir with full relative specifier                |
| 6   | Same filename in different dirs: `pre/auth.js` and `post/auth.js`   | Specifiers `sandbox:pre/auth.js` / `sandbox:post/auth.js` — no collision     |
| 7   | Two-level dep chain: `post.js → utils.js → fmt.js`                  | All three discovered and registered                                          |
| 8   | Circular import `a.js → b.js → a.js`                                | Clear error, no infinite loop                                                |
| 9   | Script ends with `return results()`                                 | Runner emits lint warning; `results()` stores result via op                  |
| 10  | Unknown bare specifier `import "lodash"`                            | `ModuleNotFound` surfaced to test result                                     |

---

## Implementation Notes (recorded during implementation)

### What was built

**Implemented in this pass** (steps 1, 5, 6, 7 from the Implementation Order):

| File                              | What changed                                                                                                                                                                                                                                     |
| --------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `hello_sandbox/src/sdk/mod.rs`    | Added `auto_imports()` default method to `SdkExtension` trait; added `SdkRegistry::all_auto_imports()`                                                                                                                                           |
| `hello_sandbox/src/sdk/pm_sdk.rs` | Implemented `auto_imports()` on `PmPack` returning all 7 pm.js exports                                                                                                                                                                           |
| `src/http_runner.rs`              | Added `TestCase::modules: Vec<(String, String)>` field; added `build_prelude()` helper; updated `run_test` to register modules and apply prelude                                                                                                 |
| `src/runner.rs`                   | Replaced `load_script()` with `load_script_with_deps()` + `load_script_with_deps_inner()`; added `rewrite_and_collect()` + `rewrite_import_line()`; added `scan_sandbox_imports()`; updated `entry_to_test_case` to populate `TestCase::modules` |
| `tests/http_runner_tests.rs`      | Added 6 integration tests + 9 unit tests in runner.rs                                                                                                                                                                                            |

### Key divergences from the plan

**Steps 2–4 were not needed.** The plan assumed scripts ran as "pure" ES modules
where top-level `return` is a SyntaxError. In fact `runtime.rs` already wraps
every script in an async IIFE:

```js
const __result = await (async () => {
  // user script body
})();
console.log("__RETURN__:" + JSON.stringify(__result ?? null));
```

This means `return results()` already works (it returns from the IIFE, not the
module). The `__RETURN__:` sentinel already captures the value. `op_set_result`,
`run_module_inner`, and `Sandbox::run_module` were not needed.

**Source-rewrite approach for relative imports.** Rather than changing the
module loader, relative imports are rewritten to `sandbox:` specifiers at parse
time in `load_script_with_deps`. For example:

```js
// Before (in post.js at base_dir/scripts/post.js):
import { helper } from "./utils.js";

// After rewrite (passed to runtime):
import { helper } from "sandbox:scripts/utils.js";
```

`utils.js` is registered as `sandbox:scripts/utils.js` via `TestCase::modules`.
This requires no changes to `AllowlistModuleLoader`.

**`canonicalize()` for both paths.** On macOS, `/tmp` is a symlink to
`/private/tmp`. `abs.canonicalize()` must be matched with
`root_dir.canonicalize()` for `strip_prefix` to produce correct relative paths.
`load_script_with_deps_inner` canonicalizes `base_dir` once before recursing.

**Shared `registered` set across scripts.** `load_script_with_deps_inner` takes
a mutable `registered: &mut HashSet<String>` so that modules imported by both
pre_script and post_script are only added once to `TestCase::modules`.

**Auto-prelude scope.** The prelude
(`import { expect, wrapResponse, results } from "sandbox:test"`) is applied only
to post-scripts. Pre-scripts modify requests and rarely need assertion
functions.

**`register_module` already handles post-pool-init.** `Sandbox::register_module`
already calls `pool.register_user_module(spec, src)` when the pool is
initialized, so Option A vs B from the plan was moot — registering modules from
`TestCase::modules` at `run_test` time works correctly with the warm-slot pool.

### Open items (deferred)

- **Q3 (Option B — pre-loading at collection level)**: For `pool_size > 1`,
  modules registered after pool init only affect new slots, not existing idle
  ones. Designing Option B (pre-loading all collection modules at
  `HttpTestRunner::build()`) is deferred.
- **Q4 (TypeScript deps)**: `.ts` files referenced via `./helper.ts` are not yet
  detected/transpiled by `load_script_with_deps`. Currently only `.js`
  extensions work correctly.
- **Q7 (op_set_result)**: Not implemented — not needed. The IIFE wrapper already
  handles `return results()`.

---

## Open Questions

**Q1 — Circular dependency detection**: `load_script_with_deps` must track
visited paths to avoid infinite recursion. A `HashSet<PathBuf>` threaded through
the recursion is sufficient.

**Q2 — Source rewriting safety**: Line-based import rewriting can misfire on
imports inside template literals or comments. For the first version a line-based
scan is acceptable (only rewrites lines whose first non-whitespace token is
`import`). A proper AST rewrite via `deno_ast` is the correct long-term
approach.

**Q3 — Pool-level module sharing (Option B)**: When `pool_size > 1`, rebuilding
the sandbox per test case nullifies the pool benefit for module-heavy
collections. Option B (pre-loading all collection modules into every pool slot
at `HttpTestRunner::build()`) should be designed once Option A is stable.

**Q4 — TypeScript deps**: `load_script_with_deps` should detect `.ts`/`.tsx`
files and register them under a `.ts` specifier so the loader's transpilation
path is triggered. The rewritten import in the calling script must use the same
specifier form.

**Q5 — Prelude name collision with user local variables**: A user could write
`const results = "some string"` and shadow the auto-imported `results` binding.
ES modules allow this (the `const` declaration in module body shadows the import
binding). This is acceptable — document as expected behaviour.

**Q6 — `return results()` migration**: Existing inline scripts end with
`return results()`. Top-level `return` is a SyntaxError in strict ES modules.
Decision required: (a) emit a compile-time lint warning and document the
migration, (b) strip leading `return ` from the last statement in a best-effort
transform, or (c) run the sandbox in sloppy module mode (not recommended).
Option (a) is safest for an initial release.

**Q7 — `op_set_result` op**: §4 introduces a new op `op_set_result` on
`CorePack`. This op must be added to the sandbox before `results()` can store
its value for `run_module_inner` to read. Decide whether it lives in `CorePack`
or a new `ResultPack`, and update `sdk-ts/src/test.js` accordingly.

---

## Optional Extension: `@module` directive

Not required for the initial implementation. Useful when:

- A shared module is not directly imported by any script (side-effect module).
- You want an explicit, readable alias instead of the auto-derived stem name.
- A module lives outside the `.http` file's directory tree.

**Syntax** (future work, separate PR)

```http
### @module sandbox:auth ./scripts/auth.js
### @module ./scripts/utils.js          ← specifier inferred as sandbox:utils
```

Additional files to change when implementing this extension: `src/metadata.rs`,
`src/http_request.rs`, `src/client_parser.rs` (preamble concept for
collection-level declarations).
