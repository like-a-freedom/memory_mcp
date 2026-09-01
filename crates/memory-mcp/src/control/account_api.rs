//! Control-plane Account/Tenant endpoints.
//!
//! Stub: create-account writes a reserved Tenant + the
//! matching Account, then enqueues a provisioning event.

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
fn account_created_response(account: &Account) -> Result<Response, ApiError> {
    let body = serde_json::to_vec(account).map_err(|error| {
        ApiError::Internal(crate::error::MemoryError::Transient(format!(
            "serialize account response: {error}"
        )))
    })?;
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = StatusCode::CREATED;
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    Ok(response)
}

// Build a JSON response from serializable data.
fn json_response<T: serde::Serialize>(status: StatusCode, data: &T) -> Result<Response, ApiError> {
    let body = serde_json::to_vec(data).map_err(|error| {
        ApiError::Internal(crate::error::MemoryError::Transient(format!(
            "serialize JSON response: {error}"
        )))
    })?;
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    Ok(response)
}

// ---------------------------------------------------------------------------
// Account management endpoints
// ---------------------------------------------------------------------------

/// GET /api/v1/account — read account metadata + tenant status.
pub async fn get_account(
    State(state): State<Arc<HttpState>>,
    axum::extract::Extension(session): axum::extract::Extension<
        super::session::ControlPlaneSession,
    >,
) -> Result<Response, ApiError> {
    let store = state.registry.store_clone();
    let account = store.find_account_by_id(&session.account_id).await?;
    let account = account.ok_or(ApiError::NotFound)?;
    let tenant = store.find_tenant_by_account(&account.id).await?;
    let resp = serde_json::json!({
        "account": account,
        "tenant_status": tenant.map(|t| t.status),
    });
    json_response(StatusCode::OK, &resp)
}

/// Request body for creating an API key.
#[derive(serde::Deserialize)]
pub struct CreateApiKeyRequest {
    pub name: String,
    /// Optional expiry in days from now.
    pub expires_in_days: Option<u32>,
}

/// Response body for creating an API key (secret shown once).
#[derive(serde::Serialize)]
pub struct CreateApiKeyResponse {
    pub id: String,
    pub secret: String,
    pub name: String,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Generate a random API key secret.
fn generate_secret() -> String {
    let mut bytes = [0u8; 32];
    rand::fill(&mut bytes);
    hex::encode(bytes)
}

/// POST /api/v1/account/api_keys — create a new API key.
pub async fn create_api_key(
    State(state): State<Arc<HttpState>>,
    axum::extract::Extension(session): axum::extract::Extension<
        super::session::ControlPlaneSession,
    >,
    body: axum::body::Body,
) -> Result<
    (
        StatusCode,
        [(axum::http::header::HeaderName, HeaderValue); 1],
        Response,
    ),
    ApiError,
> {
    use http_body_util::BodyExt;

    let bytes = body
        .collect()
        .await
        .map_err(|e| ApiError::Internal(crate::error::MemoryError::Storage(e.to_string())))?
        .to_bytes();
    let req: CreateApiKeyRequest = if bytes.is_empty() {
        CreateApiKeyRequest {
            name: "default".to_string(),
            expires_in_days: None,
        }
    } else {
        serde_json::from_slice(&bytes).map_err(|e| {
            ApiError::Internal(crate::error::MemoryError::Validation(format!(
                "create-api-key: {e}"
            )))
        })?
    };

    let secret = generate_secret();
    let expires_at = req
        .expires_in_days
        .map(|d| chrono::Utc::now() + chrono::Duration::days(d as i64));

    let key = ApiKey {
        id: new_api_key_id(),
        account_id: session.account_id.clone(),
        name: req.name.clone(),
        verifier: KeyedVerifier::compute(state.config.api_key_pepper.as_bytes(), secret.as_bytes()),
        status: ApiKeyStatus::Active,
        created_at: chrono::Utc::now(),
        expires_at,
        last_used_at: None,
        version: 0,
    };

    state.registry.store_clone().write_api_key(&key).await?;

    let resp = CreateApiKeyResponse {
        id: key.id.clone(),
        secret,
        name: req.name,
        expires_at,
    };

    let headers = [(
        axum::http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    )];

    let body = serde_json::to_vec(&resp).map_err(|error| {
        ApiError::Internal(crate::error::MemoryError::Transient(format!(
            "serialize API key response: {error}"
        )))
    })?;
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = StatusCode::CREATED;
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );

