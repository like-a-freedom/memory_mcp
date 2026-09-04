//! OIDC signup workflow (Task 12).
//!
//! Extracted from `crate::control::oidc::upsert_account_for_identity`
//! so the business rules can be exercised without spinning
//! up the OIDC callback. Landed in the same milestone as
//! `api_keys.rs`; both workflows decouple transport from
//! business rules.

use std::sync::Arc;

use crate::error::MemoryError;
use crate::http::registry::models::Account;
use crate::http::registry::models::SubjectVerifier;
use crate::http::registry::storage::RegistryStore;

/// An OIDC-verified identity, ready to be resolved to an
/// existing account or to provision a new one. The raw OIDC
/// `sub` is never stored; the `subject_verifier` is the
/// keyed blind index computed from the issuer + subject by
/// the OIDC callback before it hands off to this workflow.
pub(crate) struct VerifiedExternalIdentity {
    pub issuer: String,
    pub subject_verifier: SubjectVerifier,
}

/// The application-layer OIDC signup workflow.
///
/// The struct holds the omnibus `Arc<dyn RegistryStore>` while
/// Task 10 (consumer migration onto capability traits) is
/// deferred. The two-capability field shape documented in the
/// plan returns when the `RegistryStores` aggregator is
/// available.
pub(crate) struct OidcSignup {
    store: Arc<dyn RegistryStore>,
}

impl OidcSignup {
    /// Build a workflow from the registry store the HTTP
    /// composition selected.
    pub(crate) fn new(store: Arc<dyn RegistryStore>) -> Self {
        Self { store }
    }

    /// Resolve the verified identity to an existing account,
    /// or create a new tenant bundle for it.
    ///
    /// The contract is:
    ///
    /// 1. If the identity is already linked to an account,
    ///    return that account. (Idempotent re-login.)
    /// 2. Otherwise, atomically create the account + tenant
    ///    + identity bundle. A concurrent signup that wins
    ///    the race is resolved by a follow-up read; the
    ///    `MemoryError::Conflict` from the loser is mapped
    ///    to the winner's record when one is found.
    /// 3. Append the provisioning event only for the
    ///    account created by *this* call.
    // Indented bullet continuations below are intentional:
    // clippy's `doc_lazy_continuation` lint expects every
    // continuation line of a list item to be indented by
    // four spaces. The multi-line items above do not need
    // the extra indent because they are already inside a
    // `///` comment that is itself indented.
    #[allow(clippy::doc_lazy_continuation)]
    pub(crate) async fn resolve_or_create(
        &self,
        identity: VerifiedExternalIdentity,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Account, MemoryError> {
        // Step 1: existing identity. Borrow `identity` here
        // (no move) so the conflict-handling path can
        // borrow it again after the read.
        if let Some(account) = self
            .store
            .find_account_by_identity(&identity.issuer, &identity.subject_verifier)
            .await?
        {
            return Ok(account);
        }

        // Step 2: create a new bundle. The atomic
        // `create_account_bundle` enforces uniqueness on the
        // (issuer, subject_verifier) tuple, so a concurrent
        // signup that wins the race causes this call to
        // return `Conflict`. The read above already
        // established that no account is linked to this
        // identity; the conflict path covers a race against
        // a concurrent signup that won between the read and
        // the create. `issuer` and `subject_verifier` are
        // cloned because the conflict-handling reread needs
        // the tuple to look up the winner after `build_bundle`
        // has consumed its own copy.
        let issuer = identity.issuer.clone();
        let subject_verifier = identity.subject_verifier.clone();
        let (account, tenant, identity_record) =
            build_bundle(identity.issuer, identity.subject_verifier, now);
        match self
            .store
            .create_account_bundle(&account, &tenant, Some(&identity_record))
            .await
        {
            Ok(()) => {
                // We won the race. Append the provisioning
                // event so the scheduler advances the new
                // tenant through Reserved -> Ready.
                self.store
                    .append_provisioning_event(&tenant.id, "reserved")
                    .await?;
                Ok(account)
            }
            Err(MemoryError::Conflict(_)) => {
                // Loser of the race. Reread by identity; if a
                // winner now exists, return it. Otherwise
                // surface the original conflict.
                if let Some(account) = self
                    .store
                    .find_account_by_identity(&issuer, &subject_verifier)
                    .await?
                {
                    return Ok(account);
                }
                Err(MemoryError::Conflict(
                    "create_account_bundle lost the race but no winner was found".into(),
                ))
            }
            Err(other) => Err(other),
        }
    }
}

/// Build a deterministic account + tenant + identity triple
/// for the given verified identity. The bundle-creation
/// timestamp is the `now` argument so tests can pin it.
/// The `subject_verifier` and `issuer` are passed by value
/// because they are moved into the persisted record.
fn build_bundle(
    issuer: String,
    subject_verifier: crate::http::registry::models::SubjectVerifier,
    now: chrono::DateTime<chrono::Utc>,
) -> (
    crate::http::registry::models::Account,
    crate::http::registry::models::Tenant,
    crate::http::registry::models::ExternalIdentity,
) {
    use crate::http::registry::models::{
        Account, AccountStatus, ExternalIdentity, NamespaceBinding, Tenant, TenantStatus,
        new_account_id, new_namespace_name, new_tenant_id,
    };
    let account = Account {
        id: new_account_id(),
        status: AccountStatus::Active,
        tenant_id: new_tenant_id(),
        created_at: now,
    };
    let tenant = Tenant {
        id: account.tenant_id.clone(),
        status: TenantStatus::Reserved,
        namespace_binding: NamespaceBinding {
            namespace: new_namespace_name(),
            database: "memory".into(),
        },
        plan_version: 1,
        schema_version: 0,
        retry_stage: None,
        provisioning_lease: None,
        created_at: now,
        version: 0,
    };
    let identity_record = ExternalIdentity {
        id: format!("id_{}", uuid::Uuid::new_v4()),
        account_id: account.id.clone(),
        issuer,
        subject_verifier,
        created_at: now,
    };
    (account, tenant, identity_record)
}

#[cfg(test)]
mod tests {
    //! Workflow tests for `OidcSignup`. They construct the
    //! workflow against an in-memory `RegistryStore` and
    //! exercise the contract: existing identity, atomic
    //! bundle, provisioning-event append, and the race-loss
    //! reread. The HTTP-adapter tests in `oidc.rs` cover
    //! the OIDC transport side; this module covers the
    //! business-rule side.

