//! Control-transaction implementations over the registry stores.
//!
//! The three narrow ports in [`crate::platform::persistence::control`]
//! name cross-owner atomic operations. The registry stores already have
//! the methods; this is where they are composed as those ports, so a caller
//! that links identities is handed identity linking and nothing else.
//!
//! Implemented for [`RegistryStores`] rather than for a concrete store,
//! because composition holds the owner traits and several stores satisfy
//! them. Kept out of `platform` on purpose: the trait is the seam, the
//! registry is one implementation of it.

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::error::MemoryError;
use crate::http::registry::RegistryStores;
use crate::models::registry::{Account, ExternalIdentity, IdentityAudit, SubjectVerifier, Tenant};
use crate::platform::persistence::control::{
    AccountBundleTx, AccountDeletionTx, IdentityLinkTx, IdentityLookup,
};
use std::sync::Arc;

/// The stores a cross-owner atomic operation composes.
///
/// A cross-owner port is satisfied by one object that holds the owner traits
/// it needs, not by a caller holding several. Keeping the pairing here means
/// each port's implementation names exactly the owners it crosses, which is
/// the audit the omnibus interface removed: a reader can see that account
/// deletion spans `account` and `tenant` and nothing more.
#[derive(Clone)]
struct AccountAndTenant {
    accounts: Arc<dyn super::storage::AccountStore>,
    identities: Arc<dyn super::storage::IdentityStore>,
    tenants: Arc<dyn super::storage::TenantStore>,
}

impl AccountAndTenant {
    fn from_stores(stores: &RegistryStores) -> Self {
        Self {
            accounts: Arc::clone(&stores.accounts),
            identities: Arc::clone(&stores.identities),
            tenants: Arc::clone(&stores.tenants),
        }
    }
}

/// Wrap the owner stores as the atomic account-bundle port.
pub fn account_bundle_tx(stores: &RegistryStores) -> Arc<dyn AccountBundleTx> {
    Arc::new(AccountAndTenant::from_stores(stores))
}

/// Wrap the owner stores as the identity link/lookup ports.
pub fn identity_link_tx(stores: &RegistryStores) -> Arc<dyn IdentityLinkTx> {
    Arc::new(AccountAndTenant::from_stores(stores))
}

pub fn identity_lookup(stores: &RegistryStores) -> Arc<dyn IdentityLookup> {
    Arc::new(AccountAndTenant::from_stores(stores))
}

/// Wrap the owner stores as the atomic account-deletion port.
pub fn account_deletion_tx(stores: &RegistryStores) -> Arc<dyn AccountDeletionTx> {
    Arc::new(AccountAndTenant::from_stores(stores))
}

#[async_trait]
impl AccountBundleTx for AccountAndTenant {
    async fn create_account_bundle(
        &self,
        account: &Account,
        tenant: &Tenant,
        identity: Option<&ExternalIdentity>,
    ) -> Result<(), MemoryError> {
        self.accounts
            .create_account_bundle(account, tenant, identity)
            .await
    }
}

#[async_trait]
impl IdentityLinkTx for AccountAndTenant {
    async fn link_external_identity(
        &self,
        identity: &ExternalIdentity,
        audit: &IdentityAudit,
    ) -> Result<(), MemoryError> {
        self.identities
            .link_external_identity(identity, audit)
            .await
    }

    async fn replace_external_identity(
        &self,
        identity: &ExternalIdentity,
        audit: &IdentityAudit,
    ) -> Result<(), MemoryError> {
        self.identities
            .replace_external_identity(identity, audit)
            .await
    }

    async fn unlink_external_identity(
        &self,
        account_id: &str,
        identity_id: &str,
        audit: &IdentityAudit,
    ) -> Result<(), MemoryError> {
        self.identities
            .unlink_external_identity(account_id, identity_id, audit)
            .await
    }
}

#[async_trait]
impl IdentityLookup for AccountAndTenant {
    async fn find_account_by_id(&self, account_id: &str) -> Result<Option<Account>, MemoryError> {
        self.accounts.find_account_by_id(account_id).await
    }

    async fn find_account_by_identity(
        &self,
        issuer: &str,
        subject_verifier: &SubjectVerifier,
    ) -> Result<Option<Account>, MemoryError> {
        self.accounts
            .find_account_by_identity(issuer, subject_verifier)
            .await
    }

    async fn find_external_identities(
        &self,
        account_id: &str,
    ) -> Result<Vec<ExternalIdentity>, MemoryError> {
        self.identities.find_external_identities(account_id).await
    }
}

#[async_trait]
impl AccountDeletionTx for AccountAndTenant {
    async fn begin_account_deletion(
        &self,
        verifier: &str,
        account_id: &str,
        session_id: &str,
        now: DateTime<Utc>,
    ) -> Result<(), MemoryError> {
        self.accounts
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
        self.tenants
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
        self.tenants
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
