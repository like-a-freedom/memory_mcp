use chrono::{DateTime, Utc};

use crate::MemoryError;

pub const DELETION_TYPED_PHRASE: &str = "DELETE my account";

/// Identity owns the recent-authentication window, so this context
/// reads that policy rather than defining a second copy of it. A
/// security constant with two owners is two values waiting to drift.
pub use crate::identity::api::RECENT_AUTH_MAX_AGE;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BeginAccountDeletionCommand {
    pub account_id: String,
    pub session_id: String,
    pub authenticated_at: DateTime<Utc>,
    pub typed_phrase: String,
    pub challenge_verifier: String,
}

#[async_trait::async_trait]
pub trait AccountDeletionPort: Send + Sync {
    async fn begin_account_deletion(
        &self,
        verifier: &str,
        account_id: &str,
        session_id: &str,
        now: DateTime<Utc>,
    ) -> Result<(), MemoryError>;
}

#[derive(Debug)]
pub enum OperationsError {
    ReauthenticationRequired,
    ConfirmationPhraseRejected,
    Persistence(MemoryError),
}

impl std::fmt::Display for OperationsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ReauthenticationRequired => f.write_str("recent authentication required"),
            Self::ConfirmationPhraseRejected => {
                f.write_str("deletion confirmation phrase rejected")
            }
            Self::Persistence(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for OperationsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Persistence(error) => Some(error),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryOutcome {
    Finalized,
    Purged,
}

impl RecoveryOutcome {
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Purged)
    }
}

#[async_trait::async_trait]
pub trait DeletionRecoveryPort: Send + Sync {
    async fn list_deleting_tenants(
        &self,
        limit: usize,
        now: DateTime<Utc>,
    ) -> Result<Vec<String>, MemoryError>;

    async fn recover_tenant(
        &self,
        tenant_id: &str,
        now: DateTime<Utc>,
    ) -> Result<RecoveryOutcome, MemoryError>;
}

pub async fn run_deletion_recovery(
    port: &(impl DeletionRecoveryPort + ?Sized),
    now: DateTime<Utc>,
) -> Result<(), MemoryError> {
    let tenants = port.list_deleting_tenants(64, now).await?;
    let mut first_error = None;
    for tenant_id in tenants {
        if let Err(error) = port.recover_tenant(&tenant_id, now).await
            && first_error.is_none()
        {
            first_error = Some(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

pub async fn begin_account_deletion(
    port: &(impl AccountDeletionPort + ?Sized),
    command: &BeginAccountDeletionCommand,
    now: DateTime<Utc>,
) -> Result<(), OperationsError> {
    if now - command.authenticated_at
        > chrono::Duration::from_std(RECENT_AUTH_MAX_AGE).unwrap_or_default()
    {
        return Err(OperationsError::ReauthenticationRequired);
    }
    if command.typed_phrase.trim() != DELETION_TYPED_PHRASE {
        return Err(OperationsError::ConfirmationPhraseRejected);
    }
    port.begin_account_deletion(
        &command.challenge_verifier,
        &command.account_id,
        &command.session_id,
        now,
    )
    .await
    .map_err(OperationsError::Persistence)
}
