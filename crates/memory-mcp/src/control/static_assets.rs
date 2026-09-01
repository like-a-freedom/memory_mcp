//! Static asset serving for the optional Dioxus SPA.
//!
//! When `control-plane-ui` is enabled, built assets are embedded
//! via `include_bytes!` and served under `/` with a fallback to
//! `index.html`. API routes take priority via axum's `nest`.

use axum::body::Body;
use axum::http::{HeaderValue, StatusCode, header::CONTENT_TYPE};
use axum::response::Response;

#[derive(Debug, Clone, Copy)]
struct Asset {
    path: &'static str,
    content_type: &'static str,
    body: &'static [u8],
}

#[cfg(feature = "control-plane-ui")]
include!(concat!(env!("OUT_DIR"), "/control_plane_assets.rs"));

#[cfg(not(feature = "control-plane-ui"))]
const ASSETS: &[Asset] = &[];

const INDEX_PATH: &str = "/index.html";

/// Security headers for all responses.
pub fn attach_security_headers(mut resp: Response) -> Response {
    resp.headers_mut().insert(
        "content-security-policy",
        HeaderValue::from_static(
            "default-src 'self'; script-src 'self'; style-src 'self'; \
             connect-src 'self'; frame-ancestors 'none'; object-src 'none'; \
             base-uri 'none'; form-action 'self'",
        ),
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
            path: "/assets/app.js",
            content_type: "text/javascript; charset=utf-8",
            body: b"compiled-js-fixture",
        },
        Asset {
            path: "/index.html",
            content_type: "text/html; charset=utf-8",
            body: b"compiled-index-fixture",
        },
        Asset {
            path: "/styles.css",
            content_type: "text/css; charset=utf-8",
            body: b"compiled-css-fixture",
        },
    ];

    async fn response_body(response: Response) -> Vec<u8> {
        axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("fixture response body should be readable")
            .to_vec()
    }

    fn assert_security_headers(response: &Response) {
        assert_eq!(
            response
                .headers()
                .get("content-security-policy")
                .and_then(|value| value.to_str().ok()),
            Some(
                "default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; frame-ancestors 'none'; object-src 'none'; base-uri 'none'; form-action 'self'"
            ),
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
        let response = serve_asset_from("/assets/app.js?cache=1", FIXTURE_ASSETS);

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/javascript; charset=utf-8")
        );
        assert_security_headers(&response);
        assert_eq!(response_body(response).await, b"compiled-js-fixture");
    }

    #[tokio::test]
    async fn root_and_extensionless_routes_return_compiled_index() {
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
                response_body(response).await,
                b"compiled-index-fixture",
                "path: {path}"
            );
        }
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
    fn generated_catalog_has_sorted_non_empty_index() {
        assert!(ASSETS.windows(2).all(|pair| pair[0].path < pair[1].path));
        let index = ASSETS
            .iter()
            .find(|asset| asset.path == INDEX_PATH)
            .expect("asset build script should generate index.html");
        assert!(!index.body.is_empty());
    }

    #[test]
    fn security_headers_present() {
        let response = Response::new(Body::from("test"));
        let response = attach_security_headers(response);
        assert_security_headers(&response);
    }
}
