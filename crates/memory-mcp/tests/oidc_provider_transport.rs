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
    let err = match c
        .exchange_code("fixture-code".into(), PkceCode::new())
        .await
    {
        Err(error) => error,
        Ok(_) => panic!("the fixture's invalid grant must be refused"),
    };
    assert_eq!(err.to_string(), "invalid authorization grant");
}

#[tokio::test]
async fn discovery_chunked_without_content_length_is_bounded() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let issuer = format!("http://{}", listener.local_addr().expect("address"));
    let document = serde_json::to_vec(&serde_json::json!({"issuer":issuer,"authorization_endpoint":format!("{issuer}/authorize"),"token_endpoint":format!("{issuer}/token"),"jwks_uri":format!("{issuer}/jwks"),"padding":"x".repeat(262144)})).expect("JSON");
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept");
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            request.push(stream.read_u8().await.expect("request"));
        }
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
            )
            .await
            .expect("headers");
        stream
            .write_all(format!("{:x}\r\n", document.len()).as_bytes())
            .await
            .expect("chunk size");
        let _ = stream.write_all(&document).await;
        let _ = stream.write_all(b"\r\n0\r\n\r\n").await;
    });
    let observed = OidcClient::new(
        &issuer,
        "client",
        "audience",
        "https://app.example/callback",
        "RS256",
    )
    .await;
    assert!(matches!(observed, Err(MemoryError::Unavailable(_))));
    tokio::time::timeout(Duration::from_secs(6), task)
        .await
        .expect("server deadline")
        .expect("server joined");
}

async fn exchange_retry_hint(header: &str) -> Duration {
    let p = OidcProviderFixture::start().await;
    let c = client(&p).await.expect("discovery");
    let mut reply = ProviderReply::status(StatusCode::SERVICE_UNAVAILABLE);
    reply
        .headers
        .insert("retry-after", header.parse().expect("header"));
    p.set_reply(ProviderRoute::Token, reply);
    match c.exchange_code("once".into(), PkceCode::new()).await {
        Err(memory_mcp::control::oidc::AuthError::Unavailable { retry_after }) => retry_after,
        _ => panic!("expected provider unavailability"),
    }
}
#[tokio::test]
async fn numeric_retry_after_is_capped() {
    assert_eq!(exchange_retry_hint("900").await, Duration::from_secs(300));
}
#[tokio::test]
async fn date_retry_after_is_honored() {
    let date = (chrono::Utc::now() + chrono::Duration::seconds(180)).to_rfc2822();
    let hint = exchange_retry_hint(&date).await;
    assert!((Duration::from_secs(170)..=Duration::from_secs(180)).contains(&hint));
}
#[tokio::test]
async fn malformed_retry_after_uses_bounded_fallback() {
    assert_eq!(
        exchange_retry_hint("invalid-date").await,
        Duration::from_secs(1)
    );
}
#[tokio::test]
async fn past_retry_after_uses_bounded_fallback() {
    assert_eq!(
        exchange_retry_hint("Wed, 01 Jan 2020 00:00:00 GMT").await,
        Duration::from_secs(1)
    );
}