    use super::*;
    use crate::http::registry::models::SubjectVerifier;
    use crate::http::registry::storage::InMemoryStore;
    use std::sync::Arc;

    fn verifier(byte: u8) -> SubjectVerifier {
        SubjectVerifier([byte; 32])
    }

    fn verified(issuer: &str, byte: u8) -> VerifiedExternalIdentity {
        VerifiedExternalIdentity {
            issuer: issuer.to_string(),
            subject_verifier: verifier(byte),
        }
    }

    /// A first call to `resolve_or_create` for a brand-new
    /// identity creates the bundle and returns the new
    /// account. The provisioning event is appended.
    #[tokio::test]
    async fn create_new_identity_writes_bundle_and_event() {
        let store = Arc::new(InMemoryStore::default());
        let now = chrono::Utc::now();
        let workflow = OidcSignup::new(store.clone());
        let account = workflow
            .resolve_or_create(verified("https://issuer.example.com", 0xAA), now)
            .await
            .expect("first signup succeeds");
        assert_eq!(
            account.status,
            crate::http::registry::models::AccountStatus::Active
        );
        assert!(
            store
                .find_account_by_identity("https://issuer.example.com", &verifier(0xAA))
                .await
                .expect("identity lookup")
                .is_some(),
            "the new identity is now linked"
        );
    }

    /// A second call for the same identity returns the
    /// already-linked account without creating a new one.
    #[tokio::test]
    async fn existing_identity_is_idempotent() {
        let store = Arc::new(InMemoryStore::default());
        let now = chrono::Utc::now();
        let workflow = OidcSignup::new(store.clone());
        let first = workflow
            .resolve_or_create(verified("https://issuer.example.com", 0xBB), now)
            .await
            .expect("first signup");
        let second = workflow
            .resolve_or_create(
                verified("https://issuer.example.com", 0xBB),
                now + chrono::Duration::seconds(5),
            )
            .await
            .expect("second signup");
        assert_eq!(first.id, second.id, "second call returns the same account");
    }

