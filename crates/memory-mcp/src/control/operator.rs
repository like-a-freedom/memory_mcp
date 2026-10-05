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
#[cfg(feature = "streamable-http")]
use crate::http::tasks::state::TaskStore;

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

/// The durable fingerprint of a tenant's reembed task.
///
/// One fingerprint per tenant, because the pass is a destructive rewrite of
/// every vector in a namespace and must not run twice concurrently. The
/// `UNIQUE (tenant_id, fingerprint)` index behind [`enqueue`]'s dedupe turns a
/// second operator request into "the same task", so a retried or double-posted
/// request is a no-op rather than a second concurrent pass.
pub fn reembed_tenant_fingerprint(tenant_id: &str) -> String {
    format!("reembed:{tenant_id}")
}

/// POST /api/v1/operator/tenants/:id/reembed — enqueue a whole-namespace
/// re-embedding pass.
///
/// Class B maintenance (spec §2 Rule 1): the provider is deployment-level, so a
/// tenant whose vectors were written by a different provider stays on lexical
/// retrieval until a human asks for this. Nothing else may trigger it — not a
/// changed env var, not a startup probe — because the previous vectors are
/// destroyed with no way back.
///
/// 202 ACCEPTED when a new task was created, 204 NO_CONTENT when the
/// fingerprint already names a live task, matching `suspend_tenant`'s
/// already-terminal behaviour: an operator who retries after a timeout must
/// not double-spend the pass.
pub async fn reembed_tenant(
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
    let task_store = reembed_task_store(&state, &tenant).await?;
    let fingerprint = reembed_tenant_fingerprint(&tenant.id);
    // The dedupe probe runs BEFORE the enqueue. `enqueue` returns the existing
    // id for a live fingerprint and a fresh id for a new one, but both are
    // indistinguishable after the fact; probing first makes "was one already
    // queued?" answerable, so a retried request answers 204 and does not
    // re-spend the pass. A request that loses the race between the probe and the
    // enqueue lands on the store's `UNIQUE (tenant_id, fingerprint)` index and
    // returns the winner's id, which is the same no-op.
    let already_queued = task_store.find_task_by_fingerprint(&fingerprint).await?;
    let task_id = task_store
        .enqueue(
            crate::http::tasks::state::TASK_KIND_REEMBED,
            &fingerprint,
            // `max_failures`/`retry_failed` are the reembed pass options
            // (`ReembedOptions`), written at their wire defaults rather than
            // guessed from the struct: this route creates no body, so the
            // defaults are the only defensible values.
            serde_json::json!({ "max_failures": null, "retry_failed": false }),
        )
        .await
        .map_err(super::error::ApiError::from)?;
    if already_queued.is_some() {
        Ok(axum::http::StatusCode::NO_CONTENT)
    } else {
        let record = task_store
            .load(&task_id)
            .await
            .map_err(super::error::ApiError::from)?
            .ok_or(super::error::ApiError::NotFound)?;
        debug_assert_eq!(record.kind, crate::http::tasks::state::TASK_KIND_REEMBED);
        Ok(axum::http::StatusCode::ACCEPTED)
    }
}

