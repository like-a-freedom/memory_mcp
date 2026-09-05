//! Tenant runtime acquisition.
//!
//! Runs after `authenticate` so the principal is available. Resolves
//! the Tenant via `account_resolver`, acquires a global admission
//! permit (or a separate subscription permit for
//! `subscriptions/listen`), and acquires a pinned runtime from the
//! pool. On any error returns a clean 4xx/5xx so the caller never
//! sees a half-acquired state.

use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::sync::Arc;

use crate::http::HttpState;
use crate::http::principal::AuthenticatedPrincipal;
use crate::http::registry::account::ResolvedTenant;
use crate::http::registry::plan::{Plan, QuotaDecision};
use crate::http::runtime::guard::{AdmissionPermitRef, OperationGuardRef};

use super::preflight::{ValidatedMcpRequest, quota_denied_response};

/// Tenant runtime acquisition. Runs after `authenticate` so the
/// principal is available. See the module docs for the full state
/// machine.
pub async fn acquire_runtime(
    axum::extract::State(state): axum::extract::State<Arc<HttpState>>,
    mut req: axum::extract::Request,
    next: Next,
) -> Response {
    let principal = match req.extensions().get::<AuthenticatedPrincipal>().cloned() {
        Some(p) => p,
        None => {
            return (
                axum::http::StatusCode::UNAUTHORIZED,
                "missing authenticated principal",
            )
                .into_response();
        }
    };
    let tenant = match state
        .account_resolver
        .resolve_ready_tenant(principal.account_id())
        .await
    {
        Ok(ResolvedTenant::Ready(t)) => t,
        Ok(ResolvedTenant::Provisioning(_, _)) => {
            return (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "tenant provisioning",
            )
                .into_response();
        }
        Ok(ResolvedTenant::Suspended) => {
            return (axum::http::StatusCode::FORBIDDEN, "tenant suspended").into_response();
        }
        Ok(ResolvedTenant::Failed(_)) => {
            return (axum::http::StatusCode::SERVICE_UNAVAILABLE, "tenant failed").into_response();
        }
        Ok(ResolvedTenant::NotFound) | Err(_) => {
            return (axum::http::StatusCode::NOT_FOUND, "tenant not found").into_response();
        }
    };
    let validated = req.extensions().get::<ValidatedMcpRequest>().cloned();
    let is_subscription = validated
        .as_ref()
        .is_some_and(|request| request.subscription);
    let source_bytes = validated.and_then(|request| request.ingest_source_bytes);
    let store = state.registry.store_clone();
    let registry_plan = match store.load_plan(tenant.plan_version).await {
        Ok(plan) => plan,
        Err(error) => {
            eprintln!("memory_mcp::http: quota plan load failed: {error}");
            return (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "quota registry unavailable",
            )
                .into_response();
        }
    };
    let plan = Plan::from(&registry_plan);
    if let Some(source_bytes) = source_bytes {
        let decision = match store
            .reserve_ingest_usage(&tenant.id, source_bytes, &plan, chrono::Utc::now())
            .await
        {
            Ok(decision) => decision,
            Err(error) => {
                eprintln!("memory_mcp::http: ingest quota reserve failed: {error}");
                return (
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    "quota registry unavailable",
                )
                    .into_response();
            }
        };
        if let QuotaDecision::Deny {
            reason,
            retry_after_secs,
            guidance,
        } = decision
        {
            return quota_denied_response(reason, retry_after_secs, guidance);
        }
    }
    let permit = match state.admission.try_acquire_for(is_subscription) {
        Ok(p) => p,
        Err(()) => {
            return (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "admission capacity exhausted",
            )
                .into_response();
        }
    };
    let guard = match state
        .pool
        .acquire_or_wait_with_limit(&tenant, plan.per_tenant_request_concurrency)
        .await
    {
        Ok(g) => g,
        Err(_) => {
            return (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "runtime pool capacity exhausted",
            )
                .into_response();
        }
    };
    // The handler extracts these by `remove::<T>`. Wrap in
    // `Arc` to satisfy axum's `Extension<T: Clone>` bound.
    let permit_ref = AdmissionPermitRef(std::sync::Arc::new(permit));
    let guard_ref = OperationGuardRef(std::sync::Arc::new(guard));
    req.extensions_mut().insert(permit_ref);
    req.extensions_mut().insert(guard_ref);
    let mut resp = next.run(req).await;
    // Log context: hex of the first 8 bytes of the SHA-256 of
    // the tenant id. Cheap and stable across processes.
    use sha2::Digest;
    let digest = sha2::Sha256::digest(tenant.id.as_bytes());
    let fingerprint = hex::encode(&digest[..8]);
    resp.extensions_mut()
        .insert(crate::http::logging::TenantLogContext {
            credential_kind: principal.credential_kind().to_string(),
            tenant_fingerprint: fingerprint,
            ..Default::default()
        });
    resp
}
