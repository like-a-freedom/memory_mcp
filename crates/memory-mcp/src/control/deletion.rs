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

use chrono::Utc;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

/// The typed phrase the user must type to confirm deletion.
pub use crate::operations::api::DELETION_TYPED_PHRASE;
pub use crate::operations::api::begin_account_deletion;

/// Short-lived confirmation token for the deletion flow.
#[derive(Debug, Clone)]
pub struct DeletionConfirmationToken {
    pub account_id: String,
    pub session_id: String,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub used: bool,
}

impl DeletionConfirmationToken {
    pub fn new(account_id: &str, session_id: &str) -> Self {
        Self {
            account_id: account_id.to_string(),
            session_id: session_id.to_string(),
            expires_at: Utc::now() + chrono::Duration::minutes(5),
            used: false,
        }
    }

    pub fn is_valid(&self) -> bool {
        !self.used && self.expires_at > Utc::now()
    }
}

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
    fn deletion_token_is_valid_initially() {
        let token = DeletionConfirmationToken::new("acc1", "sess1");
        assert!(token.is_valid());
    }

    #[test]
    fn deletion_token_expires() {
        let mut token = DeletionConfirmationToken::new("acc1", "sess1");
        token.expires_at = Utc::now() - chrono::Duration::minutes(1);
        assert!(!token.is_valid());
    }

    #[test]
    fn deletion_token_one_use() {
        let mut token = DeletionConfirmationToken::new("acc1", "sess1");
        assert!(token.is_valid());
        token.used = true;
        assert!(!token.is_valid());
    }
}
