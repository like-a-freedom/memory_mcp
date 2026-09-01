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

use axum::http::StatusCode;
use axum::middleware::Next;
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
    axum::extract::Path(tenant_id): axum::extract::Path<String>,
) -> Result<axum::response::Response, super::error::ApiError> {
    let tenant = state
        .registry
        .store_clone()
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
    _state: axum::extract::State<Arc<crate::http::HttpState>>,
    _tenant_id: axum::extract::Path<String>,
) -> Result<axum::http::StatusCode, super::error::ApiError> {
    Err(super::error::ApiError::Unavailable)
}

/// POST /api/v1/operator/tenants/:id/suspend — suspend a tenant.
pub async fn suspend_tenant(
    _state: axum::extract::State<Arc<crate::http::HttpState>>,
    _tenant_id: axum::extract::Path<String>,
) -> Result<axum::http::StatusCode, super::error::ApiError> {
    Err(super::error::ApiError::Unavailable)
}

/// POST /api/v1/operator/tenants/:id/resume — resume a suspended tenant.
pub async fn resume_tenant(
    _state: axum::extract::State<Arc<crate::http::HttpState>>,
    _tenant_id: axum::extract::Path<String>,
) -> Result<axum::http::StatusCode, super::error::ApiError> {
    Err(super::error::ApiError::Unavailable)
}

/// POST /api/v1/operator/tenants/:id/purge — initiate Account deletion.
pub async fn purge_tenant(
    _state: axum::extract::State<Arc<crate::http::HttpState>>,
    _tenant_id: axum::extract::Path<String>,
) -> Result<axum::http::StatusCode, super::error::ApiError> {
    Err(super::error::ApiError::Unavailable)
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
}
