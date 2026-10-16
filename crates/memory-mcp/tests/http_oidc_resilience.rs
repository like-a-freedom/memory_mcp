#![cfg(all(feature = "streamable-http", feature = "test-fixtures"))]
mod common;
#[path = "common/oidc_provider.rs"]
mod oidc_provider;
use axum::http::StatusCode;
use common::http_server::{HttpServerConfig, HttpServerFixture, TestTenant, mcp_call};
use oidc_provider::{OidcProviderFixture, ProviderReply, ProviderRoute};
use std::time::Duration;
fn config(p: &OidcProviderFixture, methods: &str) -> HttpServerConfig {
    HttpServerConfig::default()
        .with_tenant(TestTenant::new(
            "oidc_resilience",
            "mem_sk_ak_01234567-89ab-4cde-8f01-23456789abcd_conformancesuite0123456789abcdef",
        ))
        .with_env("MEMORY_MCP_HTTP_AUTH_METHODS", methods)
        .with_env("MEMORY_MCP_HTTP_OIDC_ISSUER", p.issuer())
        .with_env("MEMORY_MCP_HTTP_OIDC_CLIENT_ID", "fixture-client")
        .with_env("MEMORY_MCP_HTTP_OIDC_AUDIENCE", "memory-mcp")
        .with_env(
            "MEMORY_MCP_HTTP_OIDC_REDIRECT_URI",
            "https://app.example/auth/oidc/callback",
        )
        .with_env("MEMORY_MCP_HTTP_OIDC_ALLOWED_ALG", "RS256")
}
fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5))
        .build()
        .expect("test client")
}
async fn get(s: &HttpServerFixture, path: &str) -> reqwest::Response {
    http()
        .get(format!("{}{path}", s.base_url))
        .header("host", "localhost")
        .send()
        .await
        .expect("HTTP response")
}
async fn assert_independent_service(s: &HttpServerFixture) {
    assert_eq!(get(s, "/health/live").await.status(), StatusCode::OK);
    assert_eq!(get(s, "/health/ready").await.status(), StatusCode::OK);
    let result=mcp_call(s.client(),&s.base_url,"mem_sk_ak_01234567-89ab-4cde-8f01-23456789abcd_conformancesuite0123456789abcdef","initialize",serde_json::json!({"protocolVersion":"2026-07-28","capabilities":{},"clientInfo":{"name":"oidc-resilience","version":"1"}})).await;
    assert!(result["payload"].get("result").is_some());
    let authorize = get(s, "/auth/oidc/authorize").await;
    assert_eq!(authorize.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(authorize.headers().contains_key("retry-after"));
}
#[tokio::test]
async fn cold_oidc_outage_preserves_api_key_service() {
    let p = OidcProviderFixture::start().await;
    p.set_reply(
        ProviderRoute::Discovery,
        ProviderReply::status(StatusCode::BAD_GATEWAY),
    );
    let s = HttpServerFixture::spawn(config(&p, "oidc")).await;
    assert_independent_service(&s).await;
}
#[tokio::test]
async fn cold_combined_auth_outage_preserves_local_routes() {
    let p = OidcProviderFixture::start().await;
    p.set_reply(
        ProviderRoute::Discovery,
        ProviderReply::status(StatusCode::BAD_GATEWAY),
    );
    let s = HttpServerFixture::spawn(config(&p, "local,oidc")).await;
    assert_independent_service(&s).await;
}
#[tokio::test]
async fn oidc_recovers_without_auth_demand() {
    let mut p = OidcProviderFixture::start().await;
    p.set_reply(
        ProviderRoute::Discovery,
        ProviderReply::status(StatusCode::BAD_GATEWAY),
    );
    let s = HttpServerFixture::spawn(config(&p, "oidc")).await;
    p.next_request(ProviderRoute::Discovery).await;
    p.set_reply(ProviderRoute::Discovery,ProviderReply::json(serde_json::json!({"issuer":p.issuer(),"authorization_endpoint":format!("{}/authorize",p.issuer()),"token_endpoint":format!("{}/token",p.issuer()),"jwks_uri":format!("{}/jwks",p.issuer())})));
    p.next_request(ProviderRoute::Jwks).await; // metadata was published before independent warmup
    assert_eq!(
        get(&s, "/auth/oidc/authorize").await.status(),
        StatusCode::SEE_OTHER
    );
}
