//! Control-plane Account/Tenant endpoints.
//!
//! Implements the `/api/v1/account/*` and `/api/v1/operator/*`
//! routes mounted by `http::router` when the `control-plane`
//! feature is enabled. The HTTP adapter owns transport
//! (body parsing, headers, status codes, cookies); the
//! business workflows live in `super::application::*` so
//! each can be exercised in isolation against an in-memory
//! registry backend.
//!
//! Routes:
//!
//! - `GET    /api/v1/account`                  — read the
//!   account record + tenant status.
//! - `GET    /api/v1/account/csrf`             — return the
//!   session-bound CSRF token.
//! - `GET    /api/v1/account/api_keys`         — list
//!   non-secret API-key metadata.
//! - `POST   /api/v1/account/api_keys`         — create a
//!   new API key (one-time secret in the response body).
//! - `DELETE /api/v1/account/api_keys/:id`     — revoke
//!   an API key by id.
//! - `GET    /api/v1/account/identity_links`   — list
//!   linked external identities.
//! - `POST   /api/v1/account/identity_links`   — begin
//!   attaching a new external identity, by sending
//!   the browser to the provider. The response names
//!   no identity: the identity is the one the provider
//!   later attests to (ADR-0057).
//! - `DELETE /api/v1/account/identity_links/:id` —
//!   unlink a previously linked identity, never the
//!   last one.
//! - `POST   /api/v1/account/delete`           — start
//!   account deletion; the response carries the
//!   one-time confirmation token.
//! - `POST   /api/v1/account/delete/confirm`   — confirm
//!   account deletion with the typed phrase.
//! - `POST   /api/v1/operator/...`              —
//!   operator-only tenant lifecycle endpoints, gated
//!   by the operator identity allowlist.

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
    let account = state
        .registry
        .accounts()
        .find_account_by_id(&session.account_id)
        .await?;
    let account = account.ok_or(ApiError::NotFound)?;
    let tenant = state
        .registry
        .tenants()
        .find_tenant_by_account(&account.id)
        .await?;
    let resp = serde_json::json!({
        "account": account,
        "tenant_status": tenant.map(|t| t.status),
    });
    json_response(StatusCode::OK, &resp)
}

/// GET /api/v1/account/csrf — return the session-bound CSRF token.
///
/// The response carries `Cache-Control: no-store` because the
/// token is a per-session capability; intermediaries must
/// not cache it.
pub async fn csrf_token(
    State(state): State<Arc<HttpState>>,
    axum::extract::Extension(session): axum::extract::Extension<
        super::session::ControlPlaneSession,
    >,
) -> Result<Response, ApiError> {
    let token =
        super::csrf::compute_csrf(&state.config.keys.csrf, &session.account_id, &session.id)?;
    let mut response = json_response(StatusCode::OK, &serde_json::json!({"csrf_token": token}))?;
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    );
    Ok(response)
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

