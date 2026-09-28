//! Control-plane atomic transaction ports.
//!
//! The control database is shared, but logical write ownership is not.
//! A handful of operations cross owners on purpose: an account, its
//! tenant and its first identity are created together; an identity link
//! and its audit row commit together; a deletion pass is begun and
//! finalized against tombstones. Those are the only cross-owner writes
//! the spec permits, and each is recorded with its commit boundary and
//! retry semantics in `docs/architecture/ddd-atomic-operations.md`.
//!
//! This module is the seam they go through. Three narrow ports rather
//! than the omnibus registry interface: a caller that links identities
//! should not be handed account creation, and a test that exercises
//! deletion should not implement forty methods to do it. ADR-0054
//! rejected a wider capability split and states the condition for
//! revisiting that.
//!
//! Implementations compose private SQL fragments under one database
//! transaction. No raw transaction or `query(sql)` escape hatch is
//! published. Nothing here decides policy — the owning contexts do —
//! which is why these are mechanics under `platform` rather than use
//! cases under a context.

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::error::MemoryError;
use crate::models::registry::{Account, ExternalIdentity, IdentityAudit, SubjectVerifier, Tenant};

/// Account, tenant and first identity creation as one transaction.
///
/// Either all three records exist afterwards or none do. There is no
/// sequential facade path that can leave half a client.
#[async_trait]
pub trait AccountBundleTx: Send + Sync {
    async fn create_account_bundle(
        &self,
        account: &Account,
        tenant: &Tenant,
        identity: Option<&ExternalIdentity>,
    ) -> Result<(), MemoryError>;
}

/// External-identity link, replacement and removal.
///
/// Every method takes the audit row rather than constructing one,
/// because the mutation and its audit must commit together (ADR-0057).
/// That is a property of the signature, not of the caller's care.
#[async_trait]
pub trait IdentityLinkTx: Send + Sync {
    /// Links `identity`, enforcing that the `(issuer, subject_verifier)`
    /// tuple is unique and the account exists.
    async fn link_external_identity(
        &self,
        identity: &ExternalIdentity,
        audit: &IdentityAudit,
    ) -> Result<(), MemoryError>;

    /// Replaces an existing link. Requires exactly one replacement
    /// target; anything else is a conflict rather than a partial write.
    async fn replace_external_identity(
        &self,
        identity: &ExternalIdentity,
        audit: &IdentityAudit,
    ) -> Result<(), MemoryError>;

    async fn unlink_external_identity(
        &self,
        account_id: &str,
        identity_id: &str,
        audit: &IdentityAudit,
    ) -> Result<(), MemoryError>;
}

/// Identity lookups, the read side of [`IdentityLinkTx`].
///
/// Split from the writes on purpose: an invitation acceptance reads an
/// account and then links, and should not be handed account creation or
/// deletion to do it.
#[async_trait]
pub trait IdentityLookup: Send + Sync {
    async fn find_account_by_id(&self, account_id: &str) -> Result<Option<Account>, MemoryError>;

    async fn find_account_by_identity(
        &self,
        issuer: &str,
        subject_verifier: &SubjectVerifier,
    ) -> Result<Option<Account>, MemoryError>;

    async fn find_external_identities(
        &self,
        account_id: &str,
    ) -> Result<Vec<ExternalIdentity>, MemoryError>;
}

/// The two ends of an account or operator deletion pass.
///
/// Not ordinary fact invalidation: tenant purge is an `operations`
/// concern with its own authorization and audit rules, and it does not
/// route through `knowledge`.
#[async_trait]
pub trait AccountDeletionTx: Send + Sync {
    async fn begin_account_deletion(
        &self,
        verifier: &str,
        account_id: &str,
        session_id: &str,
        now: DateTime<Utc>,
    ) -> Result<(), MemoryError>;

    /// Operator-initiated deletion without a user confirmation token.
    /// The same revocation and tombstone invariants apply.
    #[cfg(feature = "control-plane")]
    async fn begin_operator_deletion(
        &self,
        tenant_id: &str,
        actor: &str,
        now: DateTime<Utc>,
    ) -> Result<(), MemoryError>;

    /// Fenced, idempotent completion. Tombstones remain durable.
    #[cfg(feature = "control-plane")]
    async fn finalize_account_deletion(
        &self,
        tenant_id: &str,
        lease_owner_id: &str,
        lease_id: &str,
        fencing_generation: u64,
        completed_at: DateTime<Utc>,
    ) -> Result<(), MemoryError>;
}
