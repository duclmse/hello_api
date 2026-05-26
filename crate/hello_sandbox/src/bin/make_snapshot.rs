//! V8 snapshot generator for `hello-sandbox`.
//!
//! Pre-bakes the CorePack bootstrap (`core.js`) — including prototype freezes,
//! `globalThis` freeze, and `__sandbox_ops` installation — into a V8 snapshot
//! blob.  Loading the snapshot at runtime skips this evaluation step, cutting
//! per-slot cold-start time significantly.
//!
//! Writes two files:
//!   - `snapshot/snapshot.bin`  — raw V8 snapshot bytes
//!   - `snapshot/version.txt`   — version tag for cache invalidation
//!
//! The `build.rs` reads the version tag, and if it matches the current crate
//! version, copies `snapshot.bin` to `OUT_DIR` and sets `cfg(has_snapshot)`.
//!
//! # Usage
//!
//! ```sh
//! cargo run --bin make-snapshot
//! cargo build   # build.rs picks up the new snapshot
//! ```
//!
//! Re-run whenever `sdk-ts/src/core.js` changes or after a crate version bump.

use std::borrow::Cow;
use std::sync::Arc;

use deno_core::{Extension, ExtensionFileSource, JsRuntimeForSnapshot, RuntimeOptions};
use hello_sandbox::sdk::core_sdk::CorePack;
use hello_sandbox::sdk::SdkExtension;
use hello_sandbox::snapshot;

fn main() {
    // ── Collect ops from all built-in packs ──────────────────────────────────
    //
    // `snapshot::builtin_ops()` returns all built-in pack ops in the canonical
    // order.  This MUST match the runtime's `SharedRuntime::new()` snapshot
    // path which calls the same function.  Any deviation causes a V8
    // external-reference bounds-check panic when loading the snapshot.
    let all_ops = snapshot::builtin_ops();

    // ── Pre-freeze global injection ───────────────────────────────────────────
    //
    // Inject globals (e.g. setTimeout) into core.js before
    // Object.freeze(globalThis) so they are available to scripts when the
    // snapshot is loaded.
    let pre_freeze = snapshot::builtin_pre_freeze_globals();

    // ── ext: ESM files (evaluated eagerly in snapshot) ────────────────────────
    //
    // Only `core.js` has an `ext:` specifier and must be evaluated here.
    // `sandbox:` shims are lazy-loaded at import time; they are NOT baked in.
    let esm_sources: Vec<ExtensionFileSource> = CorePack
        .esm_files()
        .into_iter()
        .filter(|(spec, _)| spec.starts_with("ext:"))
        .map(|(spec, src)| {
            // Step 1 — inject pre-freeze globals.
            let src_with_injection = if !pre_freeze.is_empty() {
                src.replace("// PRE_FREEZE_INJECTION", &pre_freeze)
            } else {
                src.to_string()
            };

            // Step 2 — trim source at the security boundary.
            //
            // `Object.freeze(globalThis)` and `delete globalThis.Deno` must NOT
            // be evaluated during snapshot creation because V8 cannot restore a
            // snapshot context with a frozen `globalThis`: its internal
            // bootstrapping code needs to add properties to `globalThis` when
            // loading a snapshot, and hitting a frozen object causes a fatal
            // C++ assertion: "Object::AddDataProperty on frozen object".
            //
            // Instead, `SharedRuntime::new()` applies these two operations via
            // `execute_script()` immediately after the runtime is built from the
            // snapshot (see the `// Security post-snapshot setup` comment there).
            let snapshot_src =
                src_with_injection.split("// -- Delete Deno").next().unwrap_or(&src_with_injection);

            ExtensionFileSource::new_computed(spec, Arc::from(snapshot_src))
        })
        .collect();

    let ext = Extension {
        name: "sandbox_sdk",
        ops: Cow::Owned(all_ops),
        esm_files: Cow::Owned(esm_sources),
        esm_entry_point: CorePack.esm_entry_point(),
        ..Default::default()
    };

    // ── Create snapshot runtime and evaluate core.js ──────────────────────────
    //
    // `JsRuntimeForSnapshot` evaluates extensions' ESM entry points and then
    // serialises the resulting V8 heap into a binary blob.
    println!("Creating JsRuntimeForSnapshot and evaluating core.js…");
    let runtime = JsRuntimeForSnapshot::new(RuntimeOptions {
        extensions: vec![ext],
        ..Default::default()
    });

    let snapshot: Box<[u8]> = runtime.snapshot();

    // ── Write output files ────────────────────────────────────────────────────
    let out_dir = "snapshot";
    std::fs::create_dir_all(out_dir).unwrap_or_else(|e| panic!("Cannot create {out_dir}/: {e}"));

    let bin_path = format!("{out_dir}/snapshot.bin");
    std::fs::write(&bin_path, &*snapshot)
        .unwrap_or_else(|e| panic!("Cannot write {bin_path}: {e}"));

    // Version tag: crate version + locked deno_core version.
    // `build.rs` validates this before embedding the snapshot.
    let version = format!("{}/deno_core-0.401.0", env!("CARGO_PKG_VERSION"));
    let ver_path = format!("{out_dir}/version.txt");
    std::fs::write(&ver_path, &version).unwrap_or_else(|e| panic!("Cannot write {ver_path}: {e}"));

    println!("Snapshot written to {bin_path} ({} KiB)", snapshot.len() / 1024);
    println!("Version tag: {version}");
    println!();
    println!("Next steps:");
    println!("  cargo build          # build.rs embeds the snapshot");
    println!("  cargo test           # verify tests still pass with snapshot");
}
