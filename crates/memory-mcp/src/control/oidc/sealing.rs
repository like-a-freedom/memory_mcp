//! Identity subject verifier (blind index) and OIDC state
//! AEAD sealing.
//!
//! Both functions are the cryptographic primitives that back the
//! control plane's identity-allowlist and request-state storage.
//! They live together because they are the only call sites for
//! the `hmac::Hmac<Sha256>` and `chacha20poly1305::ChaCha20Poly1305`
//! dependencies, and any audit of either primitive needs to read
//! the same code.

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

use crate::error::MemoryError;

use super::flow_material::{AuthError, OidcNonce, OidcState, PkceCode, StoredOidcRequest};

/// Compute a keyed HMAC of (issuer, subject) to create a blind
/// index for the OIDC identity. Raw OIDC subjects remain transient
/// only.
pub fn identity_subject_verifier(
    key: &[u8; 32],
    issuer: &str,
    subject: &str,
) -> Result<[u8; 32], MemoryError> {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(key)
        .map_err(|_| MemoryError::ConfigInvalid("identity index key".into()))?;
    mac.update(issuer.trim().as_bytes());
    mac.update(b":");
    mac.update(subject.as_bytes());
    Ok(mac.finalize().into_bytes().into())
}

/// Seal the OIDC flow material using AEAD encryption.
pub fn seal_oidc_payload(
    key: &[u8; 32],
    state: &OidcState,
    nonce: &OidcNonce,
    pkce: &PkceCode,
) -> Result<(Vec<u8>, [u8; 12]), MemoryError> {
    use chacha20poly1305::{ChaCha20Poly1305, KeyInit, Nonce, aead::Aead};

    let plaintext = serde_json::json!({
        "state": state.as_str(),
        "nonce": nonce.as_str(),
        "pkce_verifier": pkce.verifier,
    });
    let plaintext_bytes = serde_json::to_vec(&plaintext)
        .map_err(|_| MemoryError::ConfigInvalid("seal serialization".into()))?;

    let cipher = ChaCha20Poly1305::new(key.into());
    let mut nonce_bytes = [0u8; 12];
    rand::fill(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext_bytes.as_ref())
        .map_err(|_| AuthError::Sealing)
        .map_err(|e: AuthError| MemoryError::ConfigInvalid(e.to_string()))?;

    Ok((ciphertext, nonce_bytes))
}

/// Unseal OIDC flow material from the registry.
pub fn unseal_oidc_payload(
    key: &[u8; 32],
    ciphertext: &[u8],
    nonce_bytes: &[u8; 12],
) -> Result<StoredOidcRequest, MemoryError> {
    use chacha20poly1305::{ChaCha20Poly1305, KeyInit, Nonce, aead::Aead};

    let cipher = ChaCha20Poly1305::new(key.into());
    let nonce = Nonce::from_slice(nonce_bytes);

    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| AuthError::Sealing)
        .map_err(|e: AuthError| MemoryError::ConfigInvalid(e.to_string()))?;

    #[derive(serde::Deserialize)]
    struct SealedPayload {
        state: String,
        nonce: String,
        pkce_verifier: String,
    }

    let payload: SealedPayload = serde_json::from_slice(&plaintext)
        .map_err(|e| MemoryError::ConfigInvalid(format!("unseal parse: {e}")))?;

    Ok(StoredOidcRequest {
        state: OidcState(payload.state),
        nonce: OidcNonce(payload.nonce),
        pkce: PkceCode {
            verifier: payload.pkce_verifier,
            challenge: String::new(),
        },
        // The registry enforces the authoritative expiry at consume time;
        // this value is only the decrypted projection used by callers.
        expires_at: chrono::Utc::now() + chrono::Duration::minutes(10),
    })
}
