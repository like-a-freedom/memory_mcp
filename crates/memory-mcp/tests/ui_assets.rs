#![cfg(feature = "streamable-http")]

use axum::http::StatusCode;

#[test]
fn public_ui_delivery_interface_serves_missing_assets_with_ui_policy() {
    let response = memory_mcp::ui::assets::serve_asset("/missing.js", None);

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(response.headers().contains_key("content-security-policy"));
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
}
