//! Top-level axum router builder.

use std::sync::Arc;

use axum::Router;
use axum::routing::{get, post};
// `delete` is only used by the control-plane routes below.
#[cfg(feature = "control-plane")]
use axum::routing::delete;

use super::HttpState;
use super::fault_injection::FaultInjector;

pub fn build_router(
    state: Arc<HttpState>,
    #[allow(unused_variables)] control_plane_injector: Option<Arc<dyn FaultInjector>>,
) -> Router {
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

    // Mode disclosure is mounted in *both* browser-auth modes (spec §8:
    // public `{mode:"local"|"oidc"}` **while the control plane is enabled**), so
    // an OIDC deployment can tell the UI which flow to run. It lives on the base
    // router, which already carries the host/origin and deadline layers.
    //
    // An off deployment mounts no browser-auth route at all (plan §6 feature
    // matrix: "control off | ... no browser keys/plan/discovery/routes"), so
    // this is a runtime condition and not merely the `control-plane` feature:
    // otherwise off mode would both expose a browser-auth route and answer
    // `{"mode":"oidc"}` for a surface it does not serve.
    #[cfg(feature = "control-plane")]
    let router = if state.config.enable_control_plane {
        router.route(
            "/api/v1/auth/config",
            get(crate::control::local_admin::handlers::auth_config),
        )
    } else {
        router
    };

    #[cfg(feature = "control-plane")]
    let control_extension: Option<axum::Extension<Arc<dyn FaultInjector>>> =
        control_plane_injector.map(axum::Extension);
    // The OIDC/account/operator surface is mounted only when the
    // deployment is in OIDC mode. Local mode must not expose
    // `/auth/oidc/*`, `/api/v1/account/*` or `/api/v1/operator/*` at
    // all — not merely leave them unauthenticated — and a disabled
    // control plane mounts neither surface.
    #[cfg(feature = "control-plane")]
    let router = if state.config.browser_auth_is_oidc() {
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
        let account = if let Some(ext) = control_extension {
            account.layer(ext)
        } else {
            account
        };
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

    #[cfg(feature = "control-plane")]
    let router = {
        use crate::http::config::BrowserAuthConfig;
        let is_local = matches!(state.config.browser_auth, Some(BrowserAuthConfig::Local(_)));
        if is_local {
            use crate::control::local_admin::handlers;
            use axum::routing::{delete, get, post};
            // The local surface deliberately mirrors spec §8 and mounts
            // none of the OIDC control-plane routes. The OIDC branch is
            // skipped entirely above, so `/auth/oidc/*`,
            // `/api/v1/account/*` and `/api/v1/operator/*` are absent
            // rather than merely unauthenticated.
            let local_admin = Router::new()
                .route("/api/v1/auth/local/csrf", get(handlers::preauth_csrf))
                .route(
                    "/api/v1/auth/local/challenge",
                    post(handlers::inspect_challenge),
                )
                .route("/api/v1/auth/local/activate", post(handlers::activate))
                .route("/api/v1/auth/local/reset", post(handlers::reset))
                .route("/api/v1/auth/local/login", post(handlers::login))
                .route("/api/v1/admin/session", get(handlers::session))
                .route("/api/v1/admin/reauth", post(handlers::reauth))
                .route("/api/v1/admin/logout", post(handlers::logout))
                .route(
                    "/api/v1/admin/clients",
                    get(handlers::list_clients).post(handlers::create_client),
                )
                .route(
                    "/api/v1/admin/clients/{account_id}",
                    get(handlers::get_client),
                )
                .route(
                    "/api/v1/admin/clients/{account_id}/keys",
                    get(handlers::list_keys).post(handlers::issue_key),
                )
                .route(
                    "/api/v1/admin/clients/{account_id}/keys/{key_id}",
                    delete(handlers::revoke_key),
                )
                .route(
                    "/api/v1/admin/clients/{account_id}/suspend",
                    post(handlers::suspend_client),
                )
                .route(
                    "/api/v1/admin/clients/{account_id}/resume",
                    post(handlers::resume_client),
                )
                // Local routes are merged after the base router's
                // layers were applied, so they need their own copy of the
                // host and deadline protections. The host layer also
                // enforces the configured Host allowlist; Origin is
                // enforced per-handler because only local routes require
                // its *presence*.
                .layer(axum::middleware::from_fn_with_state(
                    state.clone(),
                    super::middleware::local_admin_deadline,
                ))
                .layer(axum::middleware::from_fn_with_state(
                    state.clone(),
                    super::middleware::host_origin,
                ));
            // The reserved-surface 404 is installed as the single outer
            // fallback below so the static-asset router cannot shadow it.
            router.merge(local_admin)
        } else {
            router
        }
    };

    // ─── Reserved-surface 404 and the SPA fallback ────────
    //
    // `/api/` and `/auth/` are reserved for the API and the
    // authentication surface. An unmatched path under either prefix must
    // answer `404`, never the SPA shell: serving HTML with `200` for a
    // route the deployment deliberately does not mount is exactly the
    // mode-bypass failure spec §8 rules out, and it also disguises a
    // typo'd API call as success.
    //
    // This is installed as the single outer fallback, after any mode
    // specific fallback, so it cannot be shadowed by the static-asset
    // router.
    let ui_enabled = {
        #[cfg(feature = "control-plane-ui")]
        {
            state.config.enable_control_plane_ui
        }
        #[cfg(not(feature = "control-plane-ui"))]
        {
            false
        }
    };
    let router = router.fallback(move |uri: axum::http::Uri| async move {
        let path = uri.path();
        if path.starts_with("/api/") || path == "/api" {
            return axum::response::IntoResponse::into_response((
                axum::http::StatusCode::NOT_FOUND,
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                "{\"error\":{\"code\":\"not_found\",\"message\":\"not found\"}}",
            ));
        }
        if path.starts_with("/auth/") || path == "/auth" {
            return axum::response::IntoResponse::into_response((
                axum::http::StatusCode::NOT_FOUND,
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                "{\"error\":{\"code\":\"not_found\",\"message\":\"not found\"}}",
            ));
        }
        #[cfg(feature = "control-plane-ui")]
        if ui_enabled {
            return crate::control::static_assets::serve_asset(path);
        }
        let _ = ui_enabled;
        axum::response::IntoResponse::into_response((
            axum::http::StatusCode::NOT_FOUND,
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            "{\"error\":{\"code\":\"not_found\",\"message\":\"not found\"}}",
        ))
    });
    router.with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::test_state::HttpStateTestBuilder;
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use tower_service::Service;

    fn request(method: Method, uri: &str) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header(axum::http::header::HOST, "localhost")
            .body(Body::empty())
            .expect("request")
    }

    /// Plan §6 feature matrix: "control off | … no browser keys/plan/discovery/
    /// routes". The mode-disclosure route is the one that would otherwise be
    /// reachable in every mode, because it is mounted outside the OIDC-mode
    /// branch, so it is the regression case. An off deployment must answer
    /// `404` rather than claim a browser-auth mode it does not serve.
    #[tokio::test]
    async fn off_mode_mounts_no_browser_auth_route() {
        let state = HttpStateTestBuilder::new()
            .await
            .build()
            .await
            .expect("off-mode HTTP state");
        assert!(
            !state.config.enable_control_plane,
            "the fixture is off mode"
        );

        let mut router = build_router(state, None);
        let response = router
            .call(request(Method::GET, "/api/v1/auth/config"))
            .await
            .expect("dispatch");
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "off mode must not answer the browser-auth mode disclosure"
        );
    }

    /// The same route *is* mounted once the control plane is enabled, so the
    /// assertion above cannot pass for the wrong reason.
    #[cfg(feature = "control-plane")]
    #[tokio::test]
    async fn an_enabled_control_plane_mounts_the_mode_disclosure() {
        let (builder, _store) = HttpStateTestBuilder::local_admin().await;
        let state = builder.build().await.expect("local admin HTTP state");
        assert!(
            state.config.enable_control_plane,
            "the fixture is local mode"
        );

        let mut router = build_router(state, None);
        let response = router
            .call(request(Method::GET, "/api/v1/auth/config"))
            .await
            .expect("dispatch");
        assert_eq!(response.status(), StatusCode::OK);
    }
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
