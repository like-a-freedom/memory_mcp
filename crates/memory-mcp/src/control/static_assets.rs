//! Static asset serving for the optional Dioxus SPA.
//!
//! When `control-plane-ui` is enabled, built assets are embedded via
//! `include_bytes!` and served under `/` with a fallback to `index.html`.
//! API routes take priority via axum's `nest`.

use axum::body::Body;
use axum::http::{HeaderValue, StatusCode, header::CACHE_CONTROL, header::CONTENT_TYPE};
use axum::response::Response;

#[derive(Debug, Clone, Copy)]
struct Asset {
    path: &'static str,
    content_type: &'static str,
    /// Whether this asset is content-addressed and safe to cache immutably.
    /// Computed by `build.rs`; the served binary never guesses at a hash format.
    immutable: bool,
    body: &'static [u8],
}

#[cfg(feature = "control-plane-ui")]
include!(concat!(env!("OUT_DIR"), "/control_plane_assets.rs"));

#[cfg(not(feature = "control-plane-ui"))]
const ASSETS: &[Asset] = &[];

const INDEX_PATH: &str = "/index.html";

/// The single source of truth for the SPA policy, shared by the header
/// emitter and its test so the two cannot drift apart.
///
/// `script-src` carries `'wasm-unsafe-eval'` because the Dioxus client is
/// compiled to WebAssembly: Chromium classifies WASM compilation as an
/// eval-like sink, so `script-src 'self'` alone blocks
/// `WebAssembly.instantiateStreaming` and the SPA never mounts. The token
/// permits WebAssembly compilation only; it does not enable JavaScript `eval`
/// or `new Function`, which stay blocked. Verified by the
/// `scripts/ci/local_admin_browser.mjs` `ui` scenario, which loads the real
/// bundle in a browser under this header.
const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; \
     script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; connect-src 'self'; \
     frame-ancestors 'none'; object-src 'none'; base-uri 'none'; form-action 'self'";

/// Security headers for all responses.
pub fn attach_security_headers(mut resp: Response) -> Response {
    resp.headers_mut().insert(
        "content-security-policy",
        HeaderValue::from_static(CONTENT_SECURITY_POLICY),
    );
    resp.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    resp.headers_mut()
        .insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    resp
}

/// Serve a compiled Dioxus asset by path.
///
/// Exact bundle paths win. Extensionless paths outside the asset directory use
/// the compiled index for client-side SPA routes; missing files and malformed
/// paths return 404.
pub fn serve_asset(path: &str) -> Response {
    serve_asset_from(path, ASSETS)
}

fn serve_asset_from(path: &str, assets: &[Asset]) -> Response {
    let Some(path) = request_path(path) else {
        return not_found_response();
    };

    if let Some(asset) = assets.iter().find(|asset| asset.path == path) {
        return asset_response(asset);
    }

    if is_spa_route(path)
        && let Some(index) = assets.iter().find(|asset| asset.path == INDEX_PATH)
    {
        return asset_response(index);
    }

    not_found_response()
}

fn request_path(path: &str) -> Option<&str> {
    let path = path.split_once('?').map_or(path, |(path, _)| path);
    if path.is_empty() || !path.starts_with('/') || path.contains('\\') {
        return None;
    }
    if path
        .split('/')
        .any(|segment| segment == "." || segment == "..")
    {
        return None;
    }
    Some(path)
}

fn is_spa_route(path: &str) -> bool {
    if path == "/" || path == INDEX_PATH {
        return true;
    }
    if path == "/assets" || path.starts_with("/assets/") {
        return false;
    }
    path.rsplit('/')
        .next()
        .is_some_and(|segment| !segment.contains('.'))
}

fn asset_response(asset: &Asset) -> Response {
    let mut resp = Response::new(Body::from(asset.body));
    resp.headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(asset.content_type));
    // Content-addressed bundle assets never change once published, so they can
    // be cached immutably. The index document and any stable-path asset (the
    // favicon) are revalidated, so a new deploy is picked up promptly.
    resp.headers_mut().insert(
        CACHE_CONTROL,
        HeaderValue::from_static(if asset.immutable {
            "public, max-age=31536000, immutable"
        } else {
            "no-cache"
        }),
    );
    attach_security_headers(resp)
}

