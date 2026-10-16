#![cfg(feature = "streamable-http")]
#[path = "common/oidc_provider.rs"]
mod oidc_provider;
use axum::http::StatusCode;
use memory_mcp::control::oidc::JwksCache;
use oidc_provider::{OidcProviderFixture, ProviderReply, ProviderRoute};
use std::time::Duration;
fn cache(p: &OidcProviderFixture) -> JwksCache {
    JwksCache::from_parts(
        reqwest::Client::new(),
        format!("{}/jwks", p.issuer()),
        Duration::from_secs(300),
    )
}
#[tokio::test]
async fn failed_refresh_is_shared_by_waiters() {
    let p = OidcProviderFixture::start().await;
    p.set_reply(
        ProviderRoute::Jwks,
        ProviderReply::status(StatusCode::BAD_GATEWAY),
    );
    let c = cache(&p);
    let (a, b, d) = tokio::join!(c.key_for("a"), c.key_for("b"), c.key_for("c"));
    assert!(a.is_err() && b.is_err() && d.is_err());
    assert_eq!(p.count(ProviderRoute::Jwks), 1);
}
#[tokio::test]
async fn varying_unknown_kids_share_global_cooldown() {
    let p = OidcProviderFixture::start().await;
    let c = cache(&p);
    assert!(c.key_for("a").await.is_err());
    assert!(c.key_for("b").await.is_err());
    assert!(c.key_for("c").await.is_err());
    assert_eq!(p.count(ProviderRoute::Jwks), 1);
}
#[tokio::test]
async fn explicit_refresh_cannot_bypass_cooldown() {
    let p = OidcProviderFixture::start().await;
    p.set_reply(
        ProviderRoute::Jwks,
        ProviderReply::status(StatusCode::BAD_GATEWAY),
    );
    let c = cache(&p);
    assert!(c.refresh().await.is_err());
    assert!(c.refresh().await.is_err());
    assert_eq!(p.count(ProviderRoute::Jwks), 1);
}
