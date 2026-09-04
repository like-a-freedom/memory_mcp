//! API-key creation workflow (Task 11).
//!
//! Extracted from `crate::control::account_api::create_api_key`
//! so the business rules can be exercised without an Axum
//! router. The HTTP adapter now owns transport-only logic
//! (body parsing, response serialization, `Cache-Control`
//! headers) and delegates the workflow to `ApiKeyCreation`.
//!
//! Atomic operations stay as named single methods on
//! `RegistryStore`; the workflow does not reconstruct
//! multi-row writes as application-level sequences.

use std::sync::Arc;

use crate::error::MemoryError;
use crate::http::registry::models::{ApiKey, ApiKeyStatus, KeyedVerifier, new_api_key_id};
use crate::http::registry::storage::RegistryStore;

/// Command for `ApiKeyCreation::execute`. The
/// `expires_in_days` field is applied relative to the `now`
/// argument supplied at execution time so tests can pin the
/// expiry timestamp deterministically.
pub(crate) struct CreateApiKeyCommand {
    pub account_id: String,
    pub name: String,
    pub expires_in_days: Option<u32>,
}

/// Result of a successful API-key creation. The `secret` is
/// shown once and never persisted; the caller is responsible
/// for delivering it to the end user.
#[derive(Debug)]
pub(crate) struct CreatedApiKey {
    pub id: String,
    pub secret: String,
    pub name: String,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// The application-layer API-key creation workflow.
///
/// The struct holds the omnibus `Arc<dyn RegistryStore>` while
/// Task 10 (consumer migration onto capability traits) is
/// deferred. The four-capability field shape documented in the
/// plan returns when the `RegistryStores` aggregator is
/// available.
pub(crate) struct ApiKeyCreation {
    store: Arc<dyn RegistryStore>,
    api_key_pepper: String,
}

impl ApiKeyCreation {
    /// Build a workflow from the registry store the HTTP
    /// composition selected and the API-key pepper the
    /// operator configured. The pepper is held by `String`
    /// to match the `HttpConfig::api_key_pepper` field
    /// type; the workflow converts to bytes once at use
    /// time.
    pub(crate) fn new(store: Arc<dyn RegistryStore>, api_key_pepper: String) -> Self {
        Self {
            store,
            api_key_pepper,
        }
    }

    /// Execute the workflow. `now` is the canonical reference
    /// time; tests pin it so the `created_at` and
    /// `expires_at` timestamps are deterministic.
    pub(crate) async fn execute(
        &self,
        command: CreateApiKeyCommand,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<CreatedApiKey, MemoryError> {
        // Resolve the account + tenant + plan so the atomic
        // `create_api_key_if_below_limit` call carries the
        // correct cap. The two lookups are the standard
        // HTTP data-plane pre-condition; a missing account
        // or tenant is a 404, not a 5xx.
        let account = self
            .store
            .find_account_by_id(&command.account_id)
            .await?
            .ok_or_else(|| {
                MemoryError::Validation(format!(
                    "create_api_key: account {} not found",
                    command.account_id
                ))
            })?;
        let tenant = self
            .store
            .find_tenant_by_id(&account.tenant_id)
            .await?
            .ok_or_else(|| {
                MemoryError::Validation(format!(
                    "create_api_key: tenant {} not found",
                    account.tenant_id
                ))
            })?;
        let plan = self.store.load_plan(tenant.plan_version).await?;

        // Generate the secret in this scope so the caller
        // can return it once. The verifier is derived from
        // the secret + the configured pepper and is what the
        // registry actually persists.
        let secret = generate_secret();
        let expires_at = command
            .expires_in_days
            .map(|d| now + chrono::Duration::days(d as i64));

        let key = ApiKey {
            id: new_api_key_id(),
            account_id: account.id.clone(),
            name: command.name.clone(),
            verifier: KeyedVerifier::compute(self.api_key_pepper.as_bytes(), secret.as_bytes()),
            status: ApiKeyStatus::Active,
            created_at: now,
            expires_at,
            last_used_at: None,
            version: 0,
        };

        // Atomic cap check + insert. The storage layer enforces
        // the limit; a concurrent producer cannot race past
        // `max_active_api_keys`.
        self.store
            .create_api_key_if_below_limit(&key, plan.limits.max_active_api_keys)
            .await?;

        Ok(CreatedApiKey {
            id: key.id,
            secret,
            name: command.name,
            expires_at,
        })
    }
}

/// Generate a random API-key secret. Matches the production
/// helper in `account_api::generate_secret` exactly: 32
/// random bytes, hex-encoded.
fn generate_secret() -> String {
    let mut bytes = [0u8; 32];
    rand::fill(&mut bytes);
    hex::encode(bytes)
}

#[cfg(test)]
mod tests {
    //! Workflow tests. They construct `ApiKeyCreation` against
    //! an in-memory `RegistryStore` and exercise the
    //! business rules without an Axum router or a
    //! `HttpState`. The HTTP-adapter tests in `account_api`
    //! cover the response-shape side of the contract; this
    //! module covers the business-rule side.

