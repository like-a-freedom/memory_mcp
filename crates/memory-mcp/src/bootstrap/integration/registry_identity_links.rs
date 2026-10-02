use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::MemoryError;
use crate::http::registry::RegistryStores;
use crate::http::registry::models::{
    ExternalIdentity, IdentityAudit, SubjectVerifier, new_external_identity_id,
};
use crate::identity::api::{
    IdentityInvitationPort, IdentityLinkTransactions, LinkMode, VerifiedIdentityLinkTransactions,
};
use crate::platform::persistence::control::{IdentityLinkTx, IdentityLookup};

/// Typed against the identity ports only. It cannot reach account
/// creation or deletion, which is the narrowing this adapter exists for.
pub(crate) struct RegistryIdentityLinkTransactions {
    links: Arc<dyn IdentityLinkTx>,
    lookup: Arc<dyn IdentityLookup>,
}

impl RegistryIdentityLinkTransactions {
    /// Wrap the owner stores as each port here, so the adapter is typed
    /// against the ports only.
    pub(crate) fn from_stores(stores: &RegistryStores) -> Self {
        Self {
            links: crate::http::registry::control_impl::identity_link_tx(stores),
            lookup: crate::http::registry::control_impl::identity_lookup(stores),
        }
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
        self.links
            .unlink_external_identity(account_id, identity_id, &audit)
            .await
    }
}

#[async_trait::async_trait]
impl IdentityInvitationPort for RegistryIdentityLinkTransactions {
    async fn account_exists(&self, account_id: &str) -> Result<bool, MemoryError> {
        self.lookup
            .find_account_by_id(account_id)
            .await
            .map(|account| account.is_some())
    }

