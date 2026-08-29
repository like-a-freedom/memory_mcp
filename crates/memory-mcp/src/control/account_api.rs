//! Control-plane Account/Tenant endpoints (ADR-0052, plan §4.7).
//!
//! Phase 4 stub: create-account writes a reserved Tenant + the
//! matching Account, then enqueues a provisioning event. The
//! actual provisioning state machine lands in Task 5.1; the
//! scheduler lands in Task 6.2.

use std::sync::Arc;

use axum::Extension;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;

use super::error::ApiError;
use super::operator::OperatorPrincipal;
use crate::http::HttpState;
use crate::http::registry::models::*;
use crate::http::registry::provisioning::enqueue_provisioning;

#[derive(serde::Deserialize)]
pub struct CreateAccountRequest {
    pub display_name: Option<String>,
}

pub async fn create_account(
    State(state): State<Arc<HttpState>>,
    Extension(operator): Extension<OperatorPrincipal>,
    Json(_req): Json<CreateAccountRequest>,
) -> Result<(StatusCode, Json<Account>), ApiError> {
    operator.require_recent_auth()?;
    let account = Account {
        id: new_account_id(),
        status: AccountStatus::Active,
        tenant_id: new_tenant_id(),
        created_at: chrono::Utc::now(),
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
        created_at: chrono::Utc::now(),
        version: 0,
    };
    let store = state.registry.store_clone();
    store.write_account(&account).await?;
    store.write_tenant(&tenant).await?;
    enqueue_provisioning(&store, &tenant).await?;
    Ok((StatusCode::CREATED, Json(account)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::operator;
    use crate::http::registry::RegistryHandle;
    use crate::http::registry::RegistryStore;
    use crate::http::registry::account::AccountResolver;
    use crate::http::registry::models::{
        Account, AccountStatus, ApiKey, ApiKeyMeta, Plan, Tenant, TenantStatus, UsageCounter,
    };
    use async_trait::async_trait;
    use std::sync::Mutex;

    /// Records every call the test driver makes.
    #[derive(Default)]
    struct CallLog {
        accounts: Mutex<Vec<Account>>,
        tenants: Mutex<Vec<Tenant>>,
        events: Mutex<Vec<(String, String)>>,
    }

    /// In-memory RegistryStore for the create_account flow.
    /// Tracks calls; satisfies every trait method with `unimplemented!()`
    /// for paths the flow does not exercise.
    struct TestStore {
        log: std::sync::Arc<CallLog>,
    }

    #[async_trait]
    impl RegistryStore for TestStore {
        async fn ping(&self) -> bool {
            true
        }
        async fn find_account_by_id(
            &self,
            _id: &str,
        ) -> Result<Option<Account>, crate::error::MemoryError> {
            Ok(None)
        }
        async fn find_account_by_identity(
            &self,
            _issuer: &str,
            _sv: &[u8; 32],
        ) -> Result<Option<Account>, crate::error::MemoryError> {
            Ok(None)
        }
        async fn find_tenant_by_account(
            &self,
            _account_id: &str,
        ) -> Result<Option<Tenant>, crate::error::MemoryError> {
            Ok(None)
        }
        async fn find_tenant_by_id(
            &self,
            _id: &str,
        ) -> Result<Option<Tenant>, crate::error::MemoryError> {
            Ok(None)
        }
        async fn find_api_key(&self, _: &str) -> Result<Option<ApiKey>, crate::error::MemoryError> {
            unimplemented!()
        }
        async fn write_api_key(&self, _: &ApiKey) -> Result<(), crate::error::MemoryError> {
            unimplemented!()
        }
        async fn list_api_keys(
            &self,
            _: &str,
        ) -> Result<Vec<ApiKeyMeta>, crate::error::MemoryError> {
            unimplemented!()
        }
        async fn revoke_api_key(&self, _: &str, _: &str) -> Result<(), crate::error::MemoryError> {
            unimplemented!()
        }
        async fn touch_api_key(
            &self,
            _: &str,
            _: chrono::DateTime<chrono::Utc>,
        ) -> Result<(), crate::error::MemoryError> {
            unimplemented!()
        }
        async fn write_account(&self, account: &Account) -> Result<(), crate::error::MemoryError> {
            self.log.accounts.lock().unwrap().push(account.clone());
            Ok(())
        }
        async fn write_tenant(&self, tenant: &Tenant) -> Result<(), crate::error::MemoryError> {
            self.log.tenants.lock().unwrap().push(tenant.clone());
            Ok(())
        }
        async fn update_tenant_state(
            &self,
            _: &str,
            _: u64,
            _: TenantStatus,
            _: TenantStatus,
        ) -> Result<u64, crate::error::MemoryError> {
            unimplemented!()
        }
        async fn update_tenant_schema_version(
            &self,
            _: &str,
            _: u64,
            _: u32,
        ) -> Result<u64, crate::error::MemoryError> {
            unimplemented!()
        }
        async fn update_tenant_state_fenced(
            &self,
            _: &str,
            _: u64,
            _: TenantStatus,
            _: TenantStatus,
            _: &str,
            _: &str,
            _: u64,
        ) -> Result<u64, crate::error::MemoryError> {
            unimplemented!()
        }
        async fn update_tenant_schema_version_fenced(
            &self,
            _: &str,
            _: u64,
            _: u32,
            _: &str,
            _: &str,
            _: u64,
        ) -> Result<u64, crate::error::MemoryError> {
            unimplemented!()
        }
        async fn append_provisioning_event(
            &self,
            tenant_id: &str,
            stage: &str,
        ) -> Result<(), crate::error::MemoryError> {
            self.log
                .events
                .lock()
                .unwrap()
                .push((tenant_id.to_string(), stage.to_string()));
            Ok(())
        }
        async fn load_plan(&self, _: &str) -> Result<Plan, crate::error::MemoryError> {
            unimplemented!()
        }
        async fn increment_usage(
            &self,
            _: &str,
            _: UsageCounter,
            _: u64,
        ) -> Result<u64, crate::error::MemoryError> {
            unimplemented!()
        }
        async fn list_due_provisioning(
            &self,
            _: u32,
            _: chrono::DateTime<chrono::Utc>,
        ) -> Result<Vec<Tenant>, crate::error::MemoryError> {
            unimplemented!()
        }
        async fn claim_provisioning(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: chrono::DateTime<chrono::Utc>,
            _: chrono::DateTime<chrono::Utc>,
        ) -> Result<Option<crate::http::leases::ProvisioningLease>, crate::error::MemoryError>
        {
            unimplemented!()
        }
        async fn heartbeat_provisioning(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: u64,
            _: chrono::DateTime<chrono::Utc>,
            _: chrono::DateTime<chrono::Utc>,
        ) -> Result<(), crate::error::MemoryError> {
            unimplemented!()
        }
        async fn release_provisioning(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: u64,
        ) -> Result<(), crate::error::MemoryError> {
            unimplemented!()
        }
    }

    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::post;
    use tower_service::Service;

    /// Tests build a router with the operator stub middleware so
    /// the unauthenticated path does not short-circuit.
    async fn build_test_router(
        log: std::sync::Arc<CallLog>,
    ) -> (Router, std::sync::Arc<HttpState>) {
        let store: Arc<dyn RegistryStore> = Arc::new(TestStore { log });
        let cache = Arc::new(crate::http::principal::cache::PrincipalCache::new(8));
        let rate = Arc::new(crate::http::principal::auth::RateLimiter::new(
            4,
            std::time::Duration::from_secs(60),
            100,
        ));
        let authenticator = Arc::new(crate::http::principal::auth::Authenticator::new(
            store.clone(),
            cache,
            b"pepper".to_vec(),
            rate,
        ));
        let account_resolver = Arc::new(AccountResolver::new(store.clone()));
        let shared_handler = crate::http::transport::build_tenantless_handler(
            &crate::http::config::HttpConfig::default_for_test(),
        )
        .await
        .expect("tenantless handler builds in test");
        let core = Arc::new(crate::http::HttpStateCore {
            config: crate::http::config::HttpConfig::default_for_test(),
            shared_handler,
            shutdown: crate::http::shutdown::ShutdownState::new(),
            admission: Arc::new(crate::http::runtime::pool::AdmissionGate::new()),
            registry: RegistryHandle {
                store: store.clone(),
            },
            authenticator,
            account_resolver,
        });
        let state = std::sync::Arc::new(HttpState { core });
        let router = Router::new()
            .route(
                "/api/v1/operator/accounts",
                post(create_account)
                    .layer(axum::middleware::from_fn(operator::stub_operator_inject)),
            )
            .with_state(state.clone());
        (router, state)
    }

    #[tokio::test]
    async fn create_account_writes_registry_records_and_enqueues_provisioning() {
        let log = std::sync::Arc::new(CallLog::default());
        let (mut router, _state) = build_test_router(log.clone()).await;
        let req = Request::builder()
            .method("POST")
            .uri("/api/v1/operator/accounts")
            .header("x-operator-auth", "stub")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"display_name":"test"}"#))
            .unwrap();
        let resp = router.call(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body_bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .unwrap();
        let account: Account = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(account.status, AccountStatus::Active);
        assert!(account.tenant_id.starts_with("ten_"));

        let accounts = log.accounts.lock().unwrap();
        let tenants = log.tenants.lock().unwrap();
        let events = log.events.lock().unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(tenants.len(), 1);
        assert_eq!(tenants[0].status, TenantStatus::Reserved);
        assert_eq!(tenants[0].namespace_binding.database, "memory");
        assert_eq!(
            events.as_slice(),
            &[(account.tenant_id.clone(), "reserved".to_string())]
        );
    }
}
