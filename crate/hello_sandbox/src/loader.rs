use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use deno_core::{
    ModuleLoadOptions, ModuleLoadReferrer, ModuleLoadResponse, ModuleLoader, ModuleSource,
    ModuleSourceCode, ModuleSpecifier, ModuleType, ResolutionKind,
};
use deno_error::JsErrorBox;
use sha2::Digest;

use crate::transpile::transpile;
use crate::SandboxError;

// ─── CodeCache ────────────────────────────────────────────────────────────────

/// Version sentinel embedded in every cache key to auto-invalidate bytecode
/// blobs whenever `hello-sandbox` or `deno_core` is updated.
const CACHE_VERSION_TAG: &str =
    concat!("hello-sandbox/", env!("CARGO_PKG_VERSION"), "+deno_core-0.380.1",);

/// Compute the 64-bit cache key for a compiled module.
///
/// Incorporates the specifier, compiled JS source, and a version tag so the
/// cache self-invalidates on crate or runtime upgrades.
fn source_hash(specifier: &str, compiled_js: &str) -> u64 {
    let mut h = sha2::Sha256::new();
    h.update(specifier.as_bytes());
    h.update(b"\x00");
    h.update(compiled_js.as_bytes());
    h.update(b"\x00");
    h.update(CACHE_VERSION_TAG.as_bytes());
    let result = h.finalize();
    u64::from_le_bytes(result[..8].try_into().expect("sha256 is at least 8 bytes"))
}

/// In-memory V8 bytecode cache shared across all pool slots.
///
/// Keyed by a 64-bit hash incorporating the module specifier, compiled JS
/// source, and a version tag.  V8 validates bytecode against the hash before
/// using it; a mismatch triggers a transparent recompile and a fresh
/// [`code_cache_ready`] callback to refresh the stored blob.
///
/// Create one with [`CodeCache::new_shared`] and attach it to a loader
/// builder via [`AllowlistModuleLoaderBuilder::with_code_cache`].
pub struct CodeCache {
    entries: HashMap<u64, Vec<u8>>,
}

impl CodeCache {
    /// Create a new, empty cache wrapped in `Arc<Mutex<…>>` for sharing
    /// across pool slots.
    pub fn new_shared() -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            entries: HashMap::new(),
        }))
    }

    /// Number of cached bytecode blobs currently held.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `true` when the cache holds no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Remove the entry for `hash`, if present.
    fn purge(&mut self, hash: u64) {
        self.entries.remove(&hash);
    }
}

// ─── Registered module entry ──────────────────────────────────────────────────

/// A compiled module entry stored in the built loader.
#[derive(Clone, Debug)]
struct CompiledModule {
    /// Compiled (possibly transpiled from TS) JavaScript source.
    source: String,
    /// Precomputed cache key for this (specifier, source, version) triple.
    hash: u64,
}

// ─── AllowlistModuleLoader ────────────────────────────────────────────────────

/// A module loader that only resolves `sandbox:` scheme specifiers.
///
/// All other schemes (`ext:`, `node:`, `https:`, `file:`, bare names) are
/// hard-denied with an error message — never silently ignored.
///
/// Modules are registered by the host before the runtime starts.  At `build()`
/// time every TypeScript entry is transpiled to JavaScript and a 64-bit cache
/// hash is precomputed.  Subsequent loads are served from the compiled
/// in-memory map.
///
/// When a [`CodeCache`] is attached (via
/// [`AllowlistModuleLoaderBuilder::with_code_cache`]), the loader:
/// - Provides pre-compiled V8 bytecode on cache hits (skips parse + compile).
/// - Stores fresh bytecode via [`code_cache_ready`] after first compilation.
/// - Purges stale entries via [`purge_and_prevent_code_cache`].
#[derive(Clone)]
pub struct AllowlistModuleLoader {
    /// Maps `sandbox:` specifier string → compiled JS source + cache hash.
    modules: Arc<HashMap<String, CompiledModule>>,
    /// Shared bytecode cache — `None` disables caching.
    cache: Option<Arc<Mutex<CodeCache>>>,
}

impl AllowlistModuleLoader {
    /// Start building a new loader.
    #[allow(clippy::new_ret_no_self)]
    pub fn new() -> AllowlistModuleLoaderBuilder {
        AllowlistModuleLoaderBuilder::default()
    }
}

// ─── AllowlistModuleLoaderBuilder ─────────────────────────────────────────────

