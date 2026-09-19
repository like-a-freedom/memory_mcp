//! Credential material helpers for local admin auth and client provisioning.
//!
//! Provides HMAC-based key fingerprinting, random byte generation, and
//! API key material generation. Independent of control-plane adapters.

use hmac::{Hmac, KeyInit, Mac};
use rand_core::RngCore;
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Compute an HMAC-SHA256 fingerprint of data under a purpose-separated label.
///
/// The label prevents cross-purpose key reuse even if the underlying key
/// is the same. Returns the raw 32-byte MAC output.
pub fn hmac_fingerprint(key: &[u8; 32], label: &[u8], data: &[u8]) -> [u8; 32] {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(label);
    mac.update(data);
    mac.finalize().into_bytes().into()
}

/// Fill a buffer with cryptographically secure random bytes.
pub fn fill_random_bytes(buf: &mut [u8]) {
    rand_core::OsRng.fill_bytes(buf);
}

/// Generate a random 32-byte value.
pub fn random_32() -> [u8; 32] {
    let mut buf = [0u8; 32];
    fill_random_bytes(&mut buf);
    buf
}

/// Generate API key material: (key_id, verifier, full_credential).
///
/// - `key_id`: 24-char base64url identifier (no padding)
/// - `verifier`: HMAC-SHA256 of the raw credential under the pepper
/// - `full_credential`: 32-byte random token, hex-encoded with `mem_sk_` prefix
pub fn generate_api_key_material(pepper: &[u8]) -> (String, [u8; 32], String) {
    let raw = random_32();

    // Key ID: first 18 bytes, base64url encoded (24 chars)
    let key_id_bytes = &raw[..18];
    let key_id = base64url_encode(key_id_bytes);

    // Verifier: HMAC of raw credential under pepper
    let mut mac = HmacSha256::new_from_slice(pepper).expect("HMAC accepts any key length");
    mac.update(&raw);
    let verifier: [u8; 32] = mac.finalize().into_bytes().into();

    // Full credential: hex-encoded with prefix
    let full_credential = format!("mem_sk_{}", hex::encode(raw));

    (key_id, verifier, full_credential)
}

/// Base64url encode without padding (RFC 4648 §5).
fn base64url_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_fingerprint_deterministic() {
        let key = random_32();
        let data = b"test data";
        let fp1 = hmac_fingerprint(&key, b"label", data);
        let fp2 = hmac_fingerprint(&key, b"label", data);
        assert_eq!(fp1, fp2);
    }

    #[test]
    fn hmac_fingerprint_label_separation() {
        let key = random_32();
        let data = b"same data";
        let fp1 = hmac_fingerprint(&key, b"label_a", data);
        let fp2 = hmac_fingerprint(&key, b"label_b", data);
        assert_ne!(fp1, fp2);
    }

    #[test]
    fn random_32_different_each_time() {
        let a = random_32();
        let b = random_32();
        assert_ne!(a, b);
    }

    #[test]
    fn api_key_material_format() {
        let pepper = random_32();
        let (key_id, verifier, credential) = generate_api_key_material(&pepper);

        // Key ID is 24 chars (base64url of 18 bytes)
        assert_eq!(key_id.len(), 24);

        // Verifier is 32 bytes
        assert_eq!(verifier.len(), 32);

        // Credential has prefix
        assert!(credential.starts_with("mem_sk_"));
    }
}