/// Build the `DurableTaskStore` a reembed request enqueues into: the same
/// tenant-namespace binding the scheduler tick later claims from, so the row the
/// route writes is the row the worker reads. Split out because both the handler
/// and its tests need it and neither should re-derive the binding.
async fn reembed_task_store(
    state: &Arc<crate::http::HttpState>,
    tenant: &crate::http::registry::models::Tenant,
) -> Result<crate::http::tasks::worker::DurableTaskStore, super::error::ApiError> {
    let engine = state.registry.tenant_engine_optional().ok_or_else(|| {
        super::error::ApiError::from(crate::error::MemoryError::Unavailable(
            "tenant storage engine unavailable".into(),
        ))
    })?;
    let db = engine
        .bind(tenant)
        .await
        .map_err(super::error::ApiError::from)?;
    let bound_db = Arc::new(crate::storage::client::BoundDbClient::new(
        db,
        tenant.namespace_binding.namespace.clone(),
    ));
    Ok(crate::http::tasks::worker::DurableTaskStore::new(
        bound_db,
        tenant.id.clone(),
    ))
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

    /// Apply the durable task table to a tenant namespace, so a route test can
    /// enqueue and read back a real `tenant_task` row. The table lives in the
    /// HTTP tenant migrations rather than the storage migrations.
    async fn apply_task_schema(state: &Arc<crate::http::HttpState>, tenant: &Tenant) {
        let engine = state
            .registry
            .tenant_engine_optional()
            .expect("tenant engine wired for tests");
        let db = engine.bind(tenant).await.expect("bind tenant namespace");
        for ddl in [
            include_str!("../../migrations/041_tenant_tasks.surql"),
            include_str!("../../migrations/045_tenant_task_kind.surql"),
        ] {
            crate::storage::client::DbClient::query(
                &*db,
                ddl,
                None,
                &tenant.namespace_binding.namespace,
            )
            .await
            .expect("apply tenant_task migration");
        }
    }

    /// Read back the tenant's reembed task row, so a test can assert what the
    /// route wrote rather than only what it answered.
    async fn reembed_record(
        state: &Arc<crate::http::HttpState>,
        tenant: &Tenant,
    ) -> crate::http::tasks::state::TenantTaskRecord {
        use crate::http::tasks::state::TaskStore;
        let engine = state
            .registry
            .tenant_engine_optional()
            .expect("tenant engine wired for tests");
        let db = engine.bind(tenant).await.expect("bind tenant namespace");
        let store = crate::http::tasks::worker::DurableTaskStore::new(
            Arc::new(crate::storage::client::BoundDbClient::new(
                db,
                tenant.namespace_binding.namespace.clone(),
            )),
            tenant.id.clone(),
        );
        let task_id = store
            .find_task_by_fingerprint(&reembed_tenant_fingerprint(&tenant.id))
            .await
            .expect("probe fingerprint")
            .expect("a reembed task exists");
        store
            .load(&task_id)
            .await
            .expect("load reembed task")
            .expect("the row is present")
    }

    /// The fingerprint is per tenant, so a second tenant's reembed is not the
    /// first one's. It is what makes a retried request a no-op, so its shape is
    /// pinned: `reembed:<tenant_id>`.
    #[test]
    fn the_reembed_fingerprint_is_namespaced_by_tenant() {
        assert_eq!(reembed_tenant_fingerprint("ten_a"), "reembed:ten_a");
        assert_ne!(
            reembed_tenant_fingerprint("ten_a"),
            reembed_tenant_fingerprint("ten_b")
        );
    }

    #[tokio::test]
    async fn reembedding_an_unknown_tenant_is_a_not_found() {
        let state = state().await;

        let observed = reembed_tenant(
            State(state),
            Extension(operator()),
            Path("ten_missing".to_string()),
        )
        .await;

        assert!(matches!(observed, Err(ApiError::NotFound)));
    }

    /// A first request creates the task and says so with 202. The row it wrote is
    /// asserted, not just the status: a 202 that enqueued nothing would satisfy
    /// the status assertion alone.
    #[tokio::test]
    async fn a_first_reembed_request_is_accepted_and_queues_a_reembed_task() {
        let state = state().await;
        let tenant = tenant("ten_reembed", TenantStatus::Ready);
        apply_task_schema(&state, &tenant).await;
        seed(&state, &tenant).await;

        let observed = reembed_tenant(
            State(state.clone()),
            Extension(operator()),
            Path("ten_reembed".to_string()),
        )
        .await;

        assert_eq!(observed.ok(), Some(axum::http::StatusCode::ACCEPTED));
        let record = reembed_record(&state, &tenant).await;
        assert_eq!(
            record.kind,
            crate::http::tasks::state::TASK_KIND_REEMBED,
            "the route must enqueue a reembed task, not an extraction"
        );
        assert_eq!(
            record.fingerprint,
            reembed_tenant_fingerprint("ten_reembed"),
            "the fingerprint is what makes a second request a no-op"
        );
        assert_eq!(record.state, crate::http::tasks::state::TaskState::Queued);
    }

    /// Two requests for one tenant must not queue two passes. The second is a
    /// retried request, and a reembed is an irreversible rewrite of every vector
    /// in the namespace, so re-spending it is the failure.
    #[tokio::test]
    async fn a_second_reembed_request_for_the_same_tenant_is_a_no_op() {
        let state = state().await;
        let tenant = tenant("ten_reembed2", TenantStatus::Ready);
        apply_task_schema(&state, &tenant).await;
        seed(&state, &tenant).await;

        let first = reembed_tenant(
            State(state.clone()),
            Extension(operator()),
            Path("ten_reembed2".to_string()),
        )
        .await;
        assert_eq!(first.ok(), Some(axum::http::StatusCode::ACCEPTED));
        let task_id = reembed_record(&state, &tenant).await.id;

        let second = reembed_tenant(
            State(state.clone()),
            Extension(operator()),
            Path("ten_reembed2".to_string()),
        )
        .await;

        assert_eq!(
            second.ok(),
            Some(axum::http::StatusCode::NO_CONTENT),
            "a retried request must report no new work, not 202 again"
        );
        assert_eq!(
            reembed_record(&state, &tenant).await.id,
            task_id,
            "the retry must join the existing task, not create a second pass"
        );
    }

    #[tokio::test]
    async fn reembedding_with_stale_operator_auth_is_refused() {
        let state = state().await;
        let tenant = tenant("ten_reembed3", TenantStatus::Ready);
        apply_task_schema(&state, &tenant).await;
        seed(&state, &tenant).await;
        let stale = OperatorPrincipal {
            authenticated_at: chrono::Utc::now() - chrono::Duration::hours(1),
        };

        let observed = reembed_tenant(
            State(state.clone()),
            Extension(stale),
            Path("ten_reembed3".to_string()),
        )
        .await;

        assert!(matches!(observed, Err(ApiError::ReauthRequired)));
        assert!(
            state
                .registry
                .tenant_engine_optional()
                .expect("engine")
                .bind(&tenant)
                .await
                .is_ok()
        );
    }
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