/// Fluent builder for [`AllowlistModuleLoader`].
///
/// Implements `Clone` so the pool can keep the original builder and clone it
/// per-slot when creating fresh runtimes.  The `Arc<Mutex<CodeCache>>` field
/// is cheap to clone — all clones share the same underlying cache.
#[derive(Clone, Default)]
pub struct AllowlistModuleLoaderBuilder {
    entries: HashMap<String, RegisteredEntry>,
    cache: Option<Arc<Mutex<CodeCache>>>,
}

#[derive(Clone, Debug)]
struct RegisteredEntry {
    source: String,
    is_typescript: bool,
}

impl AllowlistModuleLoaderBuilder {
    /// Register a module under a `sandbox:` specifier.
    ///
    /// If `specifier` ends with `.ts` or `.tsx` the source will be transpiled
    /// to JavaScript during [`build()`](Self::build).
    pub fn register(mut self, specifier: impl Into<String>, source: impl Into<String>) -> Self {
        let spec = specifier.into();
        let is_typescript = spec.ends_with(".ts") || spec.ends_with(".tsx");
        self.entries.insert(
            spec,
            RegisteredEntry {
                source: source.into(),
                is_typescript,
            },
        );
        self
    }

    /// Attach a shared bytecode cache.
    ///
    /// All loaders built from the same builder instance (including those built
    /// after cloning) will share the same cache `Arc`.  Typically you create
    /// one cache via [`CodeCache::new_shared`] and share it across the pool.
    pub fn with_code_cache(mut self, cache: Arc<Mutex<CodeCache>>) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Transpile all TypeScript entries and produce the immutable loader.
    ///
    /// Cache hashes are computed here (once per build), so `load()` never
    /// needs to re-hash sources.
    pub fn build(self) -> Result<AllowlistModuleLoader, SandboxError> {
        let mut compiled = HashMap::with_capacity(self.entries.len());
        for (spec, entry) in self.entries {
            let js = transpile(&spec, &entry.source, entry.is_typescript)?;
            let hash = source_hash(&spec, &js);
            compiled.insert(spec, CompiledModule { source: js, hash });
        }
        Ok(AllowlistModuleLoader {
            modules: Arc::new(compiled),
            cache: self.cache,
        })
    }
}

// ─── Internal helpers ─────────────────────────────────────────────────────────

/// Resolve a relative specifier (`./foo`, `../bar`) against a `sandbox:` referrer.
///
/// `sandbox:` URLs use opaque paths so the standard `Url::join` does not work.
/// We resolve the path segments manually and re-attach the `sandbox:` prefix.
fn resolve_relative_sandbox(
    referrer: &str,
    specifier: &str,
) -> Result<ModuleSpecifier, JsErrorBox> {
    // Strip the "sandbox:" prefix to get just the path.
    let base_path = referrer.strip_prefix("sandbox:").unwrap_or(referrer);

    // The "directory" of the referrer: everything up to and including the last "/".
    let dir = match base_path.rfind('/') {
        Some(idx) => &base_path[..=idx],
        None => "", // referrer has no directory component
    };

    // Concatenate dir + relative specifier, then normalise `..` and `.` segments.
    let raw = format!("{dir}{}", specifier.trim_start_matches("./"));
    let normalised = normalise_path(&raw);

    ModuleSpecifier::parse(&format!("sandbox:{normalised}"))
        .map_err(|e| JsErrorBox::generic(e.to_string()))
}

/// Collapse `.` and `..` segments in a slash-separated path string.
fn normalise_path(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "." => {},
            ".." => {
                parts.pop();
            },
            s => parts.push(s),
        }
    }
    parts.join("/")
}

// ─── ModuleLoader impl ────────────────────────────────────────────────────────

