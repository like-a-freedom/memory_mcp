#![cfg(feature = "streamable-http")]
#[path = "common/oidc_provider.rs"]
mod oidc_provider;
use axum::http::StatusCode;
use memory_mcp::{
    MemoryError,
    control::oidc::{OidcClient, PkceCode},
};
use oidc_provider::{OidcProviderFixture, ProviderReply, ProviderRoute};
use std::time::Duration;
async fn client(provider: &OidcProviderFixture) -> Result<OidcClient, MemoryError> {
    OidcClient::new(
        provider.issuer(),
        "client",
        "audience",
        "https://app.example/callback",
        "RS256",
    )
    .await
}

#[tokio::test]
async fn discovery_502_is_unavailable_not_bad_configuration() {
    let p = OidcProviderFixture::start().await;
    p.set_reply(
        ProviderRoute::Discovery,
        ProviderReply::status(StatusCode::BAD_GATEWAY),
    );
    assert!(matches!(client(&p).await, Err(MemoryError::Unavailable(_))));
}
#[tokio::test]
async fn discovery_redirect_is_not_followed() {
    let target = OidcProviderFixture::start().await;
    let p = OidcProviderFixture::start().await;
    let mut reply = ProviderReply::status(StatusCode::FOUND);
    reply.headers.insert(
        "location",
        format!("{}/.well-known/openid-configuration", target.issuer())
            .parse()
            .expect("location"),
    );
    p.set_reply(ProviderRoute::Discovery, reply);
    let _ = client(&p).await;
    assert_eq!(target.count(ProviderRoute::Discovery), 0);
}
#[tokio::test]
async fn discovery_over_262144_bytes_is_refused() {
    let p = OidcProviderFixture::start().await;
    p.set_reply(ProviderRoute::Discovery,ProviderReply::json(serde_json::json!({"issuer":p.issuer(),"authorization_endpoint":format!("{}/authorize",p.issuer()),"token_endpoint":format!("{}/token",p.issuer()),"jwks_uri":format!("{}/jwks",p.issuer()),"padding":"x".repeat(262144)})));
    assert!(client(&p).await.is_err());
}
#[tokio::test]
async fn token_timeout_is_bounded_and_does_not_replay_code() {
    let p = OidcProviderFixture::start().await;
    let c = client(&p).await.expect("discovery");
    let _gate = p.hold(ProviderRoute::Token);
    let observed = tokio::time::timeout(
        Duration::from_secs(6),
        c.exchange_code("fixture-code".into(), PkceCode::new()),
    )
    .await;
    assert!(observed.is_ok(), "exchange must finish within six seconds");
    assert!(observed.expect("bounded exchange").is_err());
    assert_eq!(p.count(ProviderRoute::Token), 1);
}
#[tokio::test]
async fn invalid_grant_is_a_credential_refusal() {
    let p = OidcProviderFixture::start().await;
    let c = client(&p).await.expect("discovery");
    p.set_reply(ProviderRoute::Token,ProviderReply { status:StatusCode::BAD_REQUEST,..ProviderReply::json(serde_json::json!({"error":"invalid_grant","error_description":"fixture-secret-token"})) });
    let err = c
        .exchange_code("fixture-code".into(), PkceCode::new())
        .await
        .err()
        .expect("refused code");
    assert_eq!(err.to_string(), "invalid authorization grant");
}
