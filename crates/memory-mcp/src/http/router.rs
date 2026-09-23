//! Top-level axum router builder.

use std::sync::Arc;

use axum::Router;
use axum::routing::{get, post};
// `delete` is only used by the control-plane routes below.
#[cfg(feature = "control-plane")]
use axum::routing::delete;

use super::HttpState;
use super::fault_injection::FaultInjector;
#[cfg(feature = "control-plane")]
use crate::http::config::BrowserAuthMethod;

pub fn build_router(
    state: Arc<HttpState>,
    #[allow(unused_variables)] control_plane_injector: Option<Arc<dyn FaultInjector>>,
) -> Result<Router, crate::error::MemoryError> {
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
    // These two are scoped to the routes mounted right here: the deadline is
    // the MCP/health one (the local surface carries its own, which has to keep
    // returning its documented `Retry-After` body), and the method check is
    // guarded on `/mcp` by path. The deployment-boundary layers are NOT here:
    // `Router::layer` only wraps what exists at the moment it is called, so
    // they are applied at the end of this function, after the mode-specific
    // routes and the fallback exist.
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
        ));
    #[cfg(feature = "prometheus")]
    let router = router.route("/metrics", get(super::metrics::prometheus));

    // Mode disclosure is mounted in *both* browser-auth modes (spec §8:
    // public `{mode:"local"|"oidc"}` **while the control plane is enabled**), so
    // an OIDC deployment can tell the UI which flow to run. It is mounted on the
    // base router here, so only the boundary layers applied at the end of this
    // function cover it, not the route-scoped ones above.
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
    // Each enabled browser authentication method mounts its own surface, and a
    // method that is not enabled mounts nothing at all — not even an
    // unauthenticated route (ADR-0057). A deployment with no identity provider
    // must not answer `/auth/oidc/*`, `/api/v1/account/*` or
    // `/api/v1/operator/*`; one with no local administrator must not answer
    // `/api/v1/auth/local/*` or `/api/v1/admin/*`. Both are mounted only by
    // explicit configuration, and a disabled control plane mounts neither.
    #[cfg(feature = "control-plane")]
    let router = if state.config.has_method(BrowserAuthMethod::Oidc) {
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
                    .post(crate::control::account_api::start_identity_link),
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
    let router = if state.config.has_method(BrowserAuthMethod::Local) {
        use crate::control::local_admin::handlers;
        use axum::routing::{delete, get, post};
        // The local surface mounts none of the OIDC control-plane routes: the
        // OIDC branch above is skipped when that method is disabled, so
        // `/auth/oidc/*`, `/api/v1/account/*` and `/api/v1/operator/*` are
        // absent rather than merely unauthenticated.
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
            // Local routes are merged after the base router's route-scoped
            // layers were applied, so they carry their own copy of the local
            // deadline. Host/Origin is not repeated here: it is a property of
            // the deployment boundary, applied once at the end of this
            // function, and that single copy wraps merged routes too. Origin
            // is enforced per-handler because only local routes require its
            // *presence*.
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                super::middleware::local_admin_deadline,
            ))
            // Outermost: mint the one request id before any rejection can be
            // rendered, so the error envelope, the response header and the
            // audit trail share it.
            .layer(axum::middleware::from_fn(
                crate::control::local_admin::attach_request_id,
            ));
        // The reserved-surface 404 is installed as the single outer
        // fallback below so the static-asset router cannot shadow it.
        router.merge(local_admin)
    } else {
        router
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
    // The SPA shell is stamped with the mount base once, here: the fallback
    // below serves these bytes for every SPA route so a prefixed deployment
    // never ships the build-time sentinel to a client. Stamping is gated on
    // `ui_enabled` so a deployment that turns the UI off never fails on a
    // bundle contract it does not serve; the `#[cfg]` mirrors the gate on
    // the `serve_asset` call inside the fallback.
    #[cfg(feature = "control-plane-ui")]
    let stamped_assets = if ui_enabled {
        crate::control::static_assets::build_stamped_assets(&state.config.base_path)?
    } else {
        None
    };
    #[cfg(not(feature = "control-plane-ui"))]
    let stamped_assets: Option<std::sync::Arc<crate::control::static_assets::StampedAssets>> = None;

    let router = router.fallback(move |uri: axum::http::Uri| {
        let stamped_assets = stamped_assets.clone();
        async move {
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
                return crate::control::static_assets::serve_asset(path, stamped_assets.as_deref());
            }
            let _ = (ui_enabled, &stamped_assets);
            axum::response::IntoResponse::into_response((
                axum::http::StatusCode::NOT_FOUND,
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                "{\"error\":{\"code\":\"not_found\",\"message\":\"not found\"}}",
            ))
        }
    });

    // The deployment boundary goes on last, and last is what makes it a
    // boundary: `Router::layer` wraps only the routes *and the fallback* that
    // exist at the moment it is called, so a check installed before this point
    // is silently escaped by everything mounted after it. The relative order of
    // these three is unchanged from the block that used to sit at the top of
    // this function, so no route that was already covered changes behaviour:
    // `request_log` stays outermost, which is why a request the allowlist refuses
    // is still logged.
    let base_path = state.config.base_path.clone();
    let app = router.with_state(state.clone());
    // Under a mount base the whole router is nested: axum registers every
    // route *and the fallback* under the prefix and strips it from the
    // request URI (`StripPrefix`) before anything below inspects
    // `uri().path()`, so `reject_non_post_mcp`, the reserved-surface
    // checks and `serve_asset` keep seeing canonical paths. Requests
    // outside the prefix match nothing. The boundary layers below wrap
    // this whole shape, gap fallback included.
    let app = if base_path.is_empty() {
        app
    } else {
        let base_for_fallback = base_path.clone();
        axum::Router::new()
            .nest(&base_path, app)
            .fallback(move |uri: axum::http::Uri| {
                let base = base_for_fallback.clone();
                async move {
                    // matchit does not match an empty catch-all tail, so
                    // `{base}/` lands here while `{base}` is the nested SPA
                    // root. Canonicalize the one URL humans and proxies
                    // produce routinely instead of 404-ing it; everything
                    // else outside the nested surface is a plain 404.
                    if uri.path() == format!("{base}/") {
                        return axum::response::IntoResponse::into_response(
                            axum::response::Redirect::permanent(&base),
                        );
                    }
                    axum::response::IntoResponse::into_response((
                        axum::http::StatusCode::NOT_FOUND,
                        [(axum::http::header::CONTENT_TYPE, "application/json")],
                        "{\"error\":{\"code\":\"not_found\",\"message\":\"not found\"}}",
                    ))
                }
            })
    };
    Ok(app
        .layer(axum::middleware::from_fn_with_state(
            state,
            super::middleware::host_origin,
        ))
        .layer(axum::middleware::from_fn(
            super::middleware::inject_sse_headers,
        ))
        .layer(axum::middleware::from_fn(super::logging::request_log)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::test_state::HttpStateTestBuilder;
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use tower_service::Service;

    fn request_with_host(method: Method, uri: &str, host: &str) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header(axum::http::header::HOST, host)
            .body(Body::empty())
            .expect("request")
    }

    fn request(method: Method, uri: &str) -> Request<Body> {
        request_with_host(method, uri, "localhost")
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

        let mut router = build_router(state, None).expect("router builds in tests");
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

        let mut router = build_router(state, None).expect("router builds in tests");
        let response = router
            .call(request(Method::GET, "/api/v1/auth/config"))
            .await
            .expect("dispatch");
        assert_eq!(response.status(), StatusCode::OK);
    }

    /// `Router::layer` wraps only the routes that exist at the moment it is
    /// called, so a layer installed at the top of `build_router` is escaped by
    /// every route mounted below it. The Host allowlist is a property of the
    /// deployment boundary (spec §3.3), not of a route, so it is applied last
    /// and has to hold on all of them. The mode disclosure is the regression
    /// case: it is mounted after the route-scoped layers and answered an
    /// arbitrary `Host` before this.
    #[cfg(feature = "control-plane")]
    #[tokio::test]
    async fn the_host_allowlist_covers_routes_mounted_after_the_route_layers() {
        let (builder, _store) = HttpStateTestBuilder::local_admin().await;
        let state = builder.build().await.expect("local admin HTTP state");

        let mut refused = build_router(state.clone(), None).expect("router builds in tests");
        let response = refused
            .call(request_with_host(
                Method::GET,
                "/api/v1/auth/config",
                "evil.example",
            ))
            .await
            .expect("dispatch");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        // The local surface used to carry its own copy of this check. It does
        // not any more, so a merged route has to be covered by the boundary
        // layer as well.
        let mut merged = build_router(state.clone(), None).expect("router builds in tests");
        let response = merged
            .call(request_with_host(
                Method::GET,
                "/api/v1/admin/session",
                "evil.example",
            ))
            .await
            .expect("dispatch");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        // The same route under an allowlisted host, so the assertions above
        // cannot pass for an unrelated reason.
        let mut allowed = build_router(state, None).expect("router builds in tests");
        let response = allowed
            .call(request(Method::GET, "/api/v1/auth/config"))
            .await
            .expect("dispatch");
        assert_eq!(response.status(), StatusCode::OK);
    }

    /// The fallback is what an operator's browser reaches for the embedded UI
    /// shell and every asset under `/assets/`. No route is registered for those
    /// paths, so the only thing that can enforce the boundary there is a layer
    /// applied after the fallback itself exists.
    #[tokio::test]
    async fn the_host_allowlist_covers_the_fallback() {
        let state = HttpStateTestBuilder::new()
            .await
            .build()
            .await
            .expect("off-mode HTTP state");

        let mut refused = build_router(state.clone(), None).expect("router builds in tests");
        let response = refused
            .call(request_with_host(
                Method::GET,
                "/assets/no-such-file.css",
                "evil.example",
            ))
            .await
            .expect("dispatch");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let mut allowed = build_router(state, None).expect("router builds in tests");
        let response = allowed
            .call(request(Method::GET, "/assets/no-such-file.css"))
            .await
            .expect("dispatch");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    async fn prefixed_state(base: &str) -> std::sync::Arc<crate::http::HttpState> {
        let mut config = crate::http::config::HttpConfig::default_for_test();
        config.base_path = base.to_owned();
        config.enable_control_plane_ui = true;
        HttpStateTestBuilder::new()
            .await
            .with_config(config)
            .build()
            .await
            .expect("prefixed HTTP state")
    }

    async fn body_text(response: axum::response::Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// The point of the whole feature: `memory_mcp` must not answer at the
    /// origin root of a shared host. Without the base, `GET /mcp` is a 405
    /// and `GET /health/live` is a 200 — both must become the 404 envelope.
    #[tokio::test]
    async fn with_a_base_path_root_paths_answer_the_404_envelope() {
        let mut router =
            build_router(prefixed_state("/memory").await, None).expect("router builds in tests");
        for (method, uri) in [
            (Method::GET, "/health/live"),
            (Method::GET, "/mcp"),
            (Method::POST, "/mcp"),
        ] {
            let response = router.call(request(method, uri)).await.expect("dispatch");
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
            assert!(body_text(response).await.contains("\"not_found\""), "{uri}");
        }
    }

    #[tokio::test]
    async fn with_a_base_path_routes_reach_their_handlers() {
        let mut router =
            build_router(prefixed_state("/memory").await, None).expect("router builds in tests");

        let response = router
            .call(request(Method::GET, "/memory/health/live"))
            .await
            .expect("dispatch");
        assert_eq!(response.status(), StatusCode::OK);

        // 405 proves `reject_non_post_mcp` saw the *stripped* path: it
        // guards on `path == "/mcp"`, and `/memory/mcp` carried in the URI
        // would fall through to the fallback instead.
        let response = router
            .call(request(Method::GET, "/memory/mcp"))
            .await
            .expect("dispatch");
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);

        // POST passes the method guard and dies at MCP prevalidation with
        // the 400 JSON-RPC envelope — the whole route stack ran. The
        // mirrored MCP headers must be present (content-type + accept), or
        // prevalidation rejects the media type (415/406) before parsing
        // the (absent) envelope.
        let response = Request::builder()
            .method(Method::POST)
            .uri("/memory/mcp")
            .header(axum::http::header::HOST, "localhost")
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .header(
                axum::http::header::ACCEPT,
                "application/json, text/event-stream",
            )
            .body(Body::empty())
            .expect("request");
        let response = router.call(response).await.expect("dispatch");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn a_query_string_survives_the_mount() {
        let mut router =
            build_router(prefixed_state("/memory").await, None).expect("router builds in tests");
        let response = router
            .call(request(Method::GET, "/memory/health/live?probe=1"))
            .await
            .expect("dispatch");
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn the_trailing_slash_form_canonicalizes_to_the_bare_prefix() {
        let mut router =
            build_router(prefixed_state("/memory").await, None).expect("router builds in tests");
        let response = router
            .call(request(Method::GET, "/memory/"))
            .await
            .expect("dispatch");
        assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
        assert_eq!(
            response
                .headers()
                .get("location")
                .and_then(|value| value.to_str().ok()),
            Some("/memory")
        );
    }

    /// `serve_asset`'s not-found is plain `not found` with security headers;
    /// the gap fallback's is the JSON envelope. The body proves the bare
    /// prefix routed to the *inner* SPA fallback (the bundle is absent in
    /// tests, so the SPA route itself 404s — through `serve_asset`).
    #[cfg(feature = "control-plane-ui")]
    #[tokio::test]
    async fn the_bare_prefix_reaches_the_inner_spa_fallback() {
        let mut router =
            build_router(prefixed_state("/memory").await, None).expect("router builds in tests");
        let response = router
            .call(request(Method::GET, "/memory"))
            .await
            .expect("dispatch");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert!(response.headers().contains_key("content-security-policy"));
        assert_eq!(body_text(response).await, "not found");
    }

    #[cfg(feature = "control-plane-ui")]
    #[tokio::test]
    async fn deep_spa_paths_reach_the_inner_spa_fallback() {
        let mut router =
            build_router(prefixed_state("/memory").await, None).expect("router builds in tests");
        let response = router
            .call(request(Method::GET, "/memory/admin/login"))
            .await
            .expect("dispatch");
        assert_eq!(body_text(response).await, "not found");
    }

    /// The deployment boundary must cover the gap fallback too — it is the
    /// one surface that does not sit inside the nested router.
    #[tokio::test]
    async fn the_host_allowlist_covers_the_gap_fallback() {
        let mut router =
            build_router(prefixed_state("/memory").await, None).expect("router builds in tests");
        let response = router
            .call(request_with_host(Method::GET, "/memory/", "evil.example"))
            .await
            .expect("dispatch");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    /// An empty base must keep the exact pre-feature surface.
    #[tokio::test]
    async fn an_empty_base_path_keeps_root_serving() {
        let mut router =
            build_router(prefixed_state("").await, None).expect("router builds in tests");
        let response = router
            .call(request(Method::GET, "/health/live"))
            .await
            .expect("dispatch");
        assert_eq!(response.status(), StatusCode::OK);
        let response = router
            .call(request(Method::GET, "/mcp"))
            .await
            .expect("dispatch");
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
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
