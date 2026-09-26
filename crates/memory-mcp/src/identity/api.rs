use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::MemoryError;

/// How recently a session must have authenticated before a
/// sensitive account action is allowed.
///
/// This is the single definition: `operations` reads it rather than
/// declaring its own copy, so the two cannot drift apart.
pub const RECENT_AUTH_MAX_AGE: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnlinkIdentityCommand {
    pub account_id: String,
    pub identity_id: String,
    pub actor: String,
    pub authenticated_at: DateTime<Utc>,
}

#[async_trait::async_trait]
pub trait IdentityLinkTransactions: Send + Sync {
    async fn unlink_external_identity(
        &self,
        account_id: &str,
        identity_id: &str,
        actor: &str,
        now: DateTime<Utc>,
    ) -> Result<(), MemoryError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LinkMode {
    Add,
    Replace,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedIdentityCommand {
    pub account_id: String,
    pub issuer: String,
    pub subject_verifier: [u8; 32],
    pub actor: String,
    pub mode: LinkMode,
}

#[async_trait::async_trait]
pub trait VerifiedIdentityLinkTransactions: Send + Sync {
    async fn find_identity_account(
        &self,
        issuer: &str,
        subject_verifier: &[u8; 32],
    ) -> Result<Option<String>, MemoryError>;

    async fn link_verified_identity(
        &self,
        account_id: &str,
        issuer: &str,
        subject_verifier: [u8; 32],
        actor: &str,
        mode: LinkMode,
        now: DateTime<Utc>,
    ) -> Result<(), MemoryError>;
}

pub async fn link_verified_identity(
    transactions: &(impl VerifiedIdentityLinkTransactions + ?Sized),
    command: &VerifiedIdentityCommand,
    now: DateTime<Utc>,
) -> Result<(), IdentityError> {
    if let Some(account_id) = transactions
        .find_identity_account(&command.issuer, &command.subject_verifier)
        .await
        .map_err(IdentityError::Persistence)?
    {
        if account_id == command.account_id {
            return Ok(());
        }
        return Err(IdentityError::IdentityHeldByAnotherAccount);
    }
    transactions
        .link_verified_identity(
            &command.account_id,
            &command.issuer,
            command.subject_verifier,
            &command.actor,
            command.mode,
            now,
        )
        .await
        .map_err(IdentityError::Persistence)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityInvitationCommand {
    pub account_id: String,
    pub invited_by: String,
    pub replace: bool,
}

#[async_trait::async_trait]
pub trait IdentityInvitationPort: Send + Sync {
    async fn account_exists(&self, account_id: &str) -> Result<bool, MemoryError>;
    async fn identity_count(&self, account_id: &str) -> Result<usize, MemoryError>;
}

pub async fn validate_identity_invitation(
    port: &(impl IdentityInvitationPort + ?Sized),
    command: &IdentityInvitationCommand,
) -> Result<(), IdentityError> {
    if !port
        .account_exists(&command.account_id)
        .await
        .map_err(IdentityError::Persistence)?
    {
        return Err(IdentityError::NotFound);
    }
    if command.replace
        && port
            .identity_count(&command.account_id)
            .await
            .map_err(IdentityError::Persistence)?
            != 1
    {
        return Err(IdentityError::LastIdentityOrConflict);
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvitationSession {
    pub cookie_value: String,
}

#[async_trait::async_trait]
pub trait InvitationSessionPort: Send + Sync {
    async fn has_valid_session(
        &self,
        account_id: &str,
        cookie: Option<&str>,
    ) -> Result<bool, MemoryError>;

    async fn issue_session(&self, account_id: &str) -> Result<InvitationSession, MemoryError>;
}

pub async fn ensure_invitation_session(
    port: &(impl InvitationSessionPort + ?Sized),
    account_id: &str,
    cookie: Option<&str>,
) -> Result<Option<InvitationSession>, MemoryError> {
    if port.has_valid_session(account_id, cookie).await? {
        return Ok(None);
    }
    port.issue_session(account_id).await.map(Some)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AuthMethod {
    Local,
    Oidc,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthMethodPolicy {
    pub methods: Vec<AuthMethod>,
    pub epoch: u64,
}

#[async_trait::async_trait]
pub trait AuthMethodPolicyPort: Send + Sync {
    async fn reconcile(&self, desired: &[AuthMethod]) -> Result<AuthMethodPolicy, MemoryError>;
    async fn remove(&self, method: AuthMethod) -> Result<AuthMethodPolicy, MemoryError>;
}

pub async fn reconcile_auth_methods(
    port: &(impl AuthMethodPolicyPort + ?Sized),
    desired: &[AuthMethod],
) -> Result<AuthMethodPolicy, IdentityError> {
    port.reconcile(desired)
        .await
        .map_err(IdentityError::Persistence)
}

pub async fn remove_auth_method(
    port: &(impl AuthMethodPolicyPort + ?Sized),
    method: AuthMethod,
) -> Result<AuthMethodPolicy, IdentityError> {
    port.remove(method).await.map_err(|error| match error {
        MemoryError::Conflict(message)
            if message.contains("is not enabled by the durable policy") =>
        {
            IdentityError::MethodNotEnabled(message)
        }
        other => IdentityError::Persistence(other),
    })
}

#[derive(Debug)]
pub enum IdentityError {
    ReauthenticationRequired,
    LastIdentityOrConflict,
    NotFound,
    IdentityHeldByAnotherAccount,
    MethodNotEnabled(String),
    Persistence(MemoryError),
}

pub async fn unlink_identity(
    transactions: &(impl IdentityLinkTransactions + ?Sized),
    command: &UnlinkIdentityCommand,
    now: DateTime<Utc>,
) -> Result<(), IdentityError> {
    if now - command.authenticated_at
        > chrono::Duration::from_std(RECENT_AUTH_MAX_AGE).unwrap_or_default()
    {
        return Err(IdentityError::ReauthenticationRequired);
    }
    transactions
        .unlink_external_identity(
            &command.account_id,
            &command.identity_id,
            &command.actor,
            now,
        )
        .await
        .map_err(|error| match error {
            MemoryError::Conflict(_) => IdentityError::LastIdentityOrConflict,
            MemoryError::NotFound(_) => IdentityError::NotFound,
            other => IdentityError::Persistence(other),
        })
}
