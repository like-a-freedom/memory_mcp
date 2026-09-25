use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::MemoryError;
use crate::http::registry::models::{
    ExternalIdentity, IdentityAudit, SubjectVerifier, new_external_identity_id,
};
use crate::http::registry::storage::RegistryStore;
use crate::identity::api::{
    IdentityInvitationPort, IdentityLinkTransactions, LinkMode, VerifiedIdentityLinkTransactions,
};

pub(crate) struct RegistryIdentityLinkTransactions {
    store: Arc<dyn RegistryStore>,
}

impl RegistryIdentityLinkTransactions {
    pub(crate) fn new(store: Arc<dyn RegistryStore>) -> Self {
        Self { store }
    }
}

#[async_trait::async_trait]
impl IdentityLinkTransactions for RegistryIdentityLinkTransactions {
    async fn unlink_external_identity(
        &self,
        account_id: &str,
        identity_id: &str,
        actor: &str,
        now: DateTime<Utc>,
    ) -> Result<(), MemoryError> {
        let mut audit = IdentityAudit::by_account(account_id, now);
        audit.actor_principal = actor.to_string();
        self.store
            .unlink_external_identity(account_id, identity_id, &audit)
            .await
    }
}

#[async_trait::async_trait]
impl IdentityInvitationPort for RegistryIdentityLinkTransactions {
    async fn account_exists(&self, account_id: &str) -> Result<bool, MemoryError> {
        self.store
            .find_account_by_id(account_id)
            .await
            .map(|account| account.is_some())
    }

    async fn identity_count(&self, account_id: &str) -> Result<usize, MemoryError> {
        self.store
            .find_external_identities(account_id)
            .await
            .map(|identities| identities.len())
    }
}

#[async_trait::async_trait]
impl VerifiedIdentityLinkTransactions for RegistryIdentityLinkTransactions {
    async fn find_identity_account(
        &self,
        issuer: &str,
        subject_verifier: &[u8; 32],
    ) -> Result<Option<String>, MemoryError> {
        self.store
            .find_account_by_identity(issuer, &SubjectVerifier(*subject_verifier))
            .await
            .map(|identity| identity.map(|identity| identity.id))
    }

    async fn link_verified_identity(
        &self,
        account_id: &str,
        issuer: &str,
        subject_verifier: [u8; 32],
        actor: &str,
        mode: LinkMode,
        now: DateTime<Utc>,
    ) -> Result<(), MemoryError> {
        let identity = ExternalIdentity {
            id: new_external_identity_id(),
            issuer: issuer.to_owned(),
            subject_verifier: SubjectVerifier(subject_verifier),
            account_id: account_id.to_owned(),
            created_at: now,
        };
        let audit = if actor == account_id {
            IdentityAudit::by_account(account_id, now)
        } else {
            IdentityAudit::by_operator(actor, now)
        };
        match mode {
            LinkMode::Add => self.store.link_external_identity(&identity, &audit).await,
            LinkMode::Replace => {
                self.store
                    .replace_external_identity(&identity, &audit)
                    .await
            }
        }
    }
}
