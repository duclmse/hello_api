//! SDK extension packs.
//!
//! # Architecture
//!
//! ```text
//!  SdkExtension (trait)
//!       │
//!       ├── ops()             → Vec<OpDecl>        registered into JsRuntime
//!       ├── esm_files()       → Vec<(&str, &str)>  JS/TS shim source, embedded at compile time
//!       ├── inject_op_state() → ()                 per-slot state deposited into OpState
//!       └── ts_declarations() → &str               .d.ts exposed via `sandbox:<name>.d.ts`
//!
//!  Built-in packs
//!       ├── core    always included  (console, readInput, emit)
//!       ├── kv      host key-value store  (get/set/delete/list)
//!       ├── crypto  hashing + random      (no secrets exposure)
//!       └── http    outbound HTTP         (allowlist-gated)
//!
//!  Custom packs
//!       └── impl SdkExtension on your own struct, then .register(pack)
//! ```
//!
//! All JS shims live in `sdk-ts/src/` and are embedded via `include_str!`.
//! Their TypeScript declarations live in `sdk-ts/types/` and are served by
//! the AllowlistModuleLoader under `sandbox:sdk/<name>.d.ts` — pure metadata
//! for editor tooling; the runtime itself uses the compiled JS shims.

pub mod assert_sdk;
pub mod core_sdk;
pub mod crypto_sdk;
pub mod http_sdk;
pub mod kv_sdk;
pub mod pm_sdk;
pub mod sqlite_sdk;
pub mod timer_sdk;

use deno_core::OpDecl;

// ─── Trait ───────────────────────────────────────────────────────────────────

/// Implement this to add a new capability pack to the sandbox.
///
/// # Example — minimal custom pack
///
/// ```rust,ignore
/// use hello_sandbox::sdk::SdkExtension;
/// use deno_core::{op2, OpDecl};
///
/// pub struct MyPack;
///
/// #[op2(fast)]
/// fn op_hello(#[string] name: String) -> String {
///     format!("Hello, {name}!")
/// }
///
/// impl SdkExtension for MyPack {
///     fn name(&self) -> &'static str { "my_pack" }
///
///     fn ops(&self) -> Vec<OpDecl> {
///         vec![op_hello()]
///     }
///
///     fn esm_files(&self) -> Vec<(&'static str, &'static str)> {
///         vec![("sandbox:my_pack", include_str!("../../sdk-ts/src/my_pack.js"))]
///     }
///
///     fn ts_declarations(&self) -> &'static str {
///         include_str!("../../sdk-ts/types/my_pack.d.ts")
///     }
/// }
/// ```
pub trait SdkExtension: Send + Sync + 'static {
    /// Unique name — used as the `sandbox:<name>` module specifier.
    fn name(&self) -> &'static str;

    /// Ops to register in the `JsRuntime`.
    fn ops(&self) -> Vec<OpDecl>;

    /// ESM shim files: `(specifier, js_source)`.
    ///
    /// - `ext:` specifiers are loaded eagerly as part of the extension.
    /// - `sandbox:` specifiers are registered with the `AllowlistModuleLoader`
    ///   and loaded lazily when the script first imports them.
    fn esm_files(&self) -> Vec<(&'static str, &'static str)>;

    /// TypeScript `.d.ts` declarations for editor tooling.
    /// Served by the loader under `sandbox:<name>.d.ts`.
    fn ts_declarations(&self) -> &'static str {
        ""
    }

    /// If this pack needs to run an ESM file automatically at runtime startup,
    /// return its specifier here (e.g. `"ext:sandbox_core/core.js"`).
    /// Only `CorePack` uses this; all other packs return `None`.
    fn esm_entry_point(&self) -> Option<&'static str> {
        None
    }

    /// Whether this pack is compatible with a pre-baked V8 snapshot.
    ///
    /// All built-in packs return `true` (the default). Packs that rely on
    /// evaluating custom JS at extension load time and cannot tolerate the
    /// snapshot's frozen `globalThis` should return `false` to opt out.
    ///
    /// When a snapshot is loaded, [`SharedRuntime`] skips re-evaluating
    /// `ext:` ESM files and the `esm_entry_point` of all packs — those were
    /// already evaluated during snapshot creation. Ops and per-slot state
    /// (`inject_op_state`) are always re-wired regardless of this flag.
    ///
    /// [`SharedRuntime`]: crate::runtime::SharedRuntime
    fn snapshot_compatible(&self) -> bool {
        true
    }

    /// Returns a JavaScript snippet to inject into `core.js` at the
    /// `// PRE_FREEZE_INJECTION` marker, before `Object.freeze(globalThis)`.
    ///
    /// Use this to install globals (functions, objects) on `globalThis` that
    /// must be visible to scripts. The injection runs in the same scope as
    /// `core.js`, so `ops` (from `globalThis.__sandbox_ops`) is available.
    ///
    /// Returns `None` (default) for packs that do not need pre-freeze globals.
    ///
    /// # Notes
    ///
    /// - The injected code must be 7-bit ASCII (same requirement as all shims).
    /// - In snapshot mode, the injection is baked into the snapshot; at
    ///   runtime the globals are already on `globalThis` after snapshot load.
    /// - In non-snapshot mode, `SharedRuntime::new()` performs a text
    ///   substitution on the `core.js` source before creating the Extension.
    fn pre_freeze_globals(&self) -> Option<&'static str> {
        None
    }

    /// Inject per-slot state into `OpState` before the first run.
    ///
    /// Called once per `SharedRuntime` immediately after construction.
    /// The default no-op implementation is correct for packs that have no
    /// persistent per-slot state (e.g. `CorePack`, `CryptoPack`).
    fn inject_op_state(&self, _op_state: &mut deno_core::OpState) {}

    /// Named exports to auto-import from this pack's `sandbox:` module.
    ///
    /// Returns `Some((specifier, names))` where `specifier` is the `sandbox:`
    /// module specifier and `names` is a slice of exported symbol names that
    /// should be prepended to every user script as an import statement.
    ///
    /// Returns `None` (default) if this pack should not contribute to the
    /// auto-import prelude.
    fn auto_imports(&self) -> Option<(&'static str, &'static [&'static str])> {
        None
    }
}

