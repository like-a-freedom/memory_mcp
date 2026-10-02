//! Operator principal seam.
//!
//! Stub: with the `test-fixtures` feature, the stub
//! middleware accepts `X-Operator-Auth: stub`; without
//! `test-fixtures` there is no operator injection
//! (operator endpoints are unreachable until OIDC).
//!
//! OIDC replaces this with derived operator
//! identity; the accessor name (`require_recent_auth`) stays.

use crate::provisioning::api::transition_tenant;
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
    // A retry re-enters the stage the tenant failed in, which is why that
    // stage is recorded when the tenant transitions into `Failed`. There is
    // no safe substitute for a missing one: `Reserved` is not an edge out of
    // `Failed` in the table, so guessing one produced a transition that was
    // refused and reported as 409 forever. Refusing here says what is true —
    // this tenant cannot be retried, because nothing recorded where it
    // stopped — which is a condition an operator can act on, where the
    // invented stage looked like an ordinary conflict.
    let Some(stage) = tenant.retry_stage else {
        return Err(super::error::ApiError::Conflict);
    };
    // Through the table like the other two. `Failed -> NamespaceCreating` and
    // `Failed -> Migrating` are both legal, and a `retry_stage` outside them
    // is now refused rather than written — before, any stage the record
    // happened to carry was accepted.
    transition_tenant(
        &crate::provisioning::api::TenantLifecycle::new(store.as_ref()),
        &tenant.id,
        tenant.version,
        crate::http::registry::models::TenantStatus::Failed,
        stage,
    )
    .await
    .map_err(|_| super::error::ApiError::Conflict)?;
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
    // From here the table decides. The two checks above were already
    // consequences of it — `Suspended -> Suspended` and the two terminal
    // states are not in the table — and are kept because they answer with a
    // shape the API has always used: 204 for an idempotent re-suspend, 409
    // for a terminal tenant. The table alone would give 409 for both.
    transition_tenant(
        &crate::provisioning::api::TenantLifecycle::new(store.as_ref()),
        &tenant.id,
        tenant.version,
        tenant.status,
        crate::http::registry::models::TenantStatus::Suspended,
    )
    .await
    .map_err(|_| super::error::ApiError::Conflict)?;
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
    // Only a suspended tenant is resumable. This precondition is the
    // handler's own, and it is load-bearing for a pair the table cannot
    // refuse: a `Migrating` tenant may legally become `Ready`, because that is
    // how a provisioning worker finishes — but it was never suspended, so an
    // operator has nothing to resume and must not get that far. Asking the
    // table alone would let `Migrating -> Ready` through, and the tenant would
    // then report ready while a worker still holds its lease.
    if tenant.status != crate::http::registry::models::TenantStatus::Suspended {
        return Err(super::error::ApiError::Conflict);
    }
    // The status the tenant is actually in, as `from`. This handler used to
    // write `Suspended -> Ready` without reading it, and because the store's
    // CAS is on `expected_version` and not on `from`, resuming a tenant that
    // was never suspended issued `Migrating -> Ready` — after which the
    // operator and the provisioning loop both believed the tenant was ready,
    // while a worker still held a lease on it. The table refuses that pair.
    transition_tenant(
        &crate::provisioning::api::TenantLifecycle::new(store.as_ref()),
        &tenant.id,
        tenant.version,
        tenant.status,
        crate::http::registry::models::TenantStatus::Ready,
    )
    .await
    .map_err(|_| super::error::ApiError::Conflict)?;
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

    /// A retryable tenant is one that remembers the stage it failed in.
    ///
    /// The stage is what the retry re-enters, and it is recorded by the transition
    /// into `Failed` rather than chosen by the caller — a tenant that failed in
    /// `Migrating` re-enters `Migrating`, because re-entering `NamespaceCreating`
    /// would redo work that already succeeded.
    #[tokio::test]
    async fn retrying_a_failed_tenant_is_accepted() {
        let state = state().await;
        let mut failed = tenant("ten_fail", TenantStatus::Failed);
        failed.retry_stage = Some(TenantStatus::Migrating);
        seed(&state, &failed).await;

        let observed = retry_tenant(
            State(state.clone()),
            Extension(operator()),
            Path("ten_fail".to_string()),
        )
        .await;

        assert_eq!(observed.ok(), Some(axum::http::StatusCode::ACCEPTED));

        // The retry re-enters the recorded stage rather than restarting the tenant.
        let reloaded = state
            .registry
            .tenants()
            .find_tenant_by_id("ten_fail")
            .await
            .expect("read tenant")
            .expect("tenant exists");
        assert_eq!(reloaded.status, TenantStatus::Migrating);
    }

    /// A `Failed` tenant with no recorded stage cannot be retried, and says so.
    ///
    /// This is the state a record was in before the failure transition started
    /// recording the stage: `Failed` with nothing to re-enter. Substituting a
    /// stage would have produced a `Failed -> Reserved` pair the table refuses,
    /// which surfaced as a conflict indistinguishable from "this tenant is not
    /// failed" — so the refusal is stated rather than faked.
    #[tokio::test]
    async fn retrying_a_failed_tenant_with_no_recorded_stage_is_a_conflict() {
        let state = state().await;
        seed(&state, &tenant("ten_unstaged", TenantStatus::Failed)).await;

        let observed = retry_tenant(
            State(state),
            Extension(operator()),
            Path("ten_unstaged".to_string()),
        )
        .await;

        assert!(
            matches!(observed, Err(crate::control::error::ApiError::Conflict)),
            "a failed tenant that never recorded where it stopped cannot be retried"
        );
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

    /// Read back a tenant's status, so a test can assert a refused transition
    /// left it alone.
    async fn status_of(state: &Arc<crate::http::HttpState>, id: &str) -> TenantStatus {
        state
            .registry
            .tenants()
            .find_tenant_by_id(id)
            .await
            .expect("read tenant")
            .expect("tenant exists")
            .status
    }

    /// Operator transitions obey the Tenant transition table.
    ///
    /// These three handlers reached `store.update_tenant_state` directly, so
    /// `can_transition` was consulted by no operator path at all. Two of the
    /// three had a real hole:
    ///
    ///  * `suspend_tenant` rejected only `Deleting` and `Purged`, so it
    ///    accepted `Migrating -> Suspended`. A tenant mid-provisioning holds a
    ///    lease, and suspending it strands the worker that owns it.
    ///  * `resume_tenant` never read `tenant.status` at all. It loaded the
    ///    tenant and wrote `Suspended -> Ready` regardless. Because the store's
    ///    CAS is on `expected_version` and not on `from`, resuming a
    ///    `Migrating` tenant issued `Migrating -> Ready`, and both the
    ///    operator and the provisioning loop then believed the tenant was ready.
    ///
    /// The HTTP contract is unchanged for every existing case: a refused status
    /// was already a 409. What changes is the third case, which was 204 and
    /// becomes 409 — that is the fix, not a regression.
    #[tokio::test]
    async fn operator_transitions_obey_the_transition_table() {
        use crate::provisioning::api::can_transition;

        // Case 1: Ready -> Suspended and back, both legal.
        let ctx = state().await;
        seed(&ctx, &tenant("ten_ok", TenantStatus::Ready)).await;
        assert_eq!(
            suspend_tenant(
                State(ctx.clone()),
                Extension(operator()),
                Path("ten_ok".to_string()),
            )
            .await
            .ok(),
            Some(axum::http::StatusCode::NO_CONTENT)
        );
        assert_eq!(status_of(&ctx, "ten_ok").await, TenantStatus::Suspended);
        assert_eq!(
            resume_tenant(
                State(ctx.clone()),
                Extension(operator()),
                Path("ten_ok".to_string()),
            )
            .await
            .ok(),
            Some(axum::http::StatusCode::NO_CONTENT)
        );
        assert_eq!(status_of(&ctx, "ten_ok").await, TenantStatus::Ready);

        // Case 2: a tenant mid-provisioning cannot be suspended. The premise
        // is the table's, and it is asserted so a table change cannot quietly
        // make this case vacuous.
        assert!(
            !can_transition(TenantStatus::Migrating, TenantStatus::Suspended),
            "the table must forbid this pair, or case 2 is testing nothing"
        );
        let ctx = state().await;
        seed(&ctx, &tenant("ten_mig", TenantStatus::Migrating)).await;
        assert!(
            suspend_tenant(
                State(ctx.clone()),
                Extension(operator()),
                Path("ten_mig".to_string()),
            )
            .await
            .is_err(),
            "Migrating -> Suspended is not in the table and must be refused"
        );
        assert_eq!(
            status_of(&ctx, "ten_mig").await,
            TenantStatus::Migrating,
            "a refused suspend must not move the tenant"
        );

        // Case 3: the worst one, and a different hole than the plan said.
        // `Migrating -> Ready` *is* in the table — it is how a provisioning
        // worker finishes — so the pair is legal in general and the table alone
        // cannot refuse it here. What is wrong is that an operator reaches it
        // at all: `resume_tenant` is for a tenant that was suspended, and a
        // `Migrating` tenant was never suspended. Before this change the handler
        // wrote `Suspended -> Ready` without reading the status, so it issued
        // `Migrating -> Ready` and both the operator and the provisioning loop
        // then believed the tenant was ready while a worker still held a lease.
        let ctx = state().await;
        seed(&ctx, &tenant("ten_mig2", TenantStatus::Migrating)).await;
        assert!(
            resume_tenant(
                State(ctx.clone()),
                Extension(operator()),
                Path("ten_mig2".to_string()),
            )
            .await
            .is_err(),
            "resuming a tenant that was never suspended must be refused, even \
             though Migrating -> Ready is a legal edge for the worker"
        );
        assert_eq!(
            status_of(&ctx, "ten_mig2").await,
            TenantStatus::Migrating,
            "a refused resume must not report the tenant ready"
        );

        // Case 4: retry still works, so the fix is not over-broad.
        let ctx = state().await;
        let mut failed = tenant("ten_fail", TenantStatus::Failed);
        failed.retry_stage = Some(TenantStatus::NamespaceCreating);
        seed(&ctx, &failed).await;
        assert_eq!(
            retry_tenant(
                State(ctx.clone()),
                Extension(operator()),
                Path("ten_fail".to_string()),
            )
            .await
            .ok(),
            Some(axum::http::StatusCode::ACCEPTED)
        );
        assert_eq!(
            status_of(&ctx, "ten_fail").await,
            TenantStatus::NamespaceCreating,
            "Failed -> retry_stage is in the table and must still apply"
        );
    }

    #[tokio::test]
    async fn the_recovery_status_endpoint_reports_ok() {
        let observed = recovery_status().await.ok().expect("recovery status");

        assert_eq!(observed.status(), axum::http::StatusCode::OK);
    }
}