fn not_found_response() -> Response {
    let mut resp = Response::new(Body::from("not found"));
    *resp.status_mut() = StatusCode::NOT_FOUND;
    attach_security_headers(resp)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE_ASSETS: &[Asset] = &[
        Asset {
            path: "/assets/app-dxh395eca31249da547.js",
            content_type: "text/javascript; charset=utf-8",
            immutable: true,
            body: b"compiled-js-fixture",
        },
        Asset {
            path: "/assets/favicon.svg",
            content_type: "image/svg+xml",
            immutable: false,
            body: b"<svg></svg>",
        },
        Asset {
            path: "/index.html",
            content_type: "text/html; charset=utf-8",
            immutable: false,
            body: b"compiled-index-fixture",
        },
    ];

    async fn response_body(response: Response) -> Vec<u8> {
        axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("fixture response body should be readable")
            .to_vec()
    }

    fn assert_security_headers(response: &Response) {
        let csp = response
            .headers()
            .get("content-security-policy")
            .and_then(|value| value.to_str().ok())
            .expect("csp header");
        assert_eq!(csp, CONTENT_SECURITY_POLICY);
        // The bundle is WebAssembly, so the policy must permit WASM
        // compilation; and nothing broader than that.
        assert!(
            csp.contains("'wasm-unsafe-eval'"),
            "the shipped bundle cannot boot without a WASM-compilation allowance: {csp}"
        );
        assert!(
            !csp.contains("'unsafe-eval'"),
            "general eval must stay blocked: {csp}"
        );
        assert!(
            !csp.contains("'unsafe-inline'"),
            "inline script must stay blocked: {csp}"
        );
        assert_eq!(
            response
                .headers()
                .get("x-content-type-options")
                .and_then(|value| value.to_str().ok()),
            Some("nosniff")
        );
        assert_eq!(
            response
                .headers()
                .get("referrer-policy")
                .and_then(|value| value.to_str().ok()),
            Some("no-referrer")
        );
    }

    #[tokio::test]
    async fn fixture_asset_returns_compiled_bytes_and_content_type() {
        let response =
            serve_asset_from("/assets/app-dxh395eca31249da547.js?cache=1", FIXTURE_ASSETS);

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/javascript; charset=utf-8")
        );
        assert_security_headers(&response);
        assert_eq!(
            response
                .headers()
                .get(CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("public, max-age=31536000, immutable")
        );
        assert_eq!(response_body(response).await, b"compiled-js-fixture");
    }

    #[tokio::test]
    async fn root_and_extensionless_routes_return_revalidated_compiled_index() {
        for path in ["/", "/index.html", "/operator/settings"] {
            let response = serve_asset_from(path, FIXTURE_ASSETS);

            assert_eq!(response.status(), StatusCode::OK, "path: {path}");
            assert_eq!(
                response
                    .headers()
                    .get(CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok()),
                Some("text/html; charset=utf-8"),
                "path: {path}"
            );
            assert_eq!(
                response
                    .headers()
                    .get(CACHE_CONTROL)
                    .and_then(|value| value.to_str().ok()),
                Some("no-cache"),
                "path: {path}"
            );
            assert_eq!(
                response_body(response).await,
                b"compiled-index-fixture",
                "path: {path}"
            );
        }
    }

    #[test]
    fn stable_unhashed_assets_are_revalidated() {
        let response = serve_asset_from("/assets/favicon.svg", FIXTURE_ASSETS);
        assert_eq!(
            response
                .headers()
                .get(CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("no-cache")
        );
    }

    #[test]
    fn missing_assets_return_404_instead_of_index() {
        for path in [
            "/unknown.js",
            "/assets",
            "/assets/unknown.js",
            "/assets/unknown.wasm",
        ] {
            let response = serve_asset_from(path, FIXTURE_ASSETS);
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "path: {path}");
            assert_security_headers(&response);
        }
    }

    #[test]
    fn malformed_and_traversal_paths_return_404() {
        for path in [
            "relative",
            "",
            "/assets/../index.html",
            "/assets/./app.js",
            "/assets\\app.js",
        ] {
            assert_eq!(
                serve_asset_from(path, FIXTURE_ASSETS).status(),
                StatusCode::NOT_FOUND,
                "path: {path}"
            );
        }
    }

    #[cfg(not(feature = "control-plane-ui"))]
    #[test]
    fn disabled_ui_feature_does_not_serve_root() {
        assert_eq!(serve_asset("/").status(), StatusCode::NOT_FOUND);
    }

    #[cfg(feature = "control-plane-ui")]
    #[test]
    fn generated_catalog_is_sorted_and_matches_the_embedded_bundle() {
        // `build.rs` emits an empty catalog when `MEMORY_MCP_CONTROL_PLANE_UI_DIST`
        // was absent (ADR-0056 makes the bundle optional), so sorting is the
        // only invariant that holds for every build. The index assertions apply
        // to a build that embedded a real bundle, where `build.rs` has already
        // refused a bundle without a non-empty `index.html`.
        assert!(ASSETS.windows(2).all(|pair| pair[0].path < pair[1].path));
        if ASSETS.is_empty() {
            return;
        }
        let index = ASSETS
            .iter()
            .find(|asset| asset.path == INDEX_PATH)
            .expect("an embedded bundle must contain index.html");
        assert!(!index.body.is_empty());
        assert!(!index.immutable);
    }

    #[test]
    fn security_headers_present() {
        let response = Response::new(Body::from("test"));
        let response = attach_security_headers(response);
        assert_security_headers(&response);
    }
}