// ─── Registry ────────────────────────────────────────────────────────────────

/// Collected SDK packs — assembled by `SandboxBuilder` and consumed by
/// `SharedRuntime::new()`.
pub struct SdkRegistry {
    pub(crate) packs: Vec<Box<dyn SdkExtension>>,
}

impl SdkRegistry {
    /// Create an empty registry.
    pub fn empty() -> Self {
        Self { packs: vec![] }
    }

    /// Include the default built-in packs (core + crypto).
    /// HTTP and KV must be opted into explicitly (they carry more risk surface).
    pub fn default_packs() -> Self {
        Self {
            packs: vec![Box::new(crypto_sdk::CryptoPack)],
        }
    }

    /// Register a custom SDK pack.
    pub fn register(mut self, pack: impl SdkExtension) -> Self {
        self.packs.push(Box::new(pack));
        self
    }

    /// Collect all `OpDecl`s from all registered packs.
    pub fn all_ops(&self) -> Vec<OpDecl> {
        self.packs.iter().flat_map(|p| p.ops()).collect()
    }

    /// Collect all ESM shim files: `(specifier, source)`.
    pub fn all_esm_files(&self) -> Vec<(&'static str, &'static str)> {
        self.packs.iter().flat_map(|p| p.esm_files()).collect()
    }

    /// Return the ESM entry-point specifier, if any pack defines one.
    /// Only `CorePack` defines an entry point; others return `None`.
    pub fn esm_entry_point(&self) -> Option<&'static str> {
        self.packs.iter().find_map(|p| p.esm_entry_point())
    }

    /// Collect TypeScript declarations for all packs: `(specifier, dts_source)`.
    pub fn all_declarations(&self) -> Vec<(String, String)> {
        self.packs
            .iter()
            .map(|p| {
                let spec = format!("sandbox:{}.d.ts", p.name());
                (spec, p.ts_declarations().to_string())
            })
            .filter(|(_, dts)| !dts.is_empty())
            .collect()
    }

    /// Call `inject_op_state` on every registered pack.
    ///
    /// Used by `SharedRuntime::new()` to deposit per-slot state (e.g. `KvStore`,
    /// `HttpState`) into `OpState` immediately after the runtime is constructed.
    pub fn inject_all_op_state(&self, op_state: &mut deno_core::OpState) {
        for pack in &self.packs {
            pack.inject_op_state(op_state);
        }
    }

    /// Collect all `pre_freeze_globals()` snippets, joined by newlines.
    ///
    /// Returns an empty string if no pack provides pre-freeze globals.
    /// Used by `SharedRuntime::new()` to apply the `// PRE_FREEZE_INJECTION`
    /// substitution in `core.js`.
    pub fn collect_pre_freeze_globals(&self) -> String {
        self.packs.iter().filter_map(|p| p.pre_freeze_globals()).collect::<Vec<_>>().join("\n")
    }

    /// Build an auto-import prelude from all packs that declare `auto_imports()`.
    ///
    /// Groups exports by specifier. If two packs claim the same export name, the
    /// last-registered pack wins (its specifier is used). Returns one `import`
    /// statement per specifier, or an empty string if no pack declares
    /// auto-imports.
    pub fn all_auto_imports(&self) -> String {
        // name → specifier (last-registered wins for duplicate names)
        let mut name_to_spec: std::collections::HashMap<&'static str, &'static str> =
            Default::default();
        // specifier → ordered names (insertion order preserved via BTreeMap for stability)
        let mut spec_to_names: std::collections::BTreeMap<&'static str, Vec<&'static str>> =
            Default::default();

        for pack in &self.packs {
            if let Some((spec, names)) = pack.auto_imports() {
                for &name in names {
                    if let Some(&prev_spec) = name_to_spec.get(name) {
                        if prev_spec != spec {
                            if let Some(v) = spec_to_names.get_mut(prev_spec) {
                                v.retain(|&n| n != name);
                            }
                        }
                    }
                    name_to_spec.insert(name, spec);
                    spec_to_names.entry(spec).or_default().push(name);
                }
            }
        }

        spec_to_names
            .into_iter()
            .filter(|(_, names)| !names.is_empty())
            .map(|(spec, names)| format!("import {{ {} }} from \"{}\";", names.join(", "), spec))
            .collect::<Vec<_>>()
            .join("\n")
    }
}
