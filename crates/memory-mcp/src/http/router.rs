//! Top-level axum router builder.

use std::sync::Arc;

use axum::Router;
use axum::routing::{delete, get, post};

use super::HttpState;

pub fn build_router(state: Arc<HttpState>) -> Router {
    // Route-scoped layers added EARLIER are INNER (run
    // later). `acquire_runtime` runs after `authenticate`
    // because the principal must be in the request
    // extensions before the resolver runs.
    let mcp_route = post(super::transport::mcp_handler)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            super::middleware::acquire_runtime,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            super::middleware::authenticate,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            super::middleware::prevalidate_mcp,
        ));
    let router = Router::new()
        .route("/health/live", get(super::health::live))
        .route("/health/ready", get(super::health::ready))
        .route("/mcp", mcp_route)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            super::middleware::request_deadline,
        ))
        .layer(axum::middleware::from_fn(
            super::middleware::reject_non_post_mcp,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            super::middleware::host_origin,
        ))
        .layer(axum::middleware::from_fn(
            super::middleware::inject_sse_headers,
        ))
        .layer(axum::middleware::from_fn(super::logging::request_log));
    #[cfg(feature = "prometheus")]
    let router = router.route("/metrics", get(super::metrics::prometheus));

    #[cfg(feature = "control-plane")]
    let router = if state.config.enable_control_plane {
        let account = Router::new()
            .route(
                "/api/v1/account",
                get(crate::control::account_api::get_account),
            )
            .route(
                "/api/v1/account/csrf",
                get(crate::control::account_api::csrf_token),
            )
            .route(
                "/api/v1/account/api_keys",
                get(crate::control::account_api::list_api_keys)
                    .post(crate::control::account_api::create_api_key),
            )
            .route(
                "/api/v1/account/api_keys/{id}",
                delete(crate::control::account_api::revoke_api_key),
            )
            .route(
                "/api/v1/account/identity_links",
                get(crate::control::account_api::list_identity_links)
                    .post(crate::control::account_api::link_identity),
            )
            .route(
                "/api/v1/account/identity_links/{id}",
                delete(crate::control::account_api::unlink_identity),
            )
            .route(
                "/api/v1/account/delete",
                post(crate::control::account_api::start_account_deletion),
            )
            .route(
                "/api/v1/account/delete/confirm",
                post(crate::control::account_api::confirm_account_deletion),
            )
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                super::middleware::require_control_plane_csrf,
            ))
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                super::middleware::authenticate_control_plane_session,
            ));
        let operator = Router::new()
            .route(
                "/api/v1/operator/tenants/{id}",
                get(crate::control::operator::get_tenant),
            )
            .route(
                "/api/v1/operator/tenants/{id}/retry",
                post(crate::control::operator::retry_tenant),
            )
            .route(
                "/api/v1/operator/tenants/{id}/suspend",
                post(crate::control::operator::suspend_tenant),
            )
            .route(
                "/api/v1/operator/tenants/{id}/resume",
                post(crate::control::operator::resume_tenant),
            )
            .route(
                "/api/v1/operator/tenants/{id}/purge",
                post(crate::control::operator::purge_tenant),
            )
            .route(
                "/api/v1/operator/recovery/status",
                get(crate::control::operator::recovery_status),
            )
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                super::middleware::require_control_plane_csrf,
            ))
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                super::middleware::authenticate_control_plane_operator,
            ))
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                super::middleware::authenticate_control_plane_session,
            ));
        let oidc = Router::new()
            .route("/auth/oidc/authorize", get(crate::control::oidc::authorize))
            .route("/auth/oidc/callback", get(crate::control::oidc::callback));
        let logout = Router::new()
            .route("/auth/oidc/logout", post(crate::control::oidc::logout))
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                super::middleware::require_control_plane_csrf,
            ))
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                super::middleware::authenticate_control_plane_session,
            ));
        router
            .merge(account)
            .merge(operator)
            .merge(oidc)
            .merge(logout)
    } else {
        router
    };

    #[cfg(feature = "control-plane-ui")]
    let router = if state.config.enable_control_plane_ui {
        router.fallback(|uri: axum::http::Uri| async move {
            crate::control::static_assets::serve_asset(uri.path())
        })
    } else {
        router
    };
    router.with_state(state)
}

#[cfg(test)]
pub mod test_helpers {
    use super::*;
    use axum::body::Body;
    use axum::http::{Method, Request};
    use tower_service::Service;

    /// Drive a single request through the router. Caller specifies
    /// method, URI, and (method, host) header set used by the
    /// host-origin middleware.
    pub async fn dispatch(
        router: Router,
        method: Method,
        uri: &str,
        headers: &[(&str, &str)],
    ) -> axum::response::Response {
        let mut svc = router;
        let mut b = Request::builder().method(method).uri(uri);
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        let req = b.body(Body::empty()).expect("request builder");
        svc.call(req).await.expect("dispatch")
    }
}
