#![cfg(feature = "streamable-http")]

//! The embedded console, asserted as the compiled binary serves it.
//!
//! `crates/memory-mcp/build.rs` stages a `dx bundle` output directory and
//! generates the catalog this crate serves. Nothing else in the suite can see
//! that catalog: it is `include_bytes!`-ed at compile time from whatever
//! `MEMORY_MCP_UI_DIST` named, so a build that embedded the wrong bundle — or
//! none at all — passed every other test in the suite.
//!
//! This file replaces `scripts/ci/assert_embedded_ui.py`, which asserted the
//! same property by probing a live container over HTTP. The catalog is
//! reachable from here, so no container is needed.
//!
//! **A build without a bundle is legitimate.** `MEMORY_MCP_UI_DIST` is unset
//! whenever `ui` is compiled outside the Docker build, which is what CI's
//! default test run does. These tests assert the serving contract that holds in
//! either configuration; the *bundle's* shape is what `cargo xtask
//! check-ui-bundle` asserts, against the staging directory before it is
//! embedded.

use axum::body::Body;
use axum::http::{Response, StatusCode};

use memory_mcp::ui::assets::serve_asset;

/// Whether a console bundle was compiled into this binary.
///
/// Detected through the serving surface itself rather than through a build
/// variable, so the detection cannot disagree with what the binary actually
/// serves: `MEMORY_MCP_UI_DIST` unset produces an empty catalog, and CI's
/// default test run builds exactly that way.
fn a_bundle_is_compiled_in() -> bool {
    serve_asset("/index.html", None, None).status() == StatusCode::OK
}

fn content_type_of(response: &Response<Body>) -> String {
    response
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned()
}

/// Read a response body without pulling an async runtime into the test crate.
fn body_of(response: Response<Body>) -> Vec<u8> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current-thread runtime")
        .block_on(async {
            axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body")
                .to_vec()
        })
}

#[test]
fn a_bundled_document_is_served_at_its_own_path() {
    if !a_bundle_is_compiled_in() {
        return;
    }

    let response = serve_asset("/index.html", None, None);

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "a compiled document must be served"
    );
    assert_eq!(content_type_of(&response), "text/html; charset=utf-8");
    let body = body_of(response);
    assert!(!body.is_empty(), "the document must not be empty");
    assert!(
        String::from_utf8_lossy(&body).contains("<head>"),
        "the document must carry a head for the asset root meta to be stamped into"
    );
}

/// An extensionless path is a client-side route, so it answers with the
/// document rather than a 404. This is the SPA fallback, and it is what makes a
/// deep link like `/admin/login` load at all.
#[test]
fn an_extensionless_route_falls_back_to_the_document() {
    let response = serve_asset("/admin/login", None, None);
    if response.status() == StatusCode::NOT_FOUND && !a_bundle_is_compiled_in() {
        return;
    }

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "a client-side route must answer with the document, not a 404"
    );
    assert_eq!(content_type_of(&response), "text/html; charset=utf-8");
}

/// A path the bundle does not contain is refused, and the refusal is not
/// dressed up as a success: answering HTML for a mistyped asset path is exactly
/// what makes a broken build look like a working one.
#[test]
fn an_unlisted_path_is_refused_rather_than_served() {
    let response = serve_asset("/not-a-real-asset.js", None, None);

    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "a path the bundle does not contain must not be answered with something else"
    );
    // The browser renders a 404 too, so the policy applies to it.
    assert!(response.headers().contains_key("content-security-policy"));
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
}

/// The console's first load is dominated by its WebAssembly module, which is
/// close to a megabyte of bytes a browser decompresses on arrival. The shipped
/// binary carries a pre-compressed copy of every large asset, so a client that
/// can decode one never has to pull the identity form over the wire.
///
/// This runs against the real embedded catalog rather than a fixture: the
/// fixture tests elsewhere prove the negotiation logic, and only this one can
/// prove the *build* staged a compressed copy of the *actual* bundle. A
/// regression in `build.rs` — an asset under the size floor, an already
/// compressed type wrongly skipped — passes every unit test and fails here.
#[test]
fn a_large_embedded_asset_is_served_compressed_and_round_trips() {
    if !a_bundle_is_compiled_in() {
        return;
    }
    // The module is named by the bundle, not by this test: `dx` content-hashes
    // it, so a hardcoded name would make this test fail on every rebuild.
    let wasm = wasm_asset_path()
        .unwrap_or_else(|| panic!("a bundled console must contain a WebAssembly module to load"));

    let compressed = serve_asset(&wasm, None, Some("gzip, deflate, br"));
    assert_eq!(compressed.status(), StatusCode::OK, "path: {wasm}");
    assert_eq!(
        compressed.headers()["content-encoding"],
        "gzip",
        "a client that offers gzip must be told the body is gzip"
    );
    assert_eq!(
        compressed.headers()["vary"],
        "Accept-Encoding",
        "a compressed representation must vary by Accept-Encoding"
    );

    let identity = serve_asset(&wasm, None, Some("identity"));
    assert_eq!(identity.status(), StatusCode::OK, "path: {wasm}");
    assert!(
        !identity.headers().contains_key("content-encoding"),
        "a client that cannot decode must be served the identity body"
    );

    let compressed_body = body_of(compressed);
    let identity_body = body_of(identity);
    assert!(
        compressed_body.len() < identity_body.len(),
        "gzip must actually shrink the module: {} >= {}",
        compressed_body.len(),
        identity_body.len()
    );

    // The header and the bytes have to agree. A `Content-Encoding: gzip` whose
    // body is not valid gzip is a console that fails to boot, and the size
    // comparison above would happily pass it.
    let decoded = decompress(&compressed_body);
    assert_eq!(
        decoded, identity_body,
        "the compressed module must decode to exactly the identity bytes"
    );
    assert!(
        identity_body.starts_with(b"\0asm"),
        "the identity body must be a WebAssembly module, not something else"
    );
}