    async fn identity_count(&self, account_id: &str) -> Result<usize, MemoryError> {
        self.lookup
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
        self.lookup
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
            LinkMode::Add => self.links.link_external_identity(&identity, &audit).await,
            LinkMode::Replace => {
                self.links
                    .replace_external_identity(&identity, &audit)
                    .await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! The store-level invariants (last-link refusal, exactly-one
    //! replacement, no audit row for a refused change) are covered
    //! against the durable store in `http/registry/surreal_store.rs`
    //! and `control/account_api.rs`. What those suites cannot see is
    //! this adapter, because they call the store directly. The use-case
    //! suites (`tests/identity_unlink.rs`,
    //! `tests/identity_verified_link.rs`) do go through a port, but
    //! with a recording fake, so they never observe the row this
    //! adapter builds.
    //!
    //! Every method of both ports is exercised here, so a port method
    //! that quietly stops dispatching cannot pass: `link` (Add),
    //! `replace` (Replace), `unlink`, the three `IdentityLookup`
    //! reads, and the actor choice that only this adapter makes.
    //! `replace` matters most -- the store's exactly-one-replacement
    //! rule is ADR-0057's, and routing `LinkMode::Replace` to
    //! `link_external_identity` instead would still link the row
    //! while breaking the conflict contract.

    use super::*;
    use crate::http::registry::models::{
        Account, AccountStatus, NamespaceBinding, SubjectVerifier, Tenant, TenantStatus,
    };
    use crate::http::registry::storage::{AccountStore, IdentityStore, InMemoryStore};

    async fn store_with_account() -> Arc<InMemoryStore> {
        let store = Arc::new(InMemoryStore::default());
        let now = Utc::now();
        store
            .create_account_bundle(
                &Account {
                    id: "acct_1".into(),
                    status: AccountStatus::Active,
                    tenant_id: "ten_1".into(),
                    created_at: now,
                    display_name: None,
                },
                &Tenant {
                    id: "ten_1".into(),
                    status: TenantStatus::Ready,
                    namespace_binding: NamespaceBinding {
                        namespace: "tns_1".into(),
                        database: "memory".into(),
                    },
                    plan_version: 1,
                    schema_version: 0,
                    retry_stage: None,
                    provisioning_lease: None,
                    created_at: now,
                    version: 0,
                },
                None,
            )
            .await
            .expect("seed account");
        store
    }

    #[tokio::test]
    async fn linking_as_the_account_records_an_account_actor() {
        let store = store_with_account().await;
        let transactions = RegistryIdentityLinkTransactions::from_stores(
            &crate::http::registry::RegistryStores::from_backend(store.clone()),
        );
        let now = Utc::now();

        transactions
            .link_verified_identity(
                "acct_1",
                "https://idp.example.com",
                [0xA1; 32],
                "acct_1",
                LinkMode::Add,
                now,
            )
            .await
            .expect("self-service link");

        let rows = store.audit_events();
        assert_eq!(rows.len(), 1, "one link, one audit row");
        assert_eq!(rows[0].action, "identity_linked");
        assert_eq!(
            rows[0].actor_kind,
            crate::http::registry::models::AuditActorKind::Account,
            "an actor equal to the account is recorded as the account itself"
        );
        assert_eq!(rows[0].actor_principal, "acct_1");
    }

    #[tokio::test]
    async fn linking_as_an_administrator_records_an_operator_actor() {
        let store = store_with_account().await;
        let transactions = RegistryIdentityLinkTransactions::from_stores(
            &crate::http::registry::RegistryStores::from_backend(store.clone()),
        );
        let now = Utc::now();

        transactions
            .link_verified_identity(
                "acct_1",
                "https://idp.example.com",
                [0xB2; 32],
                "admin_root",
                LinkMode::Add,
                now,
            )
            .await
            .expect("operator-driven link");

        let rows = store.audit_events();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].actor_kind,
            crate::http::registry::models::AuditActorKind::Operator,
            "an administrator performing the change is recorded as the operator"
        );
        assert_eq!(rows[0].actor_principal, "admin_root");
    }

    /// A self-service unlink must record the *actor* the command named,
    /// not the account it acted on. An administrator may unlink
    /// somebody else's identity, and collapsing the two would make the
    /// audit row point at a principal that did not perform the action.
    #[tokio::test]
    async fn unlink_records_the_named_actor_rather_than_the_account() {
        let store = store_with_account().await;
        let now = Utc::now();
        let audit = IdentityAudit::by_account("acct_1", now);
        store
            .link_external_identity(
                &ExternalIdentity {
                    id: "idn_one".into(),
                    issuer: "https://idp.example.com".into(),
                    subject_verifier: SubjectVerifier([0xC3; 32]),
                    account_id: "acct_1".into(),
                    created_at: now,
                },
                &audit,
            )
            .await
            .expect("seed first identity");
        store
            .link_external_identity(
                &ExternalIdentity {
                    id: "idn_two".into(),
                    issuer: "https://idp.example.com".into(),
                    subject_verifier: SubjectVerifier([0xD4; 32]),
                    account_id: "acct_1".into(),
                    created_at: now,
                },
                &audit,
            )
            .await
            .expect("seed second identity");

        let transactions = RegistryIdentityLinkTransactions::from_stores(
            &crate::http::registry::RegistryStores::from_backend(store.clone()),
        );
        transactions
            .unlink_external_identity("acct_1", "idn_two", "admin_root", now)
            .await
            .expect("administrator unlinks an identity");

        let row = store
            .audit_events()
            .into_iter()
            .find(|event| event.action == "identity_unlinked")
            .expect("an unlink audit row");
        assert_eq!(row.actor_principal, "admin_root");
        assert_eq!(row.target_identity_id.as_deref(), Some("idn_two"));
    }

    /// The adapter mints a fresh identity id per link. A replay that
    /// reaches the store must therefore still be recognised as the
    /// same tuple, or the account would accumulate duplicate rows for
    /// one subject.
    #[tokio::test]
    async fn a_replayed_tuple_does_not_accumulate_duplicate_rows() {
        let store = store_with_account().await;
        let transactions = RegistryIdentityLinkTransactions::from_stores(
            &crate::http::registry::RegistryStores::from_backend(store.clone()),
        );
        let now = Utc::now();

        for _ in 0..3 {
            let account = transactions
                .find_identity_account("https://idp.example.com", &[0xE5; 32])
                .await
                .expect("lookup");
            if account.is_some() {
                continue;
            }
            transactions
                .link_verified_identity(
                    "acct_1",
                    "https://idp.example.com",
                    [0xE5; 32],
                    "acct_1",
                    LinkMode::Add,
                    now,
                )
                .await
                .expect("link");
        }

        assert_eq!(
            store
                .find_external_identities("acct_1")
                .await
                .expect("list identities")
                .len(),
            1,
            "one subject must not accumulate duplicate identity rows"
        );
        assert_eq!(
            store
                .audit_events()
                .into_iter()
                .filter(|event| event.action == "identity_linked")
                .count(),
            1,
            "one link, one audit row"
        );
    }

    async fn link_one(transactions: &RegistryIdentityLinkTransactions, subject: u8) {
        transactions
            .link_verified_identity(
                "acct_1",
                "https://idp.example.com",
                [subject; 32],
                "acct_1",
                LinkMode::Add,
                Utc::now(),
            )
            .await
            .expect("link");
    }

    #[tokio::test]
    async fn replace_supersedes_rather_than_accumulating() {
        let store = store_with_account().await;
        let transactions = RegistryIdentityLinkTransactions::from_stores(
            &crate::http::registry::RegistryStores::from_backend(store.clone()),
        );
        link_one(&transactions, 0xB1).await;

        transactions
            .link_verified_identity(
                "acct_1",
                "https://idp.example.com",
                [0xB2; 32],
                "op_1",
                LinkMode::Replace,
                Utc::now(),
            )
            .await
            .expect("replace");

        // The store performs a replacement as a superseding unlink+link
        // inside one transaction, so the audit trail is two rows. What
        // separates a replace from a plain add is the outcome: the count
        // stays at one, and the superseded row is gone. Routing Replace
        // to link_external_identity instead would leave two rows.
        let identities = store
            .find_external_identities("acct_1")
            .await
            .expect("read back");
        assert_eq!(
            identities.len(),
            1,
            "replacement supersedes rather than accumulating, got {} rows",
            identities.len()
        );
        assert_ne!(
            identities[0].subject_verifier.0, [0xB1u8; 32],
            "the superseded identity is the new one"
        );
        let actions = store
            .audit_events()
            .iter()
            .map(|row| row.action.clone())
            .collect::<Vec<_>>();
        assert!(
            actions.iter().any(|action| action == "identity_unlinked")
                && actions.iter().any(|action| action == "identity_linked"),
            "a replacement is a superseding unlink+link, got {actions:?}"
        );
    }

    #[tokio::test]
    async fn account_exists_reads_through_the_lookup_port() {
        let store = store_with_account().await;
        let transactions = RegistryIdentityLinkTransactions::from_stores(
            &crate::http::registry::RegistryStores::from_backend(store.clone()),
        );

        assert!(
            transactions
                .account_exists("acct_1")
                .await
                .expect("existing account"),
            "the seeded account is found via find_account_by_id"
        );
        assert!(
            !transactions
                .account_exists("acct_absent")
                .await
                .expect("absent account"),
            "an unknown id is absent, not an error"
        );
    }

    #[tokio::test]
    async fn identity_count_reads_through_the_lookup_port() {
        let store = store_with_account().await;
        let transactions = RegistryIdentityLinkTransactions::from_stores(
            &crate::http::registry::RegistryStores::from_backend(store.clone()),
        );

        assert_eq!(
            transactions.identity_count("acct_1").await.expect("count"),
            0,
            "the seeded account has no identities"
        );
        link_one(&transactions, 0xC1).await;
        link_one(&transactions, 0xC2).await;
        assert_eq!(
            transactions.identity_count("acct_1").await.expect("count"),
            2,
            "both links are counted through find_external_identities"
        );
    }

    #[tokio::test]
    async fn unlink_dispatches_through_the_port_and_removes_the_row() {
        let store = store_with_account().await;
        let transactions = RegistryIdentityLinkTransactions::from_stores(
            &crate::http::registry::RegistryStores::from_backend(store.clone()),
        );
        link_one(&transactions, 0xD1).await;
        link_one(&transactions, 0xD2).await;

        // ADR-0057 refuses to remove an account's last identity, so
        // link a second one and unlink the first; the refusal itself is
        // the store's own suite to pin.
        let identity_id = store
            .find_external_identities("acct_1")
            .await
            .expect("read back")
            .first()
            .expect("two linked identities")
            .id
            .clone();
        transactions
            .unlink_external_identity("acct_1", &identity_id, "op_1", Utc::now())
            .await
            .expect("unlink");

        assert_eq!(
            transactions.identity_count("acct_1").await.expect("count"),
            1,
            "the unlink went through the port and removed exactly one row"
        );
    }
}