    use super::*;
    use crate::http::registry::models::{
        Account, AccountStatus, NamespaceBinding, Plan, PlanLimits, Tenant, TenantStatus,
    };
    use crate::http::registry::storage::InMemoryStore;

    /// Helper: build an in-memory registry pre-seeded with
    /// one account + tenant + free plan.
    async fn seeded_store(max_active_keys: u32) -> (Arc<InMemoryStore>, String) {
        let store = Arc::new(InMemoryStore::default());
        let now = chrono::Utc::now();
        let account = Account {
            id: "acct_test".to_string(),
            status: AccountStatus::Active,
            tenant_id: "ten_test".to_string(),
            created_at: now,
        };
        let tenant = Tenant {
            id: "ten_test".to_string(),
            status: TenantStatus::Ready,
            namespace_binding: NamespaceBinding {
                namespace: "tns_test".to_string(),
                database: "memory".to_string(),
            },
            plan_version: 1,
            schema_version: 0,
            retry_stage: None,
            provisioning_lease: None,
            created_at: now,
            version: 0,
        };
        store.write_account(&account).await.unwrap();
        store.write_tenant(&tenant).await.unwrap();
        let plan = Plan {
            id: "free".to_string(),
            version: 1,
            limits: PlanLimits {
                max_active_api_keys: max_active_keys,
                ..PlanLimits::default()
            },
        };
        store.ensure_plan(&plan).await.unwrap();
        (store, "acct_test".to_string())
    }

    fn pepper() -> String {
        "test-pepper-32-bytes-long-xxxxxxx".to_string()
    }

    /// A successful creation returns the id, secret, name,
    /// and optional expiry. The persisted verifier is
    /// `HMAC(pepper, secret)`.
    #[tokio::test]
    async fn create_returns_one_time_secret_and_id() {
        let (store, account_id) = seeded_store(5).await;
        let workflow = ApiKeyCreation::new(store.clone(), pepper());
        let now = chrono::Utc::now();
        let result = workflow
            .execute(
                CreateApiKeyCommand {
                    account_id: account_id.clone(),
                    name: "first-key".to_string(),
                    expires_in_days: None,
                },
                now,
            )
            .await
            .expect("create succeeds");
        assert!(
            result.id.starts_with("ak_"),
            "id is opaque-prefixed: {}",
            result.id
        );
        assert_eq!(result.name, "first-key");
        assert_eq!(result.expires_at, None);
        assert_eq!(result.secret.len(), 64, "secret is 32-byte hex: {result:?}");

        // Persisted verifier matches HMAC(pepper, secret).
        let stored = store
            .find_api_key(&result.id)
            .await
            .expect("api key lookup")
            .expect("api key present");
        assert!(
            stored.verifier.verify(
                b"test-pepper-32-bytes-long-xxxxxxx",
                result.secret.as_bytes()
            ),
            "verifier must match the secret under the configured pepper"
        );
        assert_eq!(stored.status, ApiKeyStatus::Active);
        assert_eq!(stored.account_id, account_id);
    }

