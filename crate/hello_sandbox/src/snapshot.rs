//! V8 snapshot embedding.
//!
//! When a snapshot exists at `snapshot/snapshot.bin` and its version tag
//! matches the current crate, `build.rs` sets `cfg(has_snapshot)` and copies
//! the bytes to `OUT_DIR/snapshot.bin`.  This module embeds those bytes at
//! compile time and exposes them via [`get_snapshot`].
//!
//! If no snapshot is present (first build, or after a version bump) all
//! functions return `None` / empty, and the runtime falls back to the normal
//! cold-start path where `core.js` is evaluated at runtime.
//!
//! # Regenerating the snapshot
//!
//! ```sh
//! cargo run --bin make-snapshot
//! cargo build   # build.rs detects the new file and sets cfg(has_snapshot)
//! ```
//!
//! # Op-count invariant
//!
//! V8 records the number of ops registered at snapshot-creation time in the
//! snapshot's sidecar data (`ops_in_snapshot`).  When the snapshot is loaded
//! at runtime, the JsRuntime **must** have at least that many ops registered
//! or V8 will panic with an index-out-of-bounds on its external-references
//! table.
//!
//! Because the snapshot is built with **all** built-in pack ops (CorePack,
//! CryptoPack, KvPack, HttpPack, SqlitePack, TimerPack), every runtime that
//! loads the snapshot must provide those same ops — regardless of which packs
//! the user actually registered.  [`builtin_ops`] and
//! [`builtin_pre_freeze_globals`] provide the canonical lists used both here
//! and in `make_snapshot.rs`.

use deno_core::OpDecl;

use crate::sdk::assert_sdk::AssertPack;
use crate::sdk::core_sdk::CorePack;
use crate::sdk::crypto_sdk::CryptoPack;
use crate::sdk::http_sdk::{HttpConfig, HttpPack};
use crate::sdk::kv_sdk::KvPack;
use crate::sdk::sqlite_sdk::SqlitePack;
use crate::sdk::timer_sdk::TimerPack;
use crate::sdk::SdkExtension;

#[cfg(has_snapshot)]
static SNAPSHOT_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/snapshot.bin"));

/// Return the pre-baked V8 snapshot bytes, if available.
///
/// Returns `Some` when `cfg(has_snapshot)` is set (i.e. `build.rs` found a
/// valid `snapshot/snapshot.bin` with a matching version tag).
/// Returns `None` otherwise — callers must fall back to evaluating extensions
/// at runtime.
pub fn get_snapshot() -> Option<&'static [u8]> {
    #[cfg(has_snapshot)]
    return Some(SNAPSHOT_BYTES);
    #[cfg(not(has_snapshot))]
    return None;
}

/// Returns ops from **all** built-in packs in the canonical snapshot order.
///
/// This order must be identical to the order used in `make_snapshot.rs` when
/// the snapshot was created.  `SharedRuntime::new()` uses this list as the
/// base op set whenever a snapshot is being loaded, ensuring `ops.len() >=
/// ops_in_snapshot` and preventing V8 from panicking on the external-reference
/// table bounds check.
///
/// Custom user packs add their ops *after* this base set.
pub fn builtin_ops() -> Vec<OpDecl> {
    [
        CorePack.ops(),
        CryptoPack.ops(),
        KvPack::default().ops(),
        HttpPack::new(HttpConfig::default()).ops(),
        SqlitePack::new().ops(),
        TimerPack.ops(),
        AssertPack.ops(),
    ]
    .concat()
}

/// Collects `pre_freeze_globals()` snippets from all built-in packs.
///
/// Used by `make_snapshot.rs` to inject globals (e.g. `setTimeout`) into
/// `core.js` before `Object.freeze(globalThis)`.
pub fn builtin_pre_freeze_globals() -> String {
    let packs: [&dyn SdkExtension; 7] = [
        &CorePack,
        &CryptoPack,
        &KvPack::default(),
        &HttpPack::new(HttpConfig::default()),
        &SqlitePack::new(),
        &TimerPack,
        &AssertPack,
    ];
    packs.iter().filter_map(|p| p.pre_freeze_globals()).collect::<Vec<_>>().join("\n")
}
