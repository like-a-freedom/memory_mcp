//! Operator principal seam.
//!
//! Stub: with the `test-fixtures` feature, the stub
//! middleware accepts `X-Operator-Auth: stub`; without
//! `test-fixtures` there is no operator injection
//! (operator endpoints are unreachable until OIDC).
//!
//! OIDC replaces this with derived operator
//! identity; the accessor name (`require_recent_auth`) stays.

use std::sync::Arc;

#[cfg(any(test, feature = "test-fixtures"))]
use axum::http::StatusCode;
#[cfg(any(test, feature = "test-fixtures"))]
use axum::middleware::Next;
#[cfg(any(test, feature = "test-fixtures"))]
use axum::response::Response;

use super::error::ApiError;

#[derive(Clone)]
pub struct OperatorPrincipal {
    pub authenticated_at: chrono::DateTime<chrono::Utc>,
}

impl OperatorPrincipal {
    /// Require a recent operator authentication event for
    /// destructive control-plane actions.
    pub fn require_recent_auth(&self) -> Result<(), ApiError> {
        let age = chrono::Utc::now() - self.authenticated_at;
        if age < chrono::Duration::zero() || age > chrono::Duration::seconds(600) {
            return Err(ApiError::ReauthRequired);
        }
        Ok(())
    }
}

/// Test-fixtures-only stub middleware. Injects the operator
/// principal.
#[cfg(any(test, feature = "test-fixtures"))]
pub async fn stub_operator_inject(
    mut req: axum::extract::Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let is_stub = req
        .headers()
        .get("x-operator-auth")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == "stub");
    if !is_stub {
        return Err(StatusCode::UNAUTHORIZED);
    }
    req.extensions_mut().insert(OperatorPrincipal {
        authenticated_at: chrono::Utc::now(),
    });
    Ok(next.run(req).await)
}

/// Builder for the test operator router. Only compiled under
/// `test-fixtures` so a data-plane-only build never exposes
/// operator endpoints.
#[cfg(any(test, feature = "test-fixtures"))]
pub fn test_operator_router(state: Arc<crate::http::HttpState>) -> axum::Router {
    axum::Router::new()
        .route(
            "/api/v1/operator/accounts",
            axum::routing::post(super::account_api::create_account),
        )
        .layer(axum::middleware::from_fn(stub_operator_inject))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Operator API endpoints
// ---------------------------------------------------------------------------

/// GET /api/v1/operator/tenants/:id — read provisioning state.
pub async fn get_tenant(
    axum::extract::State(state): axum::extract::State<Arc<crate::http::HttpState>>,
    axum::extract::Extension(operator): axum::extract::Extension<OperatorPrincipal>,
    axum::extract::Path(tenant_id): axum::extract::Path<String>,
) -> Result<axum::response::Response, super::error::ApiError> {
    operator.require_recent_auth()?;
    let tenant = state
        .registry
        .tenants()
        .find_tenant_by_id(&tenant_id)
        .await?;
    let tenant = tenant.ok_or(super::error::ApiError::NotFound)?;
    let body = serde_json::to_vec(&tenant).map_err(|error| {
        super::error::ApiError::Internal(crate::error::MemoryError::Transient(format!(
            "serialize tenant response: {error}"
        )))
    })?;
    let mut response = axum::response::Response::new(axum::body::Body::from(body));
    *response.status_mut() = axum::http::StatusCode::OK;
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/json"),
    );
    Ok(response)
}

/// POST /api/v1/operator/tenants/:id/retry — retry failed provisioning stage.
pub async fn retry_tenant(
    axum::extract::State(state): axum::extract::State<Arc<crate::http::HttpState>>,
    axum::extract::Extension(operator): axum::extract::Extension<OperatorPrincipal>,
    axum::extract::Path(tenant_id): axum::extract::Path<String>,
) -> Result<axum::http::StatusCode, super::error::ApiError> {
    operator.require_recent_auth()?;
    let store = state.registry.tenants();
    let tenant = store
        .find_tenant_by_id(&tenant_id)
        .await?
        .ok_or(super::error::ApiError::NotFound)?;
    if tenant.status != crate::http::registry::models::TenantStatus::Failed {
        return Err(super::error::ApiError::Conflict);
    }
    let stage = tenant
        .retry_stage
        .unwrap_or(crate::http::registry::models::TenantStatus::Reserved);
    store
        .update_tenant_state(
            &tenant.id,
            tenant.version,
            crate::http::registry::models::TenantStatus::Failed,
            stage,
        )
        .await?;
    Ok(axum::http::StatusCode::ACCEPTED)
}