/// The path of the embedded WebAssembly module, discovered through the catalog
/// rather than hardcoded, so the hash `dx` assigns never invalidates it.
fn wasm_asset_path() -> Option<String> {
    // The loader is a script the document names; the module path lives inside
    // it. `serve_asset` is the only door into the catalog, so the document and
    // the script are read the way a browser would read them.
    let document =
        String::from_utf8_lossy(&body_of(serve_asset("/index.html", None, None))).into_owned();
    let script = document
        .match_indices("<script")
        .filter_map(|(at, _)| {
            let tag = &document[at..];
            tag.find("src=\"").map(|src| &tag[src + "src=\"".len()..])
        })
        .filter_map(|rest| rest.split('"').next())
        .find(|src| src.ends_with(".js"))
        .and_then(canonical)?;
    let script = String::from_utf8_lossy(&body_of(serve_asset(&script, None, None))).into_owned();
    // The loader embeds the module URL as a JavaScript string literal, and
    // which quote it uses is the minifier's choice, so both are read rather
    // than assumed. It also mentions an unbundled `ui_bg.wasm` earlier, so
    // every candidate is tried and the one carrying the bundle's own base
    // prefix is the one that names a catalog entry.
    ["\"", "'"]
        .iter()
        .flat_map(|quote| script.split(*quote))
        .filter(|candidate| candidate.ends_with(".wasm"))
        .find_map(canonical)
}

/// Reduce a bundle URL to the path the catalog is keyed by.
///
/// `dx bundle --base-path` writes the relocatable sentinel into every
/// prefix-dependent URL, so a raw document says
/// `/__memory_mcp_base__/assets/x.js` where the catalog says `/assets/x.js`.
/// A candidate carrying no sentinel is not a catalog path — the loader's own
/// unbundled filename, for one — and yields nothing rather than a wrong
/// answer, which is what lets the caller keep looking.
fn canonical(url: &str) -> Option<String> {
    url.split_once("/__memory_mcp_base__")
        .map(|(_, path)| path.to_owned())
}

fn decompress(body: &[u8]) -> Vec<u8> {
    use std::io::Read as _;
    let mut decoded = Vec::new();
    flate2::read::GzDecoder::new(body)
        .read_to_end(&mut decoded)
        .expect("the served body must be valid gzip");
    decoded
}

/// Every response the UI surface produces carries the policy that makes the
/// console bootable at all. `script-src` must permit `'wasm-unsafe-eval'`:
/// Chromium classifies `WebAssembly.instantiateStreaming` as an eval-like sink,
/// and without the token the SPA never mounts. Asserted across a hit, an SPA
/// route and a miss so the header cannot be attached to only one of them.
#[test]
fn every_ui_response_carries_the_policy_the_wasm_client_needs() {
    for path in ["/index.html", "/admin/login", "/not-a-real-asset.js"] {
        let response = serve_asset(path, None, None);
        let policy = response
            .headers()
            .get("content-security-policy")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();

        assert!(
            policy.contains("wasm-unsafe-eval"),
            "{path} must permit WebAssembly compilation, got: {policy}"
        );
        assert!(
            policy.contains("frame-ancestors 'none'"),
            "{path} must refuse framing, got: {policy}"
        );
        assert!(
            policy.contains("base-uri 'none'"),
            "{path} must refuse a base-uri override, got: {policy}"
        );
    }
}

#[test]
fn public_ui_delivery_interface_serves_missing_assets_with_ui_policy() {
    let response = memory_mcp::ui::assets::serve_asset("/missing.js", None, None);

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(response.headers().contains_key("content-security-policy"));
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
}