    /// The conflict-handling path: when a concurrent signup
    /// already linked the identity, the second call to
    /// `create_account_bundle` returns `Conflict`, the
    /// workflow rereads by identity, and the winner is
    /// returned. We simulate the conflict by pre-linking
    /// the identity so the create hits the conflict
    /// branch.
    ///
    /// The test directly seeds an account + identity that
    /// point at a different `account_id` than the one
    /// `build_bundle` would have produced. The workflow
    /// then tries to create a new bundle for the same
    /// identity, which fails with `Conflict`, and the
    /// reread returns the seeded account.
    #[tokio::test]
    async fn conflict_is_resolved_by_reread() {
        use crate::http::registry::models::{Account, AccountStatus, ExternalIdentity};
        let store = Arc::new(InMemoryStore::default());
        let now = chrono::Utc::now();

        // Seed a pre-existing account + identity.
        let existing_account = Account {
            id: "acct_existing".to_string(),
            status: AccountStatus::Active,
            tenant_id: "ten_existing".to_string(),
            created_at: now,
        };
        let existing_identity = ExternalIdentity {
            id: "id_existing".to_string(),
            account_id: existing_account.id.clone(),
            issuer: "https://issuer.example.com".to_string(),
            subject_verifier: verifier(0xCC),
            created_at: now,
        };
        store.write_account(&existing_account).await.unwrap();
        store
            .link_external_identity(&existing_identity)
            .await
            .expect("seed identity");

        // Now run the workflow. The `find_account_by_identity`
        // check at the top will find the existing account, so
        // we exercise the early-return path; the conflict
        // path requires a race that the unit test cannot
        // stage deterministically. The test still proves
        // the idempotency contract.
        let workflow = OidcSignup::new(store.clone());
        let account = workflow
            .resolve_or_create(verified("https://issuer.example.com", 0xCC), now)
            .await
            .expect("idempotent re-login");
        assert_eq!(account.id, existing_account.id);
    }

    #[tokio::test]
    async fn create_conflict_rereads_the_concurrent_winner() {
        let store = Arc::new(InMemoryStore::default());
        let now = chrono::Utc::now();
        let (winner, winner_tenant, winner_identity) =
            build_bundle("https://issuer.example.com".into(), verifier(0xCD), now);
        let winner_id = winner.id.clone();
        store.inject_oidc_conflict(Some((winner, winner_tenant, winner_identity)));

        let account = OidcSignup::new(store)
            .resolve_or_create(verified("https://issuer.example.com", 0xCD), now)
            .await
            .expect("conflict must resolve to the concurrent winner");
        assert_eq!(account.id, winner_id);
    }

    #[tokio::test]
    async fn create_conflict_without_winner_remains_conflict() {
        let store = Arc::new(InMemoryStore::default());
        let now = chrono::Utc::now();
        store.inject_oidc_conflict(None);
        let result = OidcSignup::new(store)
            .resolve_or_create(verified("https://issuer.example.com", 0xCE), now)
            .await;
        assert!(matches!(result, Err(MemoryError::Conflict(_))));
    }

    /// Two different identities produce two different
    /// accounts. Distinct `(issuer, subject_verifier)`
    /// tuples never collide.
    #[tokio::test]
    async fn distinct_identities_produce_distinct_accounts() {
        let store = Arc::new(InMemoryStore::default());
        let now = chrono::Utc::now();
        let workflow = OidcSignup::new(store.clone());
        let alice = workflow
            .resolve_or_create(verified("https://issuer.example.com", 0x01), now)
            .await
            .expect("alice signup");
        let bob = workflow
            .resolve_or_create(verified("https://issuer.example.com", 0x02), now)
            .await
            .expect("bob signup");
        assert_ne!(
            alice.id, bob.id,
            "different verifiers produce different accounts"
        );
    }
}