impl ModuleLoader for AllowlistModuleLoader {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
        _kind: ResolutionKind,
    ) -> Result<ModuleSpecifier, JsErrorBox> {
        // Hard-deny known dangerous schemes before any other check.
        for denied in &["ext:", "node:", "https:", "http:", "file:"] {
            if specifier.starts_with(denied) {
                return Err(JsErrorBox::generic(format!(
                    "Module scheme '{}' is not allowed in the sandbox. \
                     Only 'sandbox:' specifiers are permitted.",
                    denied.trim_end_matches(':')
                )));
            }
        }

        // Allow absolute `sandbox:` specifiers.
        if specifier.starts_with("sandbox:") {
            return ModuleSpecifier::parse(specifier)
                .map_err(|e| JsErrorBox::generic(e.to_string()));
        }

        // Allow relative specifiers (`./helper`, `../utils`) *only* when the
        // referrer is itself a `sandbox:` module.
        if specifier.starts_with('.') {
            // Verify the referrer is a sandbox: URL.
            let referrer_url =
                ModuleSpecifier::parse(referrer).map_err(|e| JsErrorBox::generic(e.to_string()))?;

            if referrer_url.scheme() != "sandbox" {
                return Err(JsErrorBox::generic(format!(
                    "Relative import '{specifier}' is not allowed when the referrer \
                     '{referrer}' is outside the sandbox."
                )));
            }

            // `sandbox:` uses opaque paths (no `//`), so `Url::join` won't work.
            // Resolve manually by treating the path component like a Unix file path.
            let resolved = resolve_relative_sandbox(referrer, specifier)?;
            return Ok(resolved);
        }

        // Everything else (bare names, unknown schemes) is denied.
        Err(JsErrorBox::generic(format!(
            "Module '{specifier}' is not in the sandbox allowlist. \
             Only 'sandbox:' specifiers are permitted."
        )))
    }

    fn load(
        &self,
        module_specifier: &ModuleSpecifier,
        _maybe_referrer: Option<&ModuleLoadReferrer>,
        _options: ModuleLoadOptions,
    ) -> ModuleLoadResponse {
        let spec_str = module_specifier.as_str();
        match self.modules.get(spec_str) {
            Some(module) => {
                // Build SourceCodeCacheInfo when a cache is configured.
                //
                // We always supply `hash` (even on a cache miss) so V8 knows
                // which identity tag to pass back to `code_cache_ready` after
                // it finishes compiling.  On a cache hit we also supply `data`
                // so V8 can skip parse + compile entirely.
                let code_cache_info = self.cache.as_ref().map(|arc| {
                    use deno_core::SourceCodeCacheInfo;
                    let data = arc
                        .lock()
                        .ok()
                        .and_then(|c| c.entries.get(&module.hash).map(|b| Cow::Owned(b.clone())));
                    SourceCodeCacheInfo {
                        hash: module.hash,
                        data,
                    }
                });

                ModuleLoadResponse::Sync(Ok(ModuleSource::new(
                    ModuleType::JavaScript,
                    ModuleSourceCode::String(module.source.clone().into()),
                    module_specifier,
                    code_cache_info,
                )))
            },
            None => ModuleLoadResponse::Sync(Err(JsErrorBox::generic(format!(
                "Module '{}' is not registered in the sandbox allowlist.",
                spec_str
            )))),
        }
    }

    /// Store freshly-compiled V8 bytecode for future loads.
    ///
    /// Called by deno_core after V8 compiles a module for the first time (or
    /// recompiles after a hash mismatch).  The `hash` matches the one we put in
    /// [`SourceCodeCacheInfo`] so it maps directly to our cache key.
    fn code_cache_ready(
        &self,
        _module_specifier: ModuleSpecifier,
        hash: u64,
        code_cache: &[u8],
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()>>> {
        if let Some(arc) = &self.cache {
            if let Ok(mut c) = arc.lock() {
                c.entries.insert(hash, code_cache.to_vec());
            }
        }
        Box::pin(async {})
    }

    /// Remove a stale or invalid cache entry for `module_specifier`.
    ///
    /// Called by V8 (via deno_core) when a cached bytecode blob is known to be
    /// bad — e.g. after a V8 warning about deprecated import assertions.
    fn purge_and_prevent_code_cache(&self, module_specifier: &str) {
        if let Some(arc) = &self.cache {
            if let (Some(module), Ok(mut c)) = (self.modules.get(module_specifier), arc.lock()) {
                c.purge(module.hash);
            }
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn build_loader(mods: &[(&str, &str)]) -> AllowlistModuleLoader {
        let mut builder = AllowlistModuleLoaderBuilder::default();
        for (spec, src) in mods {
            builder = builder.register(*spec, *src);
        }
        builder.build().expect("build should succeed")
    }

    fn resolve(
        loader: &AllowlistModuleLoader,
        spec: &str,
        referrer: &str,
    ) -> Result<String, String> {
        loader
            .resolve(spec, referrer, ResolutionKind::Import)
            .map(|u| u.to_string())
            .map_err(|e| e.to_string())
    }

    // ── resolve() happy paths ─────────────────────────────────────────────────

    #[test]
    fn sandbox_specifier_resolves() {
        let loader = build_loader(&[]);
        let url = resolve(&loader, "sandbox:math", "sandbox:entry").unwrap();
        assert_eq!(url, "sandbox:math");
    }

    #[test]
    fn sandbox_specifier_with_path_resolves() {
        let loader = build_loader(&[]);
        let url = resolve(&loader, "sandbox:utils/math", "sandbox:entry").unwrap();
        assert_eq!(url, "sandbox:utils/math");
    }

    #[test]
    fn relative_resolves_when_referrer_is_sandbox() {
        let loader = build_loader(&[]);
        let url = resolve(&loader, "./helper", "sandbox:utils/entry").unwrap();
        assert_eq!(url, "sandbox:utils/helper");
    }

    // ── resolve() deny-list ───────────────────────────────────────────────────

    #[test]
    fn ext_scheme_is_rejected() {
        let loader = build_loader(&[]);
        let err = resolve(&loader, "ext:core", "sandbox:entry").unwrap_err();
        assert!(err.contains("ext"), "error should mention the denied scheme");
    }

    #[test]
    fn node_scheme_is_rejected() {
        let loader = build_loader(&[]);
        let err = resolve(&loader, "node:fs", "sandbox:entry").unwrap_err();
        assert!(err.contains("node"));
    }

    #[test]
    fn https_scheme_is_rejected() {
        let loader = build_loader(&[]);
        let err = resolve(&loader, "https://evil.com/malware.js", "sandbox:entry").unwrap_err();
        assert!(err.contains("https"));
    }

    #[test]
    fn file_scheme_is_rejected() {
        let loader = build_loader(&[]);
        let err = resolve(&loader, "file:///etc/passwd", "sandbox:entry").unwrap_err();
        assert!(err.contains("file"));
    }

    #[test]
    fn bare_specifier_is_rejected() {
        let loader = build_loader(&[]);
        let err = resolve(&loader, "lodash", "sandbox:entry").unwrap_err();
        assert!(
            err.contains("allowlist") || err.contains("not allowed") || err.contains("sandbox:")
        );
    }

    #[test]
    fn relative_from_non_sandbox_referrer_is_rejected() {
        let loader = build_loader(&[]);
        // referrer is NOT a sandbox: URL
        let err = resolve(&loader, "./helper", "https://attacker.com/script.js").unwrap_err();
        assert!(
            err.contains("not allowed") || err.contains("outside"),
            "expected deny message, got: {err}"
        );
    }

    // ── load() ────────────────────────────────────────────────────────────────

    #[test]
    fn load_registered_js_module() {
        let src = "export const PI = 3.14;";
        let loader = build_loader(&[("sandbox:math", src)]);
        let specifier = ModuleSpecifier::parse("sandbox:math").unwrap();

        let options = ModuleLoadOptions {
            is_dynamic_import: false,
            is_synchronous: false,
            requested_module_type: deno_core::RequestedModuleType::None,
        };
        let response = loader.load(&specifier, None, options);

        match response {
            ModuleLoadResponse::Sync(Ok(source)) => {
                let code = match source.code {
                    ModuleSourceCode::String(s) => s.as_bytes().to_vec(),
                    ModuleSourceCode::Bytes(b) => b.as_bytes().to_vec(),
                };
                let text = String::from_utf8(code).unwrap();
                assert!(text.contains("3.14"));
            },
            _ => panic!("expected Sync Ok response"),
        }
    }

    #[test]
    fn load_unregistered_module_returns_error() {
        let loader = build_loader(&[]);
        let specifier = ModuleSpecifier::parse("sandbox:missing").unwrap();
        let options = ModuleLoadOptions {
            is_dynamic_import: false,
            is_synchronous: false,
            requested_module_type: deno_core::RequestedModuleType::None,
        };
        let response = loader.load(&specifier, None, options);
        match response {
            ModuleLoadResponse::Sync(Err(_)) => {},
            _ => panic!("expected Sync Err response"),
        }
    }

    // ── TypeScript transpilation on build() ───────────────────────────────────

    #[test]
    fn ts_module_is_transpiled_on_build() {
        let ts_src = "export const greet = (name: string): string => `Hello, ${name}!`;";
        let loader = build_loader(&[("sandbox:greet.ts", ts_src)]);

        // The map key is still the original specifier.
        let compiled = loader.modules.get("sandbox:greet.ts").expect("module should be present");
        // Type annotation should be gone after transpilation.
        assert!(
            !compiled.source.contains(": string"),
            "compiled output should not contain TypeScript type annotations"
        );
        assert!(compiled.source.contains("Hello"), "function body should be preserved");
    }

    // ── Clone independence ────────────────────────────────────────────────────

    #[test]
    fn cloning_builder_produces_independent_copies() {
        let builder =
            AllowlistModuleLoaderBuilder::default().register("sandbox:a", "export const a = 1;");

        let builder2 = builder.clone().register("sandbox:b", "export const b = 2;");

        let loader1 = builder.build().unwrap();
        let loader2 = builder2.build().unwrap();

        assert!(loader1.modules.contains_key("sandbox:a"));
        assert!(!loader1.modules.contains_key("sandbox:b"), "clone should not bleed into original");

        assert!(loader2.modules.contains_key("sandbox:a"));
        assert!(loader2.modules.contains_key("sandbox:b"));
    }

    // ── CodeCache ─────────────────────────────────────────────────────────────

    #[test]
    fn code_cache_starts_empty() {
        let cache = CodeCache::new_shared();
        assert!(cache.lock().unwrap().is_empty());
    }

    #[test]
    fn code_cache_ready_stores_bytes() {
        let cache = CodeCache::new_shared();
        let loader = AllowlistModuleLoaderBuilder::default()
            .register("sandbox:a", "export const a = 1;")
            .with_code_cache(cache.clone())
            .build()
            .unwrap();

        // Simulate V8 calling code_cache_ready with bytecode for the module.
        let hash = loader.modules.get("sandbox:a").unwrap().hash;
        let spec = ModuleSpecifier::parse("sandbox:a").unwrap();
        let fake_bytecode = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let fut = loader.code_cache_ready(spec, hash, &fake_bytecode);
        tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(fut);

        let c = cache.lock().unwrap();
        assert_eq!(c.len(), 1);
        assert_eq!(c.entries.get(&hash).unwrap(), &fake_bytecode);
    }

    #[test]
    fn load_provides_cached_bytes_on_hit() {
        use deno_core::SourceCodeCacheInfo;

        let cache = CodeCache::new_shared();
        let loader = AllowlistModuleLoaderBuilder::default()
            .register("sandbox:a", "export const a = 1;")
            .with_code_cache(cache.clone())
            .build()
            .unwrap();

        let hash = loader.modules.get("sandbox:a").unwrap().hash;
        // Pre-populate the cache with fake bytecode.
        cache.lock().unwrap().entries.insert(hash, vec![1, 2, 3, 4]);

        let specifier = ModuleSpecifier::parse("sandbox:a").unwrap();
        let opts = ModuleLoadOptions {
            is_dynamic_import: false,
            is_synchronous: false,
            requested_module_type: deno_core::RequestedModuleType::None,
        };
        match loader.load(&specifier, None, opts) {
            ModuleLoadResponse::Sync(Ok(src)) => {
                let info: &SourceCodeCacheInfo =
                    src.code_cache.as_ref().expect("cache info should be present");
                assert_eq!(info.hash, hash);
                assert_eq!(
                    info.data.as_deref(),
                    Some(&[1u8, 2, 3, 4][..]),
                    "cached bytes should be returned in data field"
                );
            },
            _other => panic!("expected Sync Ok response"),
        }
    }

    #[test]
    fn load_provides_hash_but_no_data_on_cache_miss() {
        let cache = CodeCache::new_shared();
        let loader = AllowlistModuleLoaderBuilder::default()
            .register("sandbox:b", "export const b = 2;")
            .with_code_cache(cache)
            .build()
            .unwrap();

        let specifier = ModuleSpecifier::parse("sandbox:b").unwrap();
        let opts = ModuleLoadOptions {
            is_dynamic_import: false,
            is_synchronous: false,
            requested_module_type: deno_core::RequestedModuleType::None,
        };
        match loader.load(&specifier, None, opts) {
            ModuleLoadResponse::Sync(Ok(src)) => {
                let info = src.code_cache.expect("cache info should be present (even on miss)");
                assert_ne!(info.hash, 0, "hash should be non-zero");
                assert!(info.data.is_none(), "data should be None on cache miss");
            },
            _other => panic!("expected Sync Ok response"),
        }
    }

    #[test]
    fn purge_removes_entry() {
        let cache = CodeCache::new_shared();
        let loader = AllowlistModuleLoaderBuilder::default()
            .register("sandbox:c", "export const c = 3;")
            .with_code_cache(cache.clone())
            .build()
            .unwrap();

        let hash = loader.modules.get("sandbox:c").unwrap().hash;
        cache.lock().unwrap().entries.insert(hash, vec![9, 8, 7]);

        loader.purge_and_prevent_code_cache("sandbox:c");

        assert!(cache.lock().unwrap().entries.get(&hash).is_none());
    }

    #[test]
    fn hash_changes_when_source_changes() {
        let h1 = source_hash("sandbox:x", "export const x = 1;");
        let h2 = source_hash("sandbox:x", "export const x = 2;");
        assert_ne!(h1, h2, "different sources must produce different hashes");
    }

    #[test]
    fn hash_changes_for_different_specifiers() {
        let src = "export const v = 1;";
        let h1 = source_hash("sandbox:a", src);
        let h2 = source_hash("sandbox:b", src);
        assert_ne!(h1, h2, "different specifiers must produce different hashes");
    }
}
