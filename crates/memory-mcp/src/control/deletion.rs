//! Account deletion flow.
//!
//! 1. Recent OIDC reauthentication.
//! 2. Display notice: no export/recovery.
//! 3. Typed-phrase confirmation by the user.
//! 4. Server-issued short-lived one-use confirmation token.
//! 5. Durable credential/session revocation.
//! 6. Idempotent logical deletion job.
//!
//! The deletion decision and recovery loop are owned by
//! `crate::operations::api`; this module keeps only the
//! control-plane protocol concerns (typed phrase and the durable
//! one-use token verifier).

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

/// The typed phrase the user must type to confirm deletion.
pub use crate::operations::api::DELETION_TYPED_PHRASE;

/// Validate that the typed phrase matches the expected deletion phrase.
pub fn validate_typed_phrase(phrase: &str) -> bool {
    crate::operations::api::DELETION_TYPED_PHRASE.eq(phrase.trim())
}

/// Derive the durable verifier for a one-use confirmation token. The raw token
/// is returned only to the browser in the start response and is never persisted.
pub fn token_verifier(key: &[u8; 32], token: &str) -> Result<String, crate::error::MemoryError> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key)
        .map_err(|_| crate::error::MemoryError::ConfigInvalid("deletion token key".into()))?;
    mac.update(token.as_bytes());
    Ok(hex::encode(mac.finalize().into_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_typed_phrase_exact() {
        assert!(validate_typed_phrase("DELETE my account"));
    }

    #[test]
    fn validate_typed_phrase_with_whitespace() {
        assert!(validate_typed_phrase("  DELETE my account  "));
    }

    #[test]
    fn validate_typed_phrase_wrong() {
        assert!(!validate_typed_phrase("delete my account"));
        assert!(!validate_typed_phrase("DELETE"));
        assert!(!validate_typed_phrase(""));
    }

    #[test]
    fn token_verifier_is_deterministic_and_key_dependent() {
        let key = [7u8; 32];
        let other_key = [9u8; 32];

        let verifier = token_verifier(&key, "token-a").expect("verifier");
        assert_eq!(
            verifier,
            token_verifier(&key, "token-a").expect("verifier"),
            "the same key and token must produce the same verifier"
        );
        assert_ne!(
            verifier,
            token_verifier(&key, "token-b").expect("verifier"),
            "a different token must produce a different verifier"
        );
        assert_ne!(
            verifier,
            token_verifier(&other_key, "token-a").expect("verifier"),
            "a different key must produce a different verifier"
        );
    }
}
