//! Static asset serving for the optional Dioxus SPA.
//!
//! When `control-plane-ui` is enabled, built assets are embedded via
//! `include_bytes!` and served under `/` with a fallback to `index.html`.
//! API routes take priority via axum's `nest`.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{HeaderValue, StatusCode, header::CACHE_CONTROL, header::CONTENT_TYPE};
use axum::response::Response;

use crate::error::MemoryError;

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

/// Build-time marker baked into the UI bundle by
/// `dx bundle --base-path /__memory_mcp_base__`. The compiled `index.html`
/// carries it in every prefix-dependent URL and in the `DIOXUS_ASSET_ROOT`
/// meta; [`stamp_index_html`] replaces it with the deployed mount base at
/// startup. The same literal lives in `crates/control-plane-ui/src/base.rs`,
/// `crates/control-plane-ui/index.html`, the `Dockerfile`'s `dx bundle`
/// invocation and `scripts/ci/local_admin_browser.mjs` — all five must change
/// together.
pub(crate) const BASE_PATH_SENTINEL: &str = "/__memory_mcp_base__";

const META_PREFIX: &str = r#"<meta name="DIOXUS_ASSET_ROOT" content=""#;
const HEAD_TAG: &str = "<head>";

/// Replaces every sentinel occurrence in the compiled `index.html` with
/// `base` and guarantees the `DIOXUS_ASSET_ROOT` meta carries exactly `base`,
/// so the WASM's runtime base resolution reads the deployed base. `base` is
/// the validated mount base (`""` = origin root).
pub fn stamp_index_html(raw: &[u8], base: &str) -> Result<Vec<u8>, MemoryError> {
    let html = std::str::from_utf8(raw).map_err(|_| {
        MemoryError::ConfigInvalid(
            "the embedded control-plane index.html is not valid UTF-8".to_string(),
        )
    })?;
    if !base.is_empty() && !html.contains(BASE_PATH_SENTINEL) {
        return Err(MemoryError::ConfigInvalid(format!(
            "the embedded control-plane bundle was not built with \
             `dx bundle --base-path {BASE_PATH_SENTINEL}`; rebuild the UI bundle \
             (see README \"Control-plane UI asset packaging\")"
        )));
    }
    let stamped = stamp_text(html, base)?;
    let stamped = with_meta(stamped, base);
    debug_assert!(!stamped.contains(BASE_PATH_SENTINEL));
    Ok(stamped.into_bytes())
}

/// Sentinel replacement for one UTF-8 asset, with the mount-base guard shared
/// by every stamping path.
fn stamp_text(text: &str, base: &str) -> Result<String, MemoryError> {
    if base.contains(BASE_PATH_SENTINEL) {
        return Err(MemoryError::ConfigInvalid(
            "the mount base itself must not contain the bundle sentinel".to_string(),
        ));
    }
    Ok(text.replace(BASE_PATH_SENTINEL, base))
}

/// Rewrites the `content` of the `DIOXUS_ASSET_ROOT` meta to `base`, or
/// inserts the element at the start of `<head>` when the bundle has none
/// (dioxus-cli 0.7.10 does not emit the meta, so insertion is the live path).
fn with_meta(html: String, base: &str) -> String {
    if let Some(start) = html.find(META_PREFIX) {
        let value_start = start + META_PREFIX.len();
        let width = html[value_start..].find('"').unwrap_or(0);
        let mut out = String::with_capacity(html.len() + base.len());
        out.push_str(&html[..value_start]);
        out.push_str(base);
        out.push_str(&html[value_start + width..]);
        return out;
    }
    let insert_at = html.find(HEAD_TAG).map_or(0, |pos| pos + HEAD_TAG.len());
    let mut out = String::with_capacity(html.len() + base.len() + META_PREFIX.len() + 2);
    out.push_str(&html[..insert_at]);
    out.push_str(META_PREFIX);
    out.push_str(base);
    out.push('"');
    out.push('>');
    out.push_str(&html[insert_at..]);
    out
}

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

