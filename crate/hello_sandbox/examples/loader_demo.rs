//! Phase 2 demo — `cargo run --example loader`
//!
//! Demonstrates the fully-working `AllowlistModuleLoader`:
//!   - Building a loader with JS and TypeScript modules
//!   - Security deny-list: `ext:`, `node:`, `https:`, `file:`, bare names
//!   - Relative imports within `sandbox:` modules
//!   - Clone independence of the builder

use deno_core::url::Url;
use deno_core::{
    ModuleLoadOptions, ModuleLoadResponse, ModuleLoader, ModuleSourceCode, RequestedModuleType,
    ResolutionKind,
};
use hello_sandbox::loader::{AllowlistModuleLoader, AllowlistModuleLoaderBuilder};

fn main() {
    println!("=== Phase 2: AllowlistModuleLoader ===\n");

    demo_build();
    demo_resolve();
    demo_deny_list();
    demo_relative_imports();
    demo_clone_independence();
    demo_typescript_transpilation();
}

// ── 1. Building the loader ────────────────────────────────────────────────────

fn demo_build() {
    println!("--- Build ---");

    let loader = AllowlistModuleLoader::new()
        .register("sandbox:math", "export const PI = 3.14159;")
        .register("sandbox:greet.ts", "export const hi = (name: string) => `Hello, ${name}!`;")
        .build()
        .expect("build should succeed");

    println!("  Loader built with 2 modules.");

    // Load sandbox:math
    let code = load_ok(&loader, "sandbox:math");
    println!("  sandbox:math loaded — code: {}", code.trim());
    assert!(code.contains("3.14159"));

    // Load sandbox:greet.ts (transpiled)
    let code = load_ok(&loader, "sandbox:greet.ts");
    let has_type = code.contains(": string");
    println!("  sandbox:greet.ts transpiled — type annotations present: {has_type}");
    assert!(!has_type, "type annotations must be stripped");

    // Missing module
    let missing = Url::parse("sandbox:not-there").unwrap();
    match loader.load(&missing, None, load_opts()) {
        ModuleLoadResponse::Sync(Err(e)) => {
            println!("  sandbox:not-there → error (expected): {e}");
        },
        _ => panic!("expected Sync Err for missing module"),
    }
    println!();
}

// ── 2. Resolve: happy paths ───────────────────────────────────────────────────

fn demo_resolve() {
    println!("--- Resolve (allowed) ---");
    let loader = AllowlistModuleLoader::new().build().unwrap();

    let cases: &[(&str, &str, &str)] = &[
        ("sandbox:math", "sandbox:entry", "sandbox:math"),
        ("sandbox:utils/math", "sandbox:entry", "sandbox:utils/math"),
        ("./helper", "sandbox:utils/entry", "sandbox:utils/helper"),
        ("../math", "sandbox:utils/entry", "sandbox:math"),
    ];

    for (spec, referrer, expected) in cases {
        let resolved = loader
            .resolve(spec, referrer, ResolutionKind::Import)
            .unwrap_or_else(|e| panic!("resolve({spec}, {referrer}) failed: {e}"));
        println!("  resolve({spec:30} from {referrer:25}) → {resolved}");
        assert_eq!(resolved.as_str(), *expected);
    }
    println!();
}

// ── 3. Security deny-list ─────────────────────────────────────────────────────

fn demo_deny_list() {
    println!("--- Deny-list (security) ---");
    let loader = AllowlistModuleLoader::new().build().unwrap();

    let denied: &[(&str, &str)] = &[
        ("ext:core", "sandbox:entry"),
        ("node:fs", "sandbox:entry"),
        ("https://evil.com/malware.js", "sandbox:entry"),
        ("http://evil.com/malware.js", "sandbox:entry"),
        ("file:///etc/passwd", "sandbox:entry"),
        ("lodash", "sandbox:entry"),                    // bare specifier
        ("./escape", "https://attacker.com/script.js"), // relative from non-sandbox
    ];

    for (spec, referrer) in denied {
        match loader.resolve(spec, referrer, ResolutionKind::Import) {
            Err(e) => println!("  ✓ DENIED  {spec:42} → {e}"),
            Ok(url) => panic!("  ✗ ALLOWED (should be denied): {url}"),
        }
    }
    println!();
}

// ── 4. Relative imports ───────────────────────────────────────────────────────

