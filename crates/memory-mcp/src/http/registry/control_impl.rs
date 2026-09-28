//! Control-transaction implementations over the registry store.
//!
//! The three narrow ports in [`crate::platform::persistence::control`]
//! name cross-owner atomic operations. The registry store already has
//! the methods; this is where it is exposed as those ports, so a caller
//! that links identities is handed identity linking and nothing else.
//!
//! Implemented for `Arc<dyn RegistryStore>` rather than for a concrete
//! store, because composition holds the handle and several stores
//! satisfy the trait. Kept out of `platform` on purpose: the trait is
//! the seam, the registry is one implementation of it.

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::error::MemoryError;
use crate::http::registry::storage::RegistryStore;
use crate::models::registry::{Account, ExternalIdentity, IdentityAudit, SubjectVerifier, Tenant};
use crate::platform::persistence::control::{
    AccountBundleTx, AccountDeletionTx, IdentityLinkTx, IdentityLookup,
};
use std::sync::Arc;

#[async_trait]
impl AccountBundleTx for Arc<dyn RegistryStore> {
    async fn create_account_bundle(
        &self,
        account: &Account,
        tenant: &Tenant,
        identity: Option<&ExternalIdentity>,
    ) -> Result<(), MemoryError> {
        self.as_ref()
            .create_account_bundle(account, tenant, identity)
            .await
    }
}

#[async_trait]
impl IdentityLinkTx for Arc<dyn RegistryStore> {
    async fn link_external_identity(
        &self,
        identity: &ExternalIdentity,
        audit: &IdentityAudit,
    ) -> Result<(), MemoryError> {
        self.as_ref().link_external_identity(identity, audit).await
    }

    async fn replace_external_identity(
        &self,
        identity: &ExternalIdentity,
        audit: &IdentityAudit,
    ) -> Result<(), MemoryError> {
        self.as_ref()
            .replace_external_identity(identity, audit)
            .await
    }

    async fn unlink_external_identity(
        &self,
        account_id: &str,
        identity_id: &str,
        audit: &IdentityAudit,
    ) -> Result<(), MemoryError> {
        self.as_ref()
            .unlink_external_identity(account_id, identity_id, audit)
            .await
    }
}

#[async_trait]
impl IdentityLookup for Arc<dyn RegistryStore> {
    async fn find_account_by_id(&self, account_id: &str) -> Result<Option<Account>, MemoryError> {
        self.as_ref().find_account_by_id(account_id).await
    }

    async fn find_account_by_identity(
        &self,
        issuer: &str,
        subject_verifier: &SubjectVerifier,
    ) -> Result<Option<Account>, MemoryError> {
        self.as_ref()
            .find_account_by_identity(issuer, subject_verifier)
            .await
    }

    async fn find_external_identities(
        &self,
        account_id: &str,
    ) -> Result<Vec<ExternalIdentity>, MemoryError> {
        self.as_ref().find_external_identities(account_id).await
    }
}

#[async_trait]
impl AccountDeletionTx for Arc<dyn RegistryStore> {
    async fn begin_account_deletion(
        &self,
        verifier: &str,
        account_id: &str,
        session_id: &str,
        now: DateTime<Utc>,
    ) -> Result<(), MemoryError> {
        self.as_ref()
            .begin_account_deletion(verifier, account_id, session_id, now)
            .await
    }

    #[cfg(feature = "control-plane")]
    async fn begin_operator_deletion(
        &self,
        tenant_id: &str,
        actor: &str,
        now: DateTime<Utc>,
    ) -> Result<(), MemoryError> {
        self.as_ref()
            .begin_operator_deletion(tenant_id, actor, now)
            .await
    }

    #[cfg(feature = "control-plane")]
    async fn finalize_account_deletion(
        &self,
        tenant_id: &str,
        lease_owner_id: &str,
        lease_id: &str,
        fencing_generation: u64,
        completed_at: DateTime<Utc>,
    ) -> Result<(), MemoryError> {
        self.as_ref()
            .finalize_account_deletion(
                tenant_id,
                lease_owner_id,
                lease_id,
                fencing_generation,
                completed_at,
            )
            .await
    }
}
