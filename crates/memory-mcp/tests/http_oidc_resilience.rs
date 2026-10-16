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

async fn bearer(s: &HttpServerFixture, token: &str) -> reqwest::Response {
    http().post(format!("{}/mcp",s.base_url)).header("host","localhost").bearer_auth(token).header("accept","application/json, text/event-stream").header("content-type","application/json").header("mcp-protocol-version","2026-07-28").header("mcp-method","initialize").header("mcp-name", "").json(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2026-07-28","capabilities":{},"clientInfo":{"name":"oidc","version":"1"},"_meta":common::http_server::modern_meta()}})).send().await.expect("bearer request")
}
#[tokio::test]
async fn unknown_key_with_failed_refresh_returns_503() {
    let mut p = OidcProviderFixture::start().await;
    p.set_reply(
        ProviderRoute::Jwks,
        ProviderReply::status(StatusCode::BAD_GATEWAY),
    );
    let s = HttpServerFixture::spawn(config(&p, "oidc")).await;
    p.next_request(ProviderRoute::Jwks).await;
    let token = oidc_provider::sign(
        &serde_json::json!({"sub":"user","iss":p.issuer(),"aud":"memory-mcp","exp":4102444800u64}),
        "unknown",
    );
    let response = bearer(&s, &token).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert!(
        response.headers()["retry-after"]
            .to_str()
            .expect("retry header")
            .parse::<u64>()
            .expect("seconds")
            >= 1
    );
}
#[tokio::test]
async fn malformed_credential_during_discovery_outage_is_401() {
    let p = OidcProviderFixture::start().await;
    p.set_reply(
        ProviderRoute::Discovery,
        ProviderReply::status(StatusCode::BAD_GATEWAY),
    );
    let s = HttpServerFixture::spawn(config(&p, "oidc")).await;
    let response = bearer(&s, "not-a-jwt").await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(response.headers().contains_key("www-authenticate"));
}

#[tokio::test]
async fn fresh_auto_jwt_survives_token_endpoint_outage() {
    let p = OidcProviderFixture::start().await;
    p.set_reply(
        ProviderRoute::Token,
        ProviderReply::status(StatusCode::BAD_GATEWAY),
    );
    let client = memory_mcp::control::oidc::OidcClient::new(
        p.issuer(),
        "fixture-client",
        "memory-mcp",
        "https://app.example/callback",
        "auto",
    )
    .await
    .expect("discovery");
    client.jwks().refresh().await.expect("warm keys");
    let token = oidc_provider::sign(
        &serde_json::json!({"sub":"user","iss":p.issuer(),"aud":"memory-mcp","exp":4102444800u64}),
        "key-1",
    );
    assert!(
        memory_mcp::http::oauth::validate_token(&token, &client)
            .await
            .is_ok()
    );
    assert_eq!(p.count(ProviderRoute::Token), 0);
}

#[tokio::test]
async fn successful_refresh_without_key_returns_401() {
    let mut p = OidcProviderFixture::start().await;
    let s = HttpServerFixture::spawn(config(&p, "oidc")).await;
    p.next_request(ProviderRoute::Jwks).await;
    let token = oidc_provider::sign(
        &serde_json::json!({"sub":"user","iss":p.issuer(),"aud":"memory-mcp","exp":4102444800u64}),
        "unknown",
    );
    let response = bearer(&s, &token).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(response.headers().contains_key("www-authenticate"));
}

#[tokio::test]
async fn malformed_jwt_payload_during_discovery_outage_is_401() {
    use base64::Engine;
    let p = OidcProviderFixture::start().await;
    p.set_reply(
        ProviderRoute::Discovery,
        ProviderReply::status(StatusCode::BAD_GATEWAY),
    );
    let s = HttpServerFixture::spawn(config(&p, "oidc")).await;
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(br#"{"alg":"RS256","kid":"key-1"}"#);
    let response = bearer(&s, &format!("{encoded}.bm90LWpzb24.YWJj")).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn discovery_backoff_is_visible_without_gating_readiness() {
    let mut p = OidcProviderFixture::start().await;
    p.set_reply(
        ProviderRoute::Discovery,
        ProviderReply::status(StatusCode::BAD_GATEWAY),
    );
    let s = HttpServerFixture::spawn(config(&p, "oidc")).await;
    p.next_request(ProviderRoute::Discovery).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let response = get(&s, "/health/ready").await;
            assert_eq!(response.status(), StatusCode::OK);
            let body: serde_json::Value = response.json().await.expect("health JSON");
            if body["oidc"]["metadata"] == "backoff" {
                assert_eq!(body["status"], "ready");
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("backoff published");
    let metrics = get(&s, "/metrics").await.text().await.expect("metrics");
    assert!(metrics.contains("memory_oidc_provider_requests_total"));
    assert!(
        metrics
            .lines()
            .any(|line| line.starts_with("memory_oidc_provider_ready{")
                && line.contains("component=\"metadata\"")
                && line.ends_with(" 0"))
    );
}
#[tokio::test]
async fn disabled_provider_emits_no_series_or_requests() {
    let p = OidcProviderFixture::start().await;
    let mut local = config(&p, "local");
    local
        .extra_env
        .retain(|(key, _)| !key.starts_with("MEMORY_MCP_HTTP_OIDC_"));
    let s = HttpServerFixture::spawn(local).await;
    assert_eq!(get(&s, "/health/ready").await.status(), StatusCode::OK);
    let metrics = get(&s, "/metrics").await.text().await.expect("metrics");
    assert!(
        !metrics
            .lines()
            .any(|line| line.starts_with("memory_oidc_provider_"))
    );
    assert_eq!(p.count(ProviderRoute::Discovery), 0);
    assert_eq!(p.count(ProviderRoute::Jwks), 0);
}

fn discovery(p: &OidcProviderFixture) -> serde_json::Value {
    serde_json::json!({"issuer":p.issuer(),"authorization_endpoint":format!("{}/authorize",p.issuer()),"token_endpoint":format!("{}/token",p.issuer()),"jwks_uri":format!("{}/jwks",p.issuer()),"id_token_signing_alg_values_supported":["RS256"]})
}
#[tokio::test]
async fn cold_outage_recovers_and_completes_browser_sign_in() {
    use base64::Engine;
    use sha2::Digest;
    let mut p = OidcProviderFixture::start().await;
    p.set_reply(
        ProviderRoute::Discovery,
        ProviderReply::status(StatusCode::BAD_GATEWAY),
    );
    let s = HttpServerFixture::spawn(
        config(&p, "oidc").with_env("MEMORY_MCP_HTTP_SIGNUP_MODE", "open"),
    )
    .await;
    assert_independent_service(&s).await;
    p.next_request(ProviderRoute::Discovery).await;
    p.set_reply(ProviderRoute::Discovery, ProviderReply::json(discovery(&p)));
    p.next_request(ProviderRoute::Jwks).await;
    let authorize = get(&s, "/auth/oidc/authorize").await;
    assert_eq!(authorize.status(), StatusCode::SEE_OTHER);
    let url = reqwest::Url::parse(authorize.headers()["location"].to_str().expect("redirect"))
        .expect("authorize URL");
    let params: std::collections::HashMap<String, String> =
        url.query_pairs().into_owned().collect();
    let token = oidc_provider::sign(
        &serde_json::json!({"sub":"resilience-user","iss":p.issuer(),"aud":"memory-mcp","exp":4102444800u64,"nonce":params["nonce"]}),
        "key-1",
    );
    p.set_reply(
        ProviderRoute::Token,
        ProviderReply::json(serde_json::json!({"id_token":token})),
    );
    let response = get(
        &s,
        &format!(
            "/auth/oidc/callback?code=fixture-code&state={}",
            params["state"]
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let cookie = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|h| h.to_str().ok())
        .find(|h| h.starts_with("__Host-memory_mcp_session="))
        .expect("session cookie")
        .split(';')
        .next()
        .expect("cookie pair")
        .to_owned();
    let request = p.next_request(ProviderRoute::Token).await;
    let body = String::from_utf8(request.body).expect("form UTF8");
    let form_url = reqwest::Url::parse(&format!("http://fixture/?{body}")).expect("form");
    let form: std::collections::HashMap<String, String> =
        form_url.query_pairs().into_owned().collect();
    assert_eq!(form["code"], "fixture-code");
    assert_eq!(
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(sha2::Sha256::digest(form["code_verifier"].as_bytes())),
        params["code_challenge"]
    );
    p.set_reply(
        ProviderRoute::Token,
        ProviderReply::status(StatusCode::BAD_GATEWAY),
    );
    let second = get(&s, "/auth/oidc/authorize").await;
    let url =
        reqwest::Url::parse(second.headers()["location"].to_str().expect("redirect")).expect("URL");
    let params: std::collections::HashMap<String, String> =
        url.query_pairs().into_owned().collect();
    let callback = format!(
        "/auth/oidc/callback?code=outage-code&state={}",
        params["state"]
    );
    assert_eq!(
        get(&s, &callback).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert!(get(&s, &callback).await.status().is_client_error());
    let session = http()
        .get(format!("{}/api/v1/account", s.base_url))
        .header("host", "localhost")
        .header("cookie", cookie)
        .send()
        .await
        .expect("session");
    assert_eq!(session.status(), StatusCode::OK);
    let access = oidc_provider::sign(
        &serde_json::json!({"sub":"resilience-user","iss":p.issuer(),"aud":"memory-mcp","exp":4102444800u64}),
        "key-1",
    );
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let response = bearer(&s, &access).await;
            if response.status() == StatusCode::OK {
                break;
            }
            assert_eq!(
                response.status(),
                StatusCode::SERVICE_UNAVAILABLE,
                "{}",
                response.text().await.expect("error body")
            );
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("new tenant becomes ready");
    assert_eq!(get(&s, "/health/ready").await.status(), StatusCode::OK);
    assert_eq!(p.count(ProviderRoute::Token), 2);
}
#[tokio::test]
async fn invalid_issuer_never_publishes_before_valid_recovery() {
    let mut p = OidcProviderFixture::start().await;
    let mut invalid = discovery(&p);
    invalid["issuer"] = serde_json::json!("https://wrong.example");
    p.set_reply(ProviderRoute::Discovery, ProviderReply::json(invalid));
    let s = HttpServerFixture::spawn(config(&p, "oidc")).await;
    p.next_request(ProviderRoute::Discovery).await;
    assert_eq!(
        get(&s, "/auth/oidc/authorize").await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(p.count(ProviderRoute::Jwks), 0);
    p.set_reply(ProviderRoute::Discovery, ProviderReply::json(discovery(&p)));
    p.next_request_within(ProviderRoute::Discovery, Duration::from_secs(75))
        .await;
    p.next_request(ProviderRoute::Jwks).await;
    assert_eq!(
        get(&s, "/auth/oidc/authorize").await.status(),
        StatusCode::SEE_OTHER
    );
}
#[tokio::test]
async fn blocked_discovery_shutdown_reaps_child() {
    let mut p = OidcProviderFixture::start().await;
    let _held = p.hold(ProviderRoute::Discovery);
    let mut s = HttpServerFixture::spawn(
        config(&p, "oidc").with_env("MEMORY_MCP_HTTP_SHUTDOWN_GRACE_SECS", "1"),
    )
    .await;
    p.next_request(ProviderRoute::Discovery).await;
    assert!(s.stop_gracefully(Duration::from_secs(10)).await.success());
    p.shutdown().await;
}
#[tokio::test]
async fn blocked_jwks_shutdown_reaps_child() {
    let mut p = OidcProviderFixture::start().await;
    let _held = p.hold(ProviderRoute::Jwks);
    let mut s = HttpServerFixture::spawn(
        config(&p, "oidc").with_env("MEMORY_MCP_HTTP_SHUTDOWN_GRACE_SECS", "1"),
    )
    .await;
    p.next_request(ProviderRoute::Jwks).await;
    assert!(s.stop_gracefully(Duration::from_secs(10)).await.success());
    p.shutdown().await;
}