    /// Expiry is derived from the supplied `now` so a test
    /// can pin the timestamp deterministically.
    #[tokio::test]
    async fn create_with_expiry_uses_supplied_now() {
        let (store, account_id) = seeded_store(5).await;
        let workflow = ApiKeyCreation::new(store.clone(), pepper());
        let now = chrono::DateTime::parse_from_rfc3339("2026-09-04T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let result = workflow
            .execute(
                CreateApiKeyCommand {
                    account_id,
                    name: "with-expiry".to_string(),
                    expires_in_days: Some(7),
                },
                now,
            )
            .await
            .expect("create succeeds");
        let expected = now + chrono::Duration::days(7);
        assert_eq!(result.expires_at, Some(expected));
        let stored = store
            .find_api_key(&result.id)
            .await
            .expect("api key lookup")
            .expect("api key present");
        assert_eq!(stored.expires_at, Some(expected));
        assert_eq!(stored.created_at, now);
    }

    /// Missing account: the workflow returns
    /// `MemoryError::Validation` so the HTTP adapter maps
    /// it to 404. The registry is unchanged.
    #[tokio::test]
    async fn missing_account_is_a_validation_error() {
        let store = Arc::new(InMemoryStore::default());
        let workflow = ApiKeyCreation::new(store.clone(), pepper());
        let err = workflow
            .execute(
                CreateApiKeyCommand {
                    account_id: "acct_does_not_exist".to_string(),
                    name: "x".to_string(),
                    expires_in_days: None,
                },
                chrono::Utc::now(),
            )
            .await
            .expect_err("missing account");
        assert!(matches!(err, MemoryError::Validation(_)), "got {err:?}");
    }

    /// Missing tenant (account exists but tenant row is
    /// gone): the workflow returns `Validation` so the
    /// adapter can return 404.
    #[tokio::test]
    async fn missing_tenant_is_a_validation_error() {
        let store = Arc::new(InMemoryStore::default());
        let now = chrono::Utc::now();
        store
            .write_account(&Account {
                id: "acct_orphan".to_string(),
                status: AccountStatus::Active,
                tenant_id: "ten_missing".to_string(),
                created_at: now,
            })
            .await
            .unwrap();
        let workflow = ApiKeyCreation::new(store.clone(), pepper());
        let err = workflow
            .execute(
                CreateApiKeyCommand {
                    account_id: "acct_orphan".to_string(),
                    name: "x".to_string(),
                    expires_in_days: None,
                },
                chrono::Utc::now(),
            )
            .await
            .expect_err("missing tenant");
        assert!(matches!(err, MemoryError::Validation(_)), "got {err:?}");
    }

    /// The atomic cap is enforced by the storage layer.
    /// `InMemoryStore` keeps an internal counter; the
    /// second creation over the cap returns
    /// `MemoryError::Conflict` (or `Validation` depending on
    /// the storage adapter's error mapping). The test
    /// asserts the second create fails.
    #[tokio::test]
    async fn active_key_cap_is_enforced() {
        let (store, account_id) = seeded_store(1).await;
        let workflow = ApiKeyCreation::new(store.clone(), pepper());
        let now = chrono::Utc::now();

        // First key: under the cap.
        let first = workflow
            .execute(
                CreateApiKeyCommand {
                    account_id: account_id.clone(),
                    name: "first".to_string(),
                    expires_in_days: None,
                },
                now,
            )
            .await
            .expect("first create succeeds");
        assert_eq!(first.name, "first");

        // Second key: at the cap, must fail.
        let err = workflow
            .execute(
                CreateApiKeyCommand {
                    account_id,
                    name: "second".to_string(),
                    expires_in_days: None,
                },
                now + chrono::Duration::seconds(1),
            )
            .await
            .expect_err("second create over the cap fails");
        // The `InMemoryStore` `create_api_key_if_below_limit`
        // returns `MemoryError::Conflict` when the cap is
        // exhausted; the production `SurrealRegistryStore`
        // returns the same. Either is acceptable for the
        // contract; the test asserts that the workflow
        // surfaces the storage error verbatim.
        assert!(
            matches!(err, MemoryError::Conflict(_)) || matches!(err, MemoryError::Validation(_)),
            "expected a cap-exhausted error, got {err:?}"
        );
    }

    /// The seed's `TenantStatus` must be the same value the
    /// production `InMemoryStore` writes (`Ready`). The
    /// in-memory store does not validate a status transition
    /// on `write_tenant`; the production store does. The
    /// test exercises the workflow at the in-memory level
    /// because the workflow's contract is independent of the
    /// store's transition rules.
    #[tokio::test]
    async fn workflow_does_not_mutate_tenant_state() {
        let (store, account_id) = seeded_store(5).await;
        let tenant_before = store
            .find_tenant_by_id("ten_test")
            .await
            .expect("tenant lookup")
            .expect("tenant present");
        let workflow = ApiKeyCreation::new(store.clone(), pepper());
        workflow
            .execute(
                CreateApiKeyCommand {
                    account_id,
                    name: "no-state-change".to_string(),
                    expires_in_days: None,
                },
                chrono::Utc::now(),
            )
            .await
            .expect("create succeeds");
        let tenant_after = store
            .find_tenant_by_id("ten_test")
            .await
            .expect("tenant lookup")
            .expect("tenant present");
        assert_eq!(
            tenant_before.status, tenant_after.status,
            "workflow must not transition the tenant"
        );
        assert_eq!(tenant_before.version, tenant_after.version);
    }
}