fn demo_relative_imports() {
    println!("--- Relative imports ---");
    let loader = AllowlistModuleLoader::new().build().unwrap();

    // Allowed: referrer is sandbox:
    let ok = loader
        .resolve("./sibling", "sandbox:pkg/main", ResolutionKind::Import)
        .expect("./sibling from sandbox:pkg/main should resolve");
    println!("  ./sibling from sandbox:pkg/main → {ok}");
    assert_eq!(ok.as_str(), "sandbox:pkg/sibling");

    // Denied: referrer is not sandbox:
    let err = loader
        .resolve("./escape", "https://other.com/script.js", ResolutionKind::Import)
        .expect_err("./escape from https: should be denied");
    println!("  ✓ ./escape from https: denied  → {err}");
    println!();
}

// ── 5. Clone independence ─────────────────────────────────────────────────────

fn demo_clone_independence() {
    println!("--- Clone independence ---");

    let base = AllowlistModuleLoaderBuilder::default().register("sandbox:a", "export const a = 1;");

    // Clone before adding more
    let extended = base.clone().register("sandbox:b", "export const b = 2;");

    let loader_base = base.build().unwrap();
    let loader_extended = extended.build().unwrap();

    // base has sandbox:a, NOT sandbox:b
    assert!(is_present(&loader_base, "sandbox:a"), "base must have sandbox:a");
    assert!(!is_present(&loader_base, "sandbox:b"), "base must NOT have sandbox:b");

    // extended has both
    assert!(is_present(&loader_extended, "sandbox:a"), "extended must have sandbox:a");
    assert!(is_present(&loader_extended, "sandbox:b"), "extended must have sandbox:b");

    println!("  base     has sandbox:a = true   has sandbox:b = false  ✓");
    println!("  extended has sandbox:a = true   has sandbox:b = true   ✓");
    println!();
}

// ── 6. TypeScript transpilation ───────────────────────────────────────────────

fn demo_typescript_transpilation() {
    println!("--- TypeScript transpilation on build() ---");

    let ts_modules: &[(&str, &str)] = &[
        (
            "sandbox:types.ts",
            r#"
            interface Point { x: number; y: number; }
            export const origin: Point = { x: 0, y: 0 };
            export const dist = (p: Point): number => Math.sqrt(p.x ** 2 + p.y ** 2);
        "#,
        ),
        (
            "sandbox:greet.tsx",
            r#"
            export const greet = (name: string): string => `Hello, ${name}!`;
        "#,
        ),
    ];

    let mut builder = AllowlistModuleLoaderBuilder::default();
    for (spec, src) in ts_modules {
        builder = builder.register(*spec, *src);
    }
    let loader = builder.build().expect("TS build should succeed");

    for (spec, _) in ts_modules {
        let compiled = load_ok(&loader, spec);
        let has_ts_types = compiled.contains(": number")
            || compiled.contains(": string")
            || compiled.contains("interface ");
        println!("  {spec}  — type annotations stripped: {}", !has_ts_types);
        assert!(!has_ts_types, "TypeScript types must be erased: {compiled}");
    }
    println!();
    println!("All Phase 2 assertions passed.");
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn load_opts() -> ModuleLoadOptions {
    ModuleLoadOptions {
        is_dynamic_import: false,
        is_synchronous: false,
        requested_module_type: RequestedModuleType::None,
    }
}

/// Load a module and return its source as a String; panics on error.
fn load_ok(loader: &AllowlistModuleLoader, spec: &str) -> String {
    let url = Url::parse(spec).unwrap_or_else(|e| panic!("bad specifier '{spec}': {e}"));
    match loader.load(&url, None, load_opts()) {
        ModuleLoadResponse::Sync(Ok(src)) => match src.code {
            ModuleSourceCode::String(s) => String::from_utf8(s.as_bytes().to_vec()).unwrap(),
            ModuleSourceCode::Bytes(b) => String::from_utf8(b.as_bytes().to_vec()).unwrap(),
        },
        ModuleLoadResponse::Sync(Err(e)) => panic!("load({spec}) failed: {e}"),
        ModuleLoadResponse::Async(_) => panic!("unexpected async load for {spec}"),
    }
}

/// Returns `true` if `load()` for `spec` returns a module (not an error).
fn is_present(loader: &AllowlistModuleLoader, spec: &str) -> bool {
    let url = Url::parse(spec).unwrap();
    matches!(loader.load(&url, None, load_opts()), ModuleLoadResponse::Sync(Ok(_)))
}
