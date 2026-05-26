//! Crypto pack -- hashing + random (no secrets exposure).

use deno_core::{op2, OpDecl};
use deno_error::JsErrorBox;

use crate::sdk::SdkExtension;

// ─── Ops ─────────────────────────────────────────────────────────────────────

/// Hash `data` (UTF-8) with `algorithm` ("sha256" | "sha512"). Returns lowercase hex.
#[op2]
#[string]
fn op_crypto_hash(
    #[string] algorithm: String,
    #[string] data: String,
) -> Result<String, JsErrorBox> {
    use sha2::Digest;
    match algorithm.to_lowercase().as_str() {
        "sha256" => {
            let hash = sha2::Sha256::digest(data.as_bytes());
            Ok(hash.iter().map(|b| format!("{b:02x}")).collect())
        },
        "sha512" => {
            let hash = sha2::Sha512::digest(data.as_bytes());
            Ok(hash.iter().map(|b| format!("{b:02x}")).collect())
        },
        _ => Err(JsErrorBox::generic(format!("Unsupported hash algorithm: {algorithm}"))),
    }
}

/// Return `n` cryptographically random bytes as a JSON array of u8.
#[op2]
#[serde]
fn op_crypto_random_bytes(n: u32) -> Vec<u8> {
    use rand::RngCore;
    let mut bytes = vec![0u8; n as usize];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes
}

/// Generate a UUID v4 string.
#[op2]
#[string]
fn op_crypto_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

// ─── Pack ────────────────────────────────────────────────────────────────────

/// Crypto SDK pack -- `hash`, `randomBytes`, `randomUUID`.
pub struct CryptoPack;

impl SdkExtension for CryptoPack {
    fn name(&self) -> &'static str {
        "crypto"
    }

    fn ops(&self) -> Vec<OpDecl> {
        vec![op_crypto_hash(), op_crypto_random_bytes(), op_crypto_uuid()]
    }

    fn esm_files(&self) -> Vec<(&'static str, &'static str)> {
        vec![("sandbox:crypto", include_str!("../../sdk-ts/src/crypto.js"))]
    }

    fn ts_declarations(&self) -> &'static str {
        include_str!("../../sdk-ts/types/crypto.d.ts")
    }
}