/// POST /api/v1/account/api_keys — create a new API key.
///
/// Thin HTTP adapter: parses the request body, delegates the
/// business workflow to `super::application::api_keys::ApiKeyCreation`,
/// then serializes the response. The `Cache-Control: no-store`
/// header and `201 Created` status are transport-level
/// concerns that stay here.
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

    let now = chrono::Utc::now();
    let created = crate::provisioning::api::create_api_key(
        state.api_key_issuance.as_ref(),
        crate::provisioning::api::CreateApiKeyCommand {
            account_id: session.account_id.clone(),
            name: req.name,
            expires_in_days: req.expires_in_days,
        },
        now,
    )
    .await
    .map_err(|err| match err {
        crate::error::MemoryError::NotFound(_) => ApiError::NotFound,
        crate::error::MemoryError::Conflict(_) => ApiError::Internal(
            crate::error::MemoryError::Conflict("api key cap exhausted".into()),
        ),
        other => ApiError::Internal(other),
    })?;

    let resp = CreateApiKeyResponse {
        id: created.id,
        secret: created.secret,
        name: created.name,
        expires_at: created.expires_at,
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
        .api_keys()
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
        .api_keys()
        .revoke_api_key(&session.account_id, &key_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// GET /api/v1/account/identity_links — list linked External Identities.
pub async fn list_identity_links(
    State(state): State<Arc<HttpState>>,
    axum::extract::Extension(session): axum::extract::Extension<
        super::session::ControlPlaneSession,
    >,
) -> Result<Response, ApiError> {
    let identities = state
        .registry
        .identities()
        .find_external_identities(&session.account_id)
        .await?;
    json_response(StatusCode::OK, &identities)
}

#[derive(serde::Serialize)]
pub struct LinkIdentityResponse {
    /// Where to send the browser to prove the identity it is about to attach.
    pub authorize_url: String,
}

/// POST /api/v1/account/identity_links — begin attaching an external identity
/// to this Account (ADR-0057).
///
/// The identity is *not* named here. The response sends the browser to the
/// identity provider, and the callback attaches whatever identity the provider
/// then attests to — so an account holder can only attach an identity they can
/// actually authenticate as. Naming the identity in this request, as an earlier
/// revision did, let anyone who could guess another person's `sub` claim attach
/// that identity to their own Account first.
///
/// Requires recent authentication, because the effect outlives the session.
pub async fn start_identity_link(
    State(state): State<Arc<HttpState>>,
    axum::extract::Extension(session): axum::extract::Extension<
        super::session::ControlPlaneSession,
    >,
) -> Result<Response, ApiError> {
    super::recent_auth::require_recent_auth(&session, super::recent_auth::DEFAULT_REAUTH_MAX_AGE)?;
    let authorize_url = super::oidc::start_link_flow(&state, &session.account_id).await?;
    json_response(StatusCode::OK, &LinkIdentityResponse { authorize_url })
}

/// DELETE /api/v1/account/identity_links/:id — unlink an External Identity.
///
/// The store refuses to remove the Account's last remaining identity
/// (ADR-0057): an Account with no identity left has no way back in through the
/// console, and the refusal is cheaper than a support process for undoing it.
/// The rule lives in the store rather than here so that the count and the delete
/// share one transaction. Recent authentication is required because the effect
/// outlives the session.
pub async fn unlink_identity(
    State(state): State<Arc<HttpState>>,
    axum::extract::Extension(session): axum::extract::Extension<
        super::session::ControlPlaneSession,
    >,
    axum::extract::Path(identity_id): axum::extract::Path<String>,
) -> Result<StatusCode, ApiError> {
    crate::identity::api::unlink_identity(
        state.identity_link_transactions.as_ref(),
        &crate::identity::api::UnlinkIdentityCommand {
            account_id: session.account_id.clone(),
            identity_id,
            actor: session.account_id.clone(),
            authenticated_at: session.auth_time,
        },
        chrono::Utc::now(),
    )
    .await
    .map_err(|error| match error {
        crate::identity::api::IdentityError::ReauthenticationRequired => ApiError::ReauthRequired,
        crate::identity::api::IdentityError::LastIdentityOrConflict => ApiError::Conflict,
        crate::identity::api::IdentityError::NotFound => ApiError::NotFound,
        crate::identity::api::IdentityError::IdentityHeldByAnotherAccount
        | crate::identity::api::IdentityError::MethodNotEnabled(_) => ApiError::Conflict,
        crate::identity::api::IdentityError::Persistence(error) => ApiError::Internal(error),
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// POST /api/v1/account/delete — start deletion flow and return a short-lived
/// one-use token. The token is intentionally shown once and never stored.
pub async fn start_account_deletion(
    State(state): State<Arc<HttpState>>,
    axum::extract::Extension(session): axum::extract::Extension<
        super::session::ControlPlaneSession,
    >,
) -> Result<Response, ApiError> {
    super::recent_auth::require_recent_auth(&session, super::recent_auth::DEFAULT_REAUTH_MAX_AGE)?;
    let raw_token = super::secret::random_token();
    let verifier =
        super::deletion::token_verifier(&state.config.keys.control_plane_session, &raw_token)?;
    let challenge = DeletionChallengeRecord {
        id: uuid::Uuid::new_v4().to_string(),
        verifier,
        account_id: session.account_id.clone(),
        session_id: session.id.clone(),
        expires_at: chrono::Utc::now() + chrono::Duration::minutes(5),
        consumed_at: None,
    };
    state
        .registry
        .sessions()
        .create_deletion_challenge(&challenge)
        .await?;
    let response = serde_json::json!({
        "confirmation_token": raw_token,
        "typed_phrase": super::deletion::DELETION_TYPED_PHRASE,
        "export_available": false,
        "recovery_available": false,
        "expires_at": challenge.expires_at,
    });
    let body = serde_json::to_vec(&response).map_err(|error| {
        ApiError::Internal(crate::error::MemoryError::Transient(format!(
            "serialize deletion response: {error}"
        )))
    })?;
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    );
    Ok(response)
}

#[derive(serde::Deserialize)]
pub struct ConfirmDeletionRequest {
    pub confirmation_token: String,
    pub typed_phrase: String,
}

/// POST /api/v1/account/delete/confirm — atomically consume the confirmation
/// challenge, revoke credentials/sessions, and fence data-plane access.
pub async fn confirm_account_deletion(
    State(state): State<Arc<HttpState>>,
    axum::extract::Extension(session): axum::extract::Extension<
        super::session::ControlPlaneSession,
    >,
    axum::extract::Extension(injector): axum::extract::Extension<
        std::sync::Arc<dyn crate::platform::fault_injection::FaultInjector>,
    >,
    body: Body,
) -> Result<StatusCode, ApiError> {
    super::recent_auth::require_recent_auth(&session, super::recent_auth::DEFAULT_REAUTH_MAX_AGE)?;
    let bytes = http_body_util::BodyExt::collect(body)
        .await
        .map_err(|error| ApiError::Internal(crate::error::MemoryError::Storage(error.to_string())))?
        .to_bytes();
    let request: ConfirmDeletionRequest = serde_json::from_slice(&bytes).map_err(|error| {
        ApiError::Internal(crate::error::MemoryError::Validation(format!(
            "confirm deletion body: {error}"
        )))
    })?;
    if !super::deletion::validate_typed_phrase(&request.typed_phrase) {
        return Err(ApiError::Forbidden);
    }
    let verifier = super::deletion::token_verifier(
        &state.config.keys.control_plane_session,
        &request.confirmation_token,
    )?;
    crate::operations::api::begin_account_deletion(
        state.account_deletion_port.as_ref(),
        &crate::operations::api::BeginAccountDeletionCommand {
            account_id: session.account_id.clone(),
            session_id: session.id.clone(),
            authenticated_at: session.auth_time,
            typed_phrase: request.typed_phrase,
            challenge_verifier: verifier,
        },
        chrono::Utc::now(),
    )
    .await
    .map_err(|error| match error {
        crate::operations::api::OperationsError::ReauthenticationRequired => {
            ApiError::ReauthRequired
        }
        crate::operations::api::OperationsError::ConfirmationPhraseRejected => ApiError::Forbidden,
        crate::operations::api::OperationsError::Persistence(error) => ApiError::Internal(error),
    })?;
    injector.hit(crate::platform::fault_injection::FaultPoint::AccountDeletionStarted)?;
    Ok(StatusCode::NO_CONTENT)
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
    let bundle_tx = crate::http::registry::control_impl::account_bundle_tx(state.registry.stores());
    bundle_tx
        .create_account_bundle(&account, &tenant, None)
        .await?;
    enqueue_provisioning(&state.registry.stores().provisioning(), &tenant).await?;
    account_created_response(&account)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::operator;
    use crate::control::session::ControlPlaneSession;
    use crate::http::registry::models::{
        Account, AccountStatus, ExternalIdentity, SubjectVerifier,
    };
    use crate::http::registry::storage::{AccountStore, IdentityStore, InMemoryStore};
    use axum::Router;
    use axum::body::Body;
    use axum::extract::{Extension, State};
    use axum::http::{Request, StatusCode};
    use axum::routing::post;
    use tower_service::Service;

    /// Write an Account and `count` linked identities, returning the identity
    /// ids in insertion order.
    async fn account_with_identities(store: &Arc<InMemoryStore>, count: u8) -> Vec<String> {
        let account_id = "acct_links";
        store
            .write_account(&Account {
                id: account_id.to_owned(),
                status: AccountStatus::Active,
                tenant_id: "ten_links".to_owned(),
                created_at: chrono::Utc::now(),
            })
            .await
            .expect("write account");
        let mut ids = Vec::new();
        for index in 0..count {
            let id = format!("idn_{index}");
            store
                .link_external_identity(
                    &ExternalIdentity {
                        id: id.clone(),
                        issuer: "https://idp.example.com".to_owned(),
                        subject_verifier: SubjectVerifier([index; 32]),
                        account_id: account_id.to_owned(),
                        created_at: chrono::Utc::now(),
                    },
                    &IdentityAudit::by_account(account_id, chrono::Utc::now()),
                )
                .await
                .expect("link identity");
            ids.push(id);
        }
        ids
    }

    /// The `identity_unlinked` audit row for `identity_id`, if one was written.
    fn unlink_audit(store: &InMemoryStore, identity_id: &str) -> Option<ControlAuditEvent> {
        store.audit_events().into_iter().find(|event| {
            event.action == "identity_unlinked"
                && event.target_identity_id.as_deref() == Some(identity_id)
        })
    }

    /// ADR-0057: an Account must never be left with no identity it can sign in
    /// with, so the last one cannot be removed.
    #[tokio::test]
    async fn the_last_identity_cannot_be_unlinked() {
        let store: Arc<InMemoryStore> = Arc::new(InMemoryStore::default());
        let ids = account_with_identities(&store, 1).await;

        let refused = store
            .unlink_external_identity(
                "acct_links",
                &ids[0],
                &IdentityAudit::by_account("acct_links", chrono::Utc::now()),
            )
            .await;
        assert!(
            matches!(refused, Err(crate::error::MemoryError::Conflict(_))),
            "the only identity must not be removable"
        );
        assert_eq!(
            store
                .find_external_identities("acct_links")
                .await
                .expect("list")
                .len(),
            1,
            "the refusal must not have removed anything"
        );
        assert!(
            unlink_audit(&store, &ids[0]).is_none(),
            "a refused removal must not be audited as one"
        );
    }

    /// With a second identity present the removal proceeds, so the guard above
    /// cannot pass for the wrong reason.
    #[tokio::test]
    async fn an_identity_is_removable_while_another_remains() {
        let store: Arc<InMemoryStore> = Arc::new(InMemoryStore::default());
        let ids = account_with_identities(&store, 2).await;

        store
            .unlink_external_identity(
                "acct_links",
                &ids[0],
                &IdentityAudit::by_account("acct_links", chrono::Utc::now()),
            )
            .await
            .expect("unlink one of two");
        let remaining = store
            .find_external_identities("acct_links")
            .await
            .expect("list");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, ids[1]);
    }

    /// The two halves of an identity's lifetime are audited, each naming the
    /// identity it acted on (ADR-0057).
    #[tokio::test]
    async fn linking_and_unlinking_are_audited_with_the_identity() {
        let store: Arc<InMemoryStore> = Arc::new(InMemoryStore::default());
        let ids = account_with_identities(&store, 2).await;
        store
            .unlink_external_identity(
                "acct_links",
                &ids[0],
                &IdentityAudit::by_account("acct_links", chrono::Utc::now()),
            )
            .await
            .expect("unlink one of two");

        let linked: Vec<_> = store
            .audit_events()
            .into_iter()
            .filter(|event| event.action == "identity_linked")
            .collect();
        assert_eq!(linked.len(), 2, "one row per link");
        assert_eq!(
            linked[0].target_identity_id.as_deref(),
            Some(ids[0].as_str())
        );
        assert_eq!(linked[0].account_id, "acct_links");
        assert_eq!(linked[0].actor_kind, AuditActorKind::Account);
        assert_eq!(linked[0].actor_principal, "acct_links");
        assert!(
            linked[0].correlation_id != linked[1].correlation_id,
            "each row needs its own durable id"
        );

        let unlinked = unlink_audit(&store, &ids[0]).expect("unlink is audited");
        assert_eq!(unlinked.account_id, "acct_links");
        assert_eq!(unlinked.actor_kind, AuditActorKind::Account);
    }

    /// Build a router with the operator stub middleware and an
    /// in-memory registry store. The test passes because the
    /// production registry handle is replaced with
    /// `RegistryHandle::in_memory()`.
    async fn build_test_router() -> (Router, Arc<InMemoryStore>) {
        let store: Arc<InMemoryStore> = Arc::new(InMemoryStore::default());
        let registry =
            crate::http::registry::RegistryHandle::in_memory().with_inner_store(store.clone());
        let state = crate::http::test_state::HttpStateTestBuilder::new()
            .await
            .with_registry(registry)
            .build()
            .await
            .expect("test HTTP state");
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

    /// A fresh in-memory `HttpState` for the session-scoped handlers.
    async fn state() -> Arc<crate::http::HttpState> {
        crate::http::HttpState::default_for_test().await
    }

    /// A control-plane session bound to `account_id`.
    fn session_for(account_id: &str) -> ControlPlaneSession {
        let now = chrono::Utc::now();
        ControlPlaneSession {
            id: "sess-1".to_owned(),
            cookie_hash: "hash".to_owned(),
            account_id: account_id.to_owned(),
            browser_policy_epoch: Some(1),
            auth_time: now,
            idle_expiry: now + chrono::Duration::minutes(30),
            absolute_expiry: now + chrono::Duration::hours(24),
        }
    }

    /// Seed an active account into the state's registry.
    async fn seed_account(state: &Arc<crate::http::HttpState>, account_id: &str) {
        state
            .registry
            .accounts()
            .write_account(&Account {
                id: account_id.to_owned(),
                status: AccountStatus::Active,
                tenant_id: "ten_1".to_owned(),
                created_at: chrono::Utc::now(),
            })
            .await
            .expect("write account");
    }

    /// Read a JSON response body.
    async fn json_of(response: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("body reads");
        serde_json::from_slice(&bytes).expect("json body")
    }

    #[tokio::test]
    async fn reading_an_unknown_account_is_a_not_found() {
        let state = state().await;

        let observed = get_account(State(state), Extension(session_for("acc_missing"))).await;

        assert!(matches!(observed, Err(ApiError::NotFound)));
    }

    #[tokio::test]
    async fn reading_a_known_account_succeeds() {
        let state = state().await;
        seed_account(&state, "acc_1").await;

        let observed = get_account(State(state), Extension(session_for("acc_1"))).await;

        assert!(observed.is_ok());
    }

    #[tokio::test]
    async fn the_account_response_carries_the_account_id() {
        let state = state().await;
        seed_account(&state, "acc_1").await;

        let observed = get_account(State(state), Extension(session_for("acc_1")))
            .await
            .ok()
            .expect("account reads");

        assert_eq!(json_of(observed).await["account"]["id"], "acc_1");
    }

    #[tokio::test]
    async fn an_account_with_no_tenant_reports_a_null_tenant_status() {
        // The account exists but provisioning has not produced a tenant yet, so
        // the response must say "none" rather than inventing a status.
        let state = state().await;
        seed_account(&state, "acc_1").await;

        let observed = get_account(State(state), Extension(session_for("acc_1")))
            .await
            .ok()
            .expect("account reads");

        assert!(json_of(observed).await["tenant_status"].is_null());
    }

    #[tokio::test]
    async fn the_csrf_endpoint_returns_a_token() {
        let state = state().await;

        let observed = csrf_token(State(state), Extension(session_for("acc_1"))).await;

        assert!(observed.is_ok());
    }

    #[tokio::test]
    async fn the_csrf_response_carries_the_token() {
        let state = state().await;

        let observed = csrf_token(State(state), Extension(session_for("acc_1")))
            .await
            .ok()
            .expect("csrf reads");

        let token = json_of(observed).await["csrf_token"]
            .as_str()
            .expect("token is a string")
            .to_string();

        assert_eq!(token.len(), 64, "a CSRF token is 32 hex-encoded bytes");
    }

    #[tokio::test]
    async fn the_csrf_response_is_not_cacheable() {
        let state = state().await;

        let observed = csrf_token(State(state), Extension(session_for("acc_1")))
            .await
            .ok()
            .expect("csrf reads");

        assert_eq!(
            observed
                .headers()
                .get(axum::http::header::CACHE_CONTROL)
                .expect("cache-control is set"),
            "no-store",
            "a per-session capability must not be cached by intermediaries"
        );
    }

    #[tokio::test]
    async fn two_sessions_receive_different_csrf_tokens() {
        let state = state().await;
        let mut other = session_for("acc_1");
        other.id = "sess-2".to_owned();

        let first = csrf_token(State(state.clone()), Extension(session_for("acc_1")))
            .await
            .ok()
            .expect("csrf reads");
        let second = csrf_token(State(state), Extension(other))
            .await
            .ok()
            .expect("csrf reads");

        assert_ne!(
            json_of(first).await["csrf_token"],
            json_of(second).await["csrf_token"],
            "a CSRF token is bound to its session"
        );
    }
}