    Ok((StatusCode::CREATED, headers, response))
}

/// GET /api/v1/account/api_keys — list API keys (without secrets).
pub async fn list_api_keys(
    State(state): State<Arc<HttpState>>,
    axum::extract::Extension(session): axum::extract::Extension<
        super::session::ControlPlaneSession,
    >,
) -> Result<Response, ApiError> {
    let keys = state
        .registry
        .store_clone()
        .list_api_keys(&session.account_id)
        .await?;
    json_response(StatusCode::OK, &keys)
}

/// DELETE /api/v1/account/api_keys/:id — revoke an API key.
pub async fn revoke_api_key(
    State(state): State<Arc<HttpState>>,
    axum::extract::Extension(session): axum::extract::Extension<
        super::session::ControlPlaneSession,
    >,
    axum::extract::Path(key_id): axum::extract::Path<String>,
) -> Result<StatusCode, ApiError> {
    state
        .registry
        .store_clone()
        .revoke_api_key(&session.account_id, &key_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// GET /api/v1/account/identity_links — list linked External Identities.
pub async fn list_identity_links(
    _state: State<Arc<HttpState>>,
    _session: axum::extract::Extension<super::session::ControlPlaneSession>,
) -> Result<Response, ApiError> {
    // ExternalIdentity linking is not wired yet; fail closed
    // instead of presenting an empty list as authoritative.
    Err(ApiError::Unavailable)
}

/// DELETE /api/v1/account/identity_links/:id — unlink an External Identity.
pub async fn unlink_identity(
    _state: State<Arc<HttpState>>,
    _session: axum::extract::Extension<super::session::ControlPlaneSession>,
    identity_id: axum::extract::Path<String>,
) -> Result<StatusCode, ApiError> {
    // ExternalIdentity unlinking is not wired yet; do not
    // report a successful or misleading not-found mutation.
    let _ = identity_id;
    Err(ApiError::Unavailable)
}

/// POST /api/v1/account/delete — start deletion flow (sends confirmation).
pub async fn start_account_deletion(
    _state: State<Arc<HttpState>>,
    _session: axum::extract::Extension<super::session::ControlPlaneSession>,
) -> Result<Response, ApiError> {
    // A confirmation token is not durable yet, so do not
    // advertise a deletion flow that cannot be completed.
    Err(ApiError::Unavailable)
}

/// POST /api/v1/account/delete/confirm — confirm with typed phrase.
pub async fn confirm_account_deletion(
    _state: State<Arc<HttpState>>,
    _session: axum::extract::Extension<super::session::ControlPlaneSession>,
) -> Result<StatusCode, ApiError> {
    // Deletion flow stub; return stub for now.
    Err(ApiError::Unavailable)
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
    account_created_response(&account)
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
        let registry =
            crate::http::registry::RegistryHandle::in_memory().with_inner_store(store.clone());
        let pool = Arc::new(crate::http::runtime::pool::Pool::with_defaults(
            std::sync::Arc::new(registry.clone()),
        ));
        let state = Arc::new(HttpState {
            config: crate::http::config::HttpConfig::default_for_test(),
            pool,
            shutdown: crate::http::shutdown::ShutdownState::new(),
            admission: Arc::new(crate::http::runtime::pool::AdmissionGate::open()),
            registry,
            authenticator,
            account_resolver,
            #[cfg(feature = "control-plane")]
            oidc_client: None,
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
