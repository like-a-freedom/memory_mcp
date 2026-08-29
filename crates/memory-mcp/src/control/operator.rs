//! Operator principal seam (ADR-0052, plan §4.7).
//!
//! Phase 4 stub: with the `test-fixtures` feature, the stub
//! middleware accepts `X-Operator-Auth: stub`; without
//! `test-fixtures` there is NO operator injection in Phase 4
//! (operator endpoints are unreachable until Phase 10).
//!
//! Phase 10 (Task 10.6) replaces this with OIDC-derived operator
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
    /// Phase 4 stub: always recent. Task 10.4 enforces the
    /// 10-minute bound.
    pub fn require_recent_auth(&self) -> Result<(), ApiError> {
        Ok(())
    }
}

/// Test-fixtures-only stub middleware. Injects the operator
/// principal for the Phase 4–9 operator endpoints. Removed by
/// Task 10.6 (OIDC operators).
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
