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

use super::flow_material::{
    AuthError, OidcFlowIntent, OidcNonce, OidcState, PkceCode, StoredOidcRequest,
};

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

/// Seal the OIDC flow material, including what the flow is for, using AEAD
/// encryption.
pub fn seal_oidc_payload(
    key: &[u8; 32],
    state: &OidcState,
    nonce: &OidcNonce,
    pkce: &PkceCode,
    intent: &OidcFlowIntent,
) -> Result<(Vec<u8>, [u8; 12]), MemoryError> {
    use chacha20poly1305::{ChaCha20Poly1305, KeyInit, Nonce, aead::Aead};

    // The intent is inside the sealed plaintext, so a callback cannot be told
    // which flow it is completing by anything the browser controls.
    let intent = match intent {
        OidcFlowIntent::SignIn => serde_json::json!({ "kind": "sign_in" }),
        OidcFlowIntent::Link { account_id } => {
            serde_json::json!({ "kind": "link", "account_id": account_id })
        }
        OidcFlowIntent::Invite {
            account_id,
            invited_by,
            replace,
        } => serde_json::json!({
            "kind": "invite",
            "account_id": account_id,
            "invited_by": invited_by,
            "replace": replace,
        }),
    };
    let plaintext = serde_json::json!({
        "state": state.as_str(),
        "nonce": nonce.as_str(),
        "pkce_verifier": pkce.verifier,
        "intent": intent,
    });
    let plaintext_bytes = serde_json::to_vec(&plaintext)
        .map_err(|_| MemoryError::ConfigInvalid("seal serialization".into()))?;

    let cipher = ChaCha20Poly1305::new(key.into());
    let mut nonce_bytes = [0u8; 12];
    rand::fill(&mut nonce_bytes);
    // `Nonce::from_slice` is deprecated in `chacha20poly1305` 0.11. The length is
    // known here, so the array reference converts without a runtime check.
    let nonce: &Nonce = (&nonce_bytes).into();

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
    let nonce: &Nonce = nonce_bytes.into();

    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| AuthError::Sealing)
        .map_err(|e: AuthError| MemoryError::ConfigInvalid(e.to_string()))?;

    #[derive(serde::Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    enum SealedIntent {
        SignIn,
        Link {
            account_id: String,
        },
        Invite {
            account_id: String,
            invited_by: String,
            #[serde(default)]
            replace: bool,
        },
    }

    #[derive(serde::Deserialize)]
    struct SealedPayload {
        state: String,
        nonce: String,
        pkce_verifier: String,
        /// Absent in a payload sealed before the intent existed; those flows
        /// are logins, so that is what they decode as.
        #[serde(default)]
        intent: Option<SealedIntent>,
    }

    let payload: SealedPayload = serde_json::from_slice(&plaintext)
        .map_err(|e| MemoryError::ConfigInvalid(format!("unseal parse: {e}")))?;

    let intent = match payload.intent {
        None | Some(SealedIntent::SignIn) => OidcFlowIntent::SignIn,
        Some(SealedIntent::Link { account_id }) => OidcFlowIntent::Link { account_id },
        Some(SealedIntent::Invite {
            account_id,
            invited_by,
            replace,
        }) => OidcFlowIntent::Invite {
            account_id,
            invited_by,
            replace,
        },
    };

    Ok(StoredOidcRequest {
        state: OidcState(payload.state),
        nonce: OidcNonce(payload.nonce),
        pkce: PkceCode {
            verifier: payload.pkce_verifier,
            challenge: String::new(),
        },
        intent,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealing_round_trips_flow_material_and_the_invitation_intent() {
        let key = [0x5a; 32];
        let state = OidcState("state-token-fixed".into());
        let nonce = OidcNonce("nonce-token-fixed".into());
        let pkce = PkceCode {
            verifier: "fixed-verifier-from-the-caller".into(),
            challenge: "not-stored".into(),
        };
        let intent = OidcFlowIntent::Invite {
            account_id: "acct_invited".into(),
            invited_by: "admin_1".into(),
            replace: true,
        };

        let (ciphertext, nonce_bytes) =
            seal_oidc_payload(&key, &state, &nonce, &pkce, &intent).expect("seal flow");
        let stored = unseal_oidc_payload(&key, &ciphertext, &nonce_bytes).expect("unseal flow");

        assert_eq!(stored.state.as_str(), "state-token-fixed");
        assert_eq!(stored.nonce.as_str(), "nonce-token-fixed");
        assert_eq!(stored.pkce.verifier, "fixed-verifier-from-the-caller");
        assert_eq!(stored.intent, intent);
    }

    #[test]
    fn a_legacy_sealed_flow_without_an_intent_is_sign_in() {
        use chacha20poly1305::{ChaCha20Poly1305, KeyInit, Nonce, aead::Aead};

        let key = [0x31; 32];
        let nonce_bytes = [0x42; 12];
        let nonce: &Nonce = (&nonce_bytes).into();
        let plaintext = serde_json::to_vec(&serde_json::json!({
            "state": "legacy-state",
            "nonce": "legacy-nonce",
            "pkce_verifier": "legacy-verifier",
        }))
        .expect("serialize legacy flow");
        let ciphertext = ChaCha20Poly1305::new((&key).into())
            .encrypt(nonce, plaintext.as_ref())
            .expect("encrypt legacy flow");

        let stored =
            unseal_oidc_payload(&key, &ciphertext, &nonce_bytes).expect("unseal legacy flow");

        assert_eq!(stored.state.as_str(), "legacy-state");
        assert_eq!(stored.nonce.as_str(), "legacy-nonce");
        assert_eq!(stored.pkce.verifier, "legacy-verifier");
        assert_eq!(stored.intent, OidcFlowIntent::SignIn);
    }
}