/// Prefix-dependent bundle bytes, stamped with the mount base at router
/// assembly. `dx bundle --base-path` bakes the sentinel not only into
/// `index.html` (asset URLs, the `DIOXUS_ASSET_ROOT` meta) but also into the
/// JS loader's hashed WASM URL, so every UTF-8 asset carrying the sentinel is
/// stamped. Binary assets (the WASM) pass through byte for byte: their baked
/// `DIOXUS_ASSET_ROOT` constant is filtered client-side (`base.rs`).
pub struct StampedAssets {
    entries: Vec<(&'static str, Vec<u8>)>,
}

impl StampedAssets {
    /// The stamped bytes for `path`, when that asset carries the sentinel.
    pub fn get(&self, path: &str) -> Option<&[u8]> {
        self.entries
            .iter()
            .find(|(entry_path, _)| *entry_path == path)
            .map(|(_, body)| body.as_slice())
    }
}

/// Stamps every prefix-dependent asset once, at router assembly. `None` when
/// no UI bundle is compiled into the binary.
pub fn build_stamped_assets(base: &str) -> Result<Option<Arc<StampedAssets>>, MemoryError> {
    Ok(stamped_entries(ASSETS, base)?.map(Arc::new))
}

fn stamped_entries(assets: &[Asset], base: &str) -> Result<Option<StampedAssets>, MemoryError> {
    let mut entries = Vec::new();
    for asset in assets {
        if asset.path == INDEX_PATH {
            entries.push((asset.path, stamp_index_html(asset.body, base)?));
            continue;
        }
        let Ok(text) = std::str::from_utf8(asset.body) else {
            continue;
        };
        if text.contains(BASE_PATH_SENTINEL) {
            entries.push((asset.path, stamp_text(text, base)?.into_bytes()));
        }
    }
    if entries.is_empty() {
        return Ok(None);
    }
    Ok(Some(StampedAssets { entries }))
}

/// Serve a compiled Dioxus asset by path. `stamped` carries the
/// mount-base-stamped bytes of every prefix-dependent asset from
/// [`build_stamped_assets`]; they replace the compiled ones wherever present
/// (SPA routes and `/index.html` included).
///
/// Exact bundle paths win. Extensionless paths outside the asset directory use
/// the compiled index for client-side SPA routes; missing files and malformed
/// paths return 404.
pub fn serve_asset(path: &str, stamped: Option<&StampedAssets>) -> Response {
    serve_asset_from(path, ASSETS, stamped)
}

fn serve_asset_from(path: &str, assets: &[Asset], stamped: Option<&StampedAssets>) -> Response {
    let Some(path) = request_path(path) else {
        return not_found_response();
    };

    if let Some(asset) = assets.iter().find(|asset| asset.path == path) {
        return asset_response(asset, stamped.and_then(|s| s.get(asset.path)));
    }

    if is_spa_route(path)
        && let Some(index) = assets.iter().find(|asset| asset.path == INDEX_PATH)
    {
        return asset_response(index, stamped.and_then(|s| s.get(index.path)));
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

fn asset_response(asset: &Asset, stamped: Option<&[u8]>) -> Response {
    // Stamped bytes win: the compiled asset still carries the build sentinel
    // and must never reach a client on a prefixed deployment.
    let body = match stamped {
        Some(bytes) => Body::from(bytes.to_vec()),
        None => Body::from(asset.body),
    };
    let mut resp = Response::new(body);
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
        let response = serve_asset_from(
            "/assets/app-dxh395eca31249da547.js?cache=1",
            FIXTURE_ASSETS,
            None,
        );

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
            let response = serve_asset_from(path, FIXTURE_ASSETS, None);

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
        let response = serve_asset_from("/assets/favicon.svg", FIXTURE_ASSETS, None);
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
            let response = serve_asset_from(path, FIXTURE_ASSETS, None);
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
                serve_asset_from(path, FIXTURE_ASSETS, None).status(),
                StatusCode::NOT_FOUND,
                "path: {path}"
            );
        }
    }

