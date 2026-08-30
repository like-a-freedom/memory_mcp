//! Control-plane Account/Tenant endpoints (ADR-0052, plan §4.7).
//!
//! Phase 4 stub: create-account writes a reserved Tenant + the
//! matching Account, then enqueues a provisioning event. The
//! actual provisioning state machine lands in Task 5.1; the
//! scheduler lands in Task 6.2.

use std::sync::Arc;

use axum::Extension;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode};
use axum::response::Response;

use super::error::ApiError;
use super::operator::OperatorPrincipal;
use crate::http::HttpState;
use crate::http::registry::models::*;
use crate::http::registry::provisioning::enqueue_provisioning;

#[derive(serde::Deserialize)]
pub struct CreateAccountRequest {
    pub display_name: Option<String>,
}

/// Build a JSON 201 Created response carrying the Account
/// body. Manual because the workspace does not enable axum's
/// `json` feature; serde_json::to_vec keeps the JSON contract
/// under our control.
fn account_created_response(account: &Account) -> Response {
    let body = serde_json::to_vec(account).expect("Account serializes");
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = StatusCode::CREATED;
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

/// Parse the request body into a `CreateAccountRequest`. The
/// body is optional: an empty body is treated as "no fields set".
async fn read_create_account(body: Body) -> Result<CreateAccountRequest, ApiError> {
    use http_body_util::BodyExt;
    let bytes = body
        .collect()
        .await
        .map_err(|err| ApiError::Internal(crate::error::MemoryError::Storage(err.to_string())))?
        .to_bytes();
    if bytes.is_empty() {
        return Ok(CreateAccountRequest { display_name: None });
    }
    serde_json::from_slice(&bytes).map_err(|err| {
        ApiError::Internal(crate::error::MemoryError::Validation(format!(
            "create-account body: {err}"
        )))
    })
}

pub async fn create_account(
    State(state): State<Arc<HttpState>>,
    Extension(operator): Extension<OperatorPrincipal>,
    req: axum::extract::Request,
) -> Result<Response, ApiError> {
    operator.require_recent_auth()?;
    let _req: CreateAccountRequest = read_create_account(req.into_body()).await?;
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
    Ok(account_created_response(&account))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::operator;
    use crate::http::registry::account::AccountResolver;
    use crate::http::registry::storage::InMemoryStore;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::post;
    use tower_service::Service;

    /// Build a router with the operator stub middleware and an
    /// in-memory registry store. The test passes because the
    /// production registry handle is replaced with
    /// `RegistryHandle::in_memory()`.
    async fn build_test_router() -> (Router, Arc<InMemoryStore>) {
        let store: Arc<InMemoryStore> = Arc::new(InMemoryStore::default());
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
        let state = Arc::new(HttpState {
            config: crate::http::config::HttpConfig::default_for_test(),
            shared_handler,
            shutdown: crate::http::shutdown::ShutdownState::new(),
            admission: Arc::new(crate::http::runtime::pool::AdmissionGate::new()),
            registry: crate::http::registry::RegistryHandle {
                store: store.clone(),
            },
            authenticator,
            account_resolver,
        });
        let router = Router::new()
            .route(
                "/api/v1/operator/accounts",
                post(create_account)
                    .layer(axum::middleware::from_fn(operator::stub_operator_inject)),
            )
            .with_state(state);
        (router, store)
    }

    #[tokio::test]
    async fn create_account_writes_registry_records_and_enqueues_provisioning() {
        let (mut router, store) = build_test_router().await;
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

        let events = store.provisioning_events();
        assert_eq!(events.len(), 1);
        let (tid, stage) = &events[0];
        assert_eq!(tid, &account.tenant_id);
        assert_eq!(stage, "reserved");
    }
}
