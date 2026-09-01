//! Static asset serving for the optional Dioxus SPA (Task 10.8).
//!
//! When `control-plane-ui` is enabled, built assets are embedded
//! via `include_bytes!` and served under `/` with a fallback to
//! `index.html`. API routes take priority via axum's `nest`.

use axum::http::{HeaderValue, StatusCode};
use axum::response::Response;

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

/// Serve a static asset by path. Returns 404 for unknown paths.
pub fn serve_asset(path: &str) -> Response {
    // In a real build, these would be include_bytes! from the
    // Dioxus build output. For now, return a placeholder.
    match path {
        "/" | "/index.html" => {
            let body = "<!DOCTYPE html><html><head><title>Memory MCP</title></head><body><div id=\"app\"></div></body></html>";
            let mut resp = Response::new(axum::body::Body::from(body));
            resp.headers_mut().insert(
                axum::http::header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            );
            attach_security_headers(resp)
        }
        _ => {
            let mut resp = Response::new(axum::body::Body::from("not found"));
            *resp.status_mut() = StatusCode::NOT_FOUND;
            attach_security_headers(resp)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serve_index_returns_html() {
        let resp = serve_asset("/");
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(resp.headers().get("content-security-policy").is_some());
    }

    #[test]
    fn serve_unknown_returns_404() {
        let resp = serve_asset("/unknown.js");
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn security_headers_present() {
        let resp = Response::new(axum::body::Body::from("test"));
        let resp = attach_security_headers(resp);
        assert!(resp.headers().get("x-content-type-options").is_some());
        assert!(resp.headers().get("referrer-policy").is_some());
    }
}