/// POST /api/v1/operator/tenants/:id/suspend — suspend a tenant.
pub async fn suspend_tenant(
    axum::extract::State(state): axum::extract::State<Arc<crate::http::HttpState>>,
    axum::extract::Extension(operator): axum::extract::Extension<OperatorPrincipal>,
    axum::extract::Path(tenant_id): axum::extract::Path<String>,
) -> Result<axum::http::StatusCode, super::error::ApiError> {
    operator.require_recent_auth()?;
    let store = state.registry.tenants();
    let tenant = store
        .find_tenant_by_id(&tenant_id)
        .await?
        .ok_or(super::error::ApiError::NotFound)?;
    if matches!(
        tenant.status,
        crate::http::registry::models::TenantStatus::Suspended
    ) {
        return Ok(axum::http::StatusCode::NO_CONTENT);
    }
    if matches!(
        tenant.status,
        crate::http::registry::models::TenantStatus::Deleting
            | crate::http::registry::models::TenantStatus::Purged
    ) {
        return Err(super::error::ApiError::Conflict);
    }
    store
        .update_tenant_state(
            &tenant.id,
            tenant.version,
            tenant.status,
            crate::http::registry::models::TenantStatus::Suspended,
        )
        .await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// POST /api/v1/operator/tenants/:id/resume — resume a suspended tenant.
pub async fn resume_tenant(
    axum::extract::State(state): axum::extract::State<Arc<crate::http::HttpState>>,
    axum::extract::Extension(operator): axum::extract::Extension<OperatorPrincipal>,
    axum::extract::Path(tenant_id): axum::extract::Path<String>,
) -> Result<axum::http::StatusCode, super::error::ApiError> {
    operator.require_recent_auth()?;
    let store = state.registry.tenants();
    let tenant = store
        .find_tenant_by_id(&tenant_id)
        .await?
        .ok_or(super::error::ApiError::NotFound)?;
    store
        .update_tenant_state(
            &tenant.id,
            tenant.version,
            crate::http::registry::models::TenantStatus::Suspended,
            crate::http::registry::models::TenantStatus::Ready,
        )
        .await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// POST /api/v1/operator/tenants/:id/purge — initiate Account deletion.
pub async fn purge_tenant(
    axum::extract::State(state): axum::extract::State<Arc<crate::http::HttpState>>,
    axum::extract::Extension(operator): axum::extract::Extension<OperatorPrincipal>,
    axum::extract::Path(tenant_id): axum::extract::Path<String>,
) -> Result<axum::http::StatusCode, super::error::ApiError> {
    operator.require_recent_auth()?;
    let store = state.registry.tenants();
    let tx = crate::http::registry::control_impl::account_deletion_tx(state.registry.stores());
    let tenant = store
        .find_tenant_by_id(&tenant_id)
        .await?
        .ok_or(super::error::ApiError::NotFound)?;
    if tenant.status == crate::http::registry::models::TenantStatus::Purged {
        return Ok(axum::http::StatusCode::NO_CONTENT);
    }
    tx.begin_operator_deletion(&tenant.id, "operator", chrono::Utc::now())
        .await?;
    Ok(axum::http::StatusCode::ACCEPTED)
}

/// GET /api/v1/operator/recovery/status — read recovery status.
pub async fn recovery_status() -> Result<axum::response::Response, super::error::ApiError> {
    let body = serde_json::json!({ "status": "ok" });
    let body = serde_json::to_vec(&body).map_err(|error| {
        super::error::ApiError::Internal(crate::error::MemoryError::Transient(format!(
            "serialize recovery response: {error}"
        )))
    })?;
    let mut response = axum::response::Response::new(axum::body::Body::from(body));
    *response.status_mut() = axum::http::StatusCode::OK;
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/json"),
    );
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::registry::models::{NamespaceBinding, Tenant, TenantStatus};
    use axum::extract::{Extension, Path, State};
    use std::sync::Arc;

    /// A fresh in-memory `HttpState` for the operator handlers.
    async fn state() -> Arc<crate::http::HttpState> {
        crate::http::HttpState::default_for_test().await
    }

    /// An operator principal authenticated now, inside the ten-minute window.
    fn operator() -> OperatorPrincipal {
        OperatorPrincipal {
            authenticated_at: chrono::Utc::now(),
        }
    }

    /// A tenant in `status`, bound to its own namespace.
    fn tenant(id: &str, status: TenantStatus) -> Tenant {
        Tenant {
            id: id.to_string(),
            status,
            namespace_binding: NamespaceBinding {
                namespace: format!("tns_{id}"),
                database: "memory".into(),
            },
            plan_version: 1,
            schema_version: 0,
            retry_stage: None,
            provisioning_lease: None,
            created_at: chrono::Utc::now(),
            version: 0,
        }
    }

    /// Persist `tenant` into the state under test.
    async fn seed(state: &Arc<crate::http::HttpState>, tenant: &Tenant) {
        state
            .registry
            .tenants()
            .write_tenant(tenant)
            .await
            .expect("seed tenant");
    }

    #[test]
    fn recent_operator_auth_is_accepted() {
        let principal = OperatorPrincipal {
            authenticated_at: chrono::Utc::now(),
        };
        assert!(principal.require_recent_auth().is_ok());
    }

    #[test]
    fn stale_operator_auth_is_rejected() {
        let principal = OperatorPrincipal {
            authenticated_at: chrono::Utc::now() - chrono::Duration::minutes(11),
        };
        assert!(matches!(
            principal.require_recent_auth(),
            Err(ApiError::ReauthRequired)
        ));
    }

    #[test]
    fn an_operator_authenticated_in_the_future_is_rejected() {
        // A clock skew must not extend the reauth window indefinitely.
        let principal = OperatorPrincipal {
            authenticated_at: chrono::Utc::now() + chrono::Duration::minutes(1),
        };

        assert!(matches!(
            principal.require_recent_auth(),
            Err(ApiError::ReauthRequired)
        ));
    }

    #[tokio::test]
    async fn reading_an_unknown_tenant_is_a_not_found() {
        let state = state().await;

        let observed = get_tenant(
            State(state),
            Extension(operator()),
            Path("ten_missing".to_string()),
        )
        .await;

        assert!(matches!(observed, Err(ApiError::NotFound)));
    }

    #[tokio::test]
    async fn reading_a_known_tenant_succeeds() {
        let state = state().await;
        seed(&state, &tenant("ten_ok", TenantStatus::Ready)).await;

        let observed = get_tenant(
            State(state),
            Extension(operator()),
            Path("ten_ok".to_string()),
        )
        .await;

        assert!(observed.is_ok());
    }

    #[tokio::test]
    async fn reading_a_tenant_returns_ok_status() {
        let state = state().await;
        seed(&state, &tenant("ten_ok", TenantStatus::Ready)).await;

        let observed = get_tenant(
            State(state),
            Extension(operator()),
            Path("ten_ok".to_string()),
        )
        .await
        .ok()
        .expect("tenant reads");

        assert_eq!(observed.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn reading_a_tenant_with_stale_operator_auth_is_refused() {
        let state = state().await;
        seed(&state, &tenant("ten_ok", TenantStatus::Ready)).await;
        let stale = OperatorPrincipal {
            authenticated_at: chrono::Utc::now() - chrono::Duration::hours(1),
        };

        let observed = get_tenant(State(state), Extension(stale), Path("ten_ok".to_string())).await;

        assert!(matches!(observed, Err(ApiError::ReauthRequired)));
    }

    #[tokio::test]
    async fn retrying_a_healthy_tenant_is_a_conflict() {
        let state = state().await;
        seed(&state, &tenant("ten_ok", TenantStatus::Ready)).await;

        let observed = retry_tenant(
            State(state),
            Extension(operator()),
            Path("ten_ok".to_string()),
        )
        .await;

        assert!(
            matches!(observed, Err(ApiError::Conflict)),
            "only a failed tenant can be retried"
        );
    }

    #[tokio::test]
    async fn retrying_a_failed_tenant_is_accepted() {
        let state = state().await;
        seed(&state, &tenant("ten_fail", TenantStatus::Failed)).await;

        let observed = retry_tenant(
            State(state),
            Extension(operator()),
            Path("ten_fail".to_string()),
        )
        .await;

        assert_eq!(observed.ok(), Some(axum::http::StatusCode::ACCEPTED));
    }

    #[tokio::test]
    async fn retrying_an_unknown_tenant_is_a_not_found() {
        let state = state().await;

        let observed = retry_tenant(
            State(state),
            Extension(operator()),
            Path("ten_missing".to_string()),
        )
        .await;

        assert!(matches!(observed, Err(ApiError::NotFound)));
    }

    #[tokio::test]
    async fn suspending_a_ready_tenant_succeeds() {
        let state = state().await;
        seed(&state, &tenant("ten_s", TenantStatus::Ready)).await;

        let observed = suspend_tenant(
            State(state),
            Extension(operator()),
            Path("ten_s".to_string()),
        )
        .await;

        assert_eq!(observed.ok(), Some(axum::http::StatusCode::NO_CONTENT));
    }

    #[tokio::test]
    async fn suspending_an_already_suspended_tenant_is_a_no_op() {
        let state = state().await;
        seed(&state, &tenant("ten_s", TenantStatus::Suspended)).await;

        let observed = suspend_tenant(
            State(state),
            Extension(operator()),
            Path("ten_s".to_string()),
        )
        .await;

        assert_eq!(observed.ok(), Some(axum::http::StatusCode::NO_CONTENT));
    }

    #[tokio::test]
    async fn suspending_a_purged_tenant_is_a_conflict() {
        let state = state().await;
        seed(&state, &tenant("ten_p", TenantStatus::Purged)).await;

        let observed = suspend_tenant(
            State(state),
            Extension(operator()),
            Path("ten_p".to_string()),
        )
        .await;

        assert!(matches!(observed, Err(ApiError::Conflict)));
    }

    #[tokio::test]
    async fn resuming_a_suspended_tenant_succeeds() {
        let state = state().await;
        seed(&state, &tenant("ten_r", TenantStatus::Suspended)).await;

        let observed = resume_tenant(
            State(state),
            Extension(operator()),
            Path("ten_r".to_string()),
        )
        .await;

        assert_eq!(observed.ok(), Some(axum::http::StatusCode::NO_CONTENT));
    }

    #[tokio::test]
    async fn resuming_an_unknown_tenant_is_a_not_found() {
        let state = state().await;

        let observed = resume_tenant(
            State(state),
            Extension(operator()),
            Path("ten_missing".to_string()),
        )
        .await;

        assert!(matches!(observed, Err(ApiError::NotFound)));
    }

    #[tokio::test]
    async fn purging_an_unknown_tenant_is_a_not_found() {
        let state = state().await;

        let observed = purge_tenant(
            State(state),
            Extension(operator()),
            Path("ten_missing".to_string()),
        )
        .await;

        assert!(matches!(observed, Err(ApiError::NotFound)));
    }

    #[tokio::test]
    async fn purging_an_already_purged_tenant_is_a_no_op() {
        let state = state().await;
        seed(&state, &tenant("ten_p", TenantStatus::Purged)).await;

        let observed = purge_tenant(
            State(state),
            Extension(operator()),
            Path("ten_p".to_string()),
        )
        .await;

        assert_eq!(observed.ok(), Some(axum::http::StatusCode::NO_CONTENT));
    }

    #[tokio::test]
    async fn the_recovery_status_endpoint_reports_ok() {
        let observed = recovery_status().await.ok().expect("recovery status");

        assert_eq!(observed.status(), axum::http::StatusCode::OK);
    }
}
