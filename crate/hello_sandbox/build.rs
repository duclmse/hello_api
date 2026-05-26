/// Build script for hello-sandbox.
///
/// Detects whether a pre-baked V8 snapshot exists at `snapshot/snapshot.bin`
/// and whether its version tag matches the current crate version.
///
/// If both checks pass the snapshot bytes are copied to `OUT_DIR/snapshot.bin`
/// and `cfg(has_snapshot)` is set so that `src/snapshot.rs` can embed them
/// with `include_bytes!`.
///
/// Regenerate the snapshot with:
/// ```sh
/// cargo run --bin make-snapshot
/// ```
fn main() {
    // Declare the custom cfg so rustc doesn't warn about `#[cfg(has_snapshot)]`.
    println!("cargo::rustc-check-cfg=cfg(has_snapshot)");
    // Re-run this script when the snapshot files change.
    println!("cargo:rerun-if-changed=snapshot/snapshot.bin");
    println!("cargo:rerun-if-changed=snapshot/version.txt");

    let pkg_version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let expected_version = format!("{pkg_version}/deno_core-0.401.0");

    // Read and validate version tag.
    let version_path = "snapshot/version.txt";
    let Ok(version_content) = std::fs::read_to_string(version_path) else {
        return; // No snapshot — skip silently.
    };
    if version_content.trim() != expected_version {
        eprintln!(
            "cargo:warning=Snapshot version mismatch \
             (expected {expected_version:?}, got {:?}) — \
             run `cargo run --bin make-snapshot` to regenerate.",
            version_content.trim()
        );
        return;
    }

    // Read snapshot bytes.
    let bin_path = "snapshot/snapshot.bin";
    let Ok(snapshot_bytes) = std::fs::read(bin_path) else {
        eprintln!("cargo:warning=snapshot/version.txt found but snapshot/snapshot.bin is missing.");
        return;
    };
    if snapshot_bytes.is_empty() {
        eprintln!("cargo:warning=snapshot/snapshot.bin is empty — skipping.");
        return;
    }

    // Copy to OUT_DIR so `include_bytes!` can reach it at compile time.
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let dest = format!("{out_dir}/snapshot.bin");
    std::fs::write(&dest, &snapshot_bytes)
        .unwrap_or_else(|e| panic!("Failed to write {dest}: {e}"));

    // Signal to rustc that the snapshot is available.
    println!("cargo:rustc-cfg=has_snapshot");
}