    #[cfg(not(feature = "control-plane-ui"))]
    #[test]
    fn disabled_ui_feature_does_not_serve_root() {
        assert_eq!(serve_asset("/", None).status(), StatusCode::NOT_FOUND);
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

    const PROBE_INDEX: &str = r#"<!DOCTYPE html><html><head><meta name="DIOXUS_ASSET_ROOT" content="/__memory_mcp_base__"><link rel="icon" type="image/svg+xml" href="/__memory_mcp_base__/assets/favicon.svg"></head><body><script src="/__memory_mcp_base__/./assets/app-1.js"></script></body></html>"#;

    #[test]
    fn stamps_every_sentinel_occurrence_with_the_mount_base() {
        let out = stamp_index_html(PROBE_INDEX.as_bytes(), "/memory").expect("stamps");
        let out = String::from_utf8(out).expect("utf-8");
        assert!(!out.contains(BASE_PATH_SENTINEL));
        assert!(out.contains(r#"href="/memory/assets/favicon.svg""#));
        assert!(out.contains(r#"src="/memory/./assets/app-1.js""#));
        assert!(out.contains(r#"<meta name="DIOXUS_ASSET_ROOT" content="/memory">"#));
    }

    #[test]
    fn an_empty_base_reproduces_root_absolute_urls() {
        let out = stamp_index_html(PROBE_INDEX.as_bytes(), "").expect("stamps");
        let out = String::from_utf8(out).expect("utf-8");
        assert!(out.contains(r#"href="/assets/favicon.svg""#));
        assert!(out.contains(r#"src="/./assets/app-1.js""#));
        assert!(out.contains(r#"<meta name="DIOXUS_ASSET_ROOT" content="">"#));
    }

    #[test]
    fn a_root_base_accepts_a_legacy_bundle_without_the_sentinel() {
        let raw = r#"<!DOCTYPE html><html><head></head><body></body></html>"#;
        assert!(stamp_index_html(raw.as_bytes(), "").is_ok());
    }

    #[test]
    fn a_prefixed_base_rejects_a_bundle_without_the_sentinel() {
        let raw = r#"<!DOCTYPE html><html><head></head><body></body></html>"#;
        let err = stamp_index_html(raw.as_bytes(), "/memory").expect_err("must reject");
        assert!(matches!(err, MemoryError::ConfigInvalid(_)), "{err:?}");
    }

    #[test]
    fn missing_meta_is_injected_so_the_wasm_reads_the_deployed_base() {
        let raw = r#"<!DOCTYPE html><html><head><title>t</title></head><body><script src="/__memory_mcp_base__/./assets/app-1.js"></script></body></html>"#;
        let out = stamp_index_html(raw.as_bytes(), "/memory").expect("stamps");
        let out = String::from_utf8(out).expect("utf-8");
        assert!(out.contains(r#"<head><meta name="DIOXUS_ASSET_ROOT" content="/memory">"#));
    }

    #[test]
    fn a_stale_meta_content_is_rewritten_to_the_deployed_base() {
        let raw = r#"<!DOCTYPE html><html><head><meta name="DIOXUS_ASSET_ROOT" content=""></head><body><script src="/__memory_mcp_base__/./assets/app-1.js"></script></body></html>"#;
        let out = stamp_index_html(raw.as_bytes(), "/memory").expect("stamps");
        let out = String::from_utf8(out).expect("utf-8");
        assert!(out.contains(r#"<meta name="DIOXUS_ASSET_ROOT" content="/memory">"#));
    }

    #[test]
    fn non_utf8_input_is_a_config_error() {
        let err = stamp_index_html(&[0xff, 0xfe], "/memory").expect_err("must reject");
        assert!(matches!(err, MemoryError::ConfigInvalid(_)), "{err:?}");
    }

    #[test]
    fn a_base_that_itself_contains_the_sentinel_is_rejected() {
        // The public-URL grammar would allow such a base segment; stamping it
        // would leave sentinel residue in the served shell, so it must fail
        // loudly at router assembly instead.
        let err =
            stamp_index_html(PROBE_INDEX.as_bytes(), BASE_PATH_SENTINEL).expect_err("must reject");
        assert!(matches!(err, MemoryError::ConfigInvalid(_)), "{err:?}");
    }

    const PREFIXED_ASSETS: &[Asset] = &[
        Asset {
            path: "/assets/app-dxh.js",
            content_type: "text/javascript; charset=utf-8",
            immutable: true,
            body: b"init(\"/__memory_mcp_base__/assets/app_bg-dxh.wasm\");",
        },
        Asset {
            path: "/index.html",
            content_type: "text/html; charset=utf-8",
            immutable: false,
            body: b"<!DOCTYPE html><html><head></head><body><script src=\"/__memory_mcp_base__/assets/app-dxh.js\"></script></body></html>",
        },
    ];

    const BINARY_ASSETS: &[Asset] = &[Asset {
        path: "/assets/app_bg-dxh.wasm",
        content_type: "application/wasm",
        immutable: true,
        body: b"/__memory_mcp_base__\xff\xfe",
    }];

    #[tokio::test]
    async fn a_stamped_index_replaces_the_compiled_document_on_spa_routes() {
        let stamped = stamped_entries(PREFIXED_ASSETS, "/memory")
            .expect("stamps")
            .expect("entries");
        let response = serve_asset_from("/admin/clients", PREFIXED_ASSETS, Some(&stamped));
        let body = response_body(response).await;
        let body = String::from_utf8(body).expect("utf-8");
        assert!(
            body.contains(r#"src="/memory/assets/app-dxh.js""#),
            "{body}"
        );
        assert!(!body.contains(BASE_PATH_SENTINEL));
    }

    #[tokio::test]
    async fn the_index_path_also_serves_the_stamped_document() {
        let stamped = stamped_entries(PREFIXED_ASSETS, "/memory")
            .expect("stamps")
            .expect("entries");
        let response = serve_asset_from("/index.html", PREFIXED_ASSETS, Some(&stamped));
        let body = response_body(response).await;
        let body = String::from_utf8(body).expect("utf-8");
        assert!(
            body.contains(r#"src="/memory/assets/app-dxh.js""#),
            "{body}"
        );
    }

    #[tokio::test]
    async fn a_text_asset_carrying_the_sentinel_is_served_stamped() {
        // `dx bundle` bakes the sentinel into the JS loader too (its hashed
        // WASM URL), not just index.html. Any UTF-8 asset carrying the
        // sentinel must be stamped, or the loader 404s on a prefixed
        // deployment and the WASM never boots.
        let stamped = stamped_entries(PREFIXED_ASSETS, "/memory")
            .expect("stamps")
            .expect("entries");
        let response = serve_asset_from("/assets/app-dxh.js", PREFIXED_ASSETS, Some(&stamped));
        assert_eq!(
            response_body(response).await,
            br#"init("/memory/assets/app_bg-dxh.wasm");"#
        );
    }

    #[tokio::test]
    async fn binary_assets_are_served_byte_for_byte() {
        // The WASM is not UTF-8; its baked `DIOXUS_ASSET_ROOT` constant is
        // filtered client-side (`base.rs`) and its bytes must never be
        // rewritten here.
        let stamped = stamped_entries(BINARY_ASSETS, "/memory").expect("walks the catalog");
        assert!(stamped.is_none(), "binary assets are never stamped");
        let response = serve_asset_from("/assets/app_bg-dxh.wasm", BINARY_ASSETS, stamped.as_ref());
        assert_eq!(
            response_body(response).await,
            b"/__memory_mcp_base__\xff\xfe"
        );
    }
}
