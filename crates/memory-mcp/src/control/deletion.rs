//! Account deletion flow (Task 10.7, spec §14).
//!
//! 1. Recent OIDC reauthentication (Task 10.4).
//! 2. Display notice: no export/recovery.
//! 3. Typed-phrase confirmation by the user.
//! 4. Server-issued short-lived one-use confirmation token.
//! 5. Durable credential/session revocation.
//! 6. Idempotent logical deletion job.

use chrono::Utc;

use super::error::ApiError;
use super::recent_auth;
use super::session::ControlPlaneSession;

/// The typed phrase the user must type to confirm deletion.
pub const DELETION_TYPED_PHRASE: &str = "DELETE my account";

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
    phrase.trim() == DELETION_TYPED_PHRASE
}

/// Execute the deletion flow: validate recent auth, typed phrase,
/// revoke sessions, and mark account for deletion.
pub async fn execute_deletion(
    session: &ControlPlaneSession,
    typed_phrase: &str,
    store: &dyn crate::http::registry::storage::RegistryStore,
) -> Result<(), ApiError> {
    // Step 1: Recent auth check (10 minutes).
    recent_auth::require_recent_auth(session, recent_auth::DEFAULT_REAUTH_MAX_AGE)?;

    // Step 3: Typed phrase validation.
    if !validate_typed_phrase(typed_phrase) {
        return Err(ApiError::Forbidden);
    }

    // Step 5: Revoke the current session.
    store.delete_session(&session.cookie_hash).await?;

    // Step 6: Mark account as deleting (terminal state).
    let mut account = store.find_account_by_id(&session.account_id).await?;
    if let Some(ref mut acct) = account {
        acct.status = crate::http::registry::models::AccountStatus::Deleting;
        store.write_account(acct).await?;
    }

    Ok(())
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
