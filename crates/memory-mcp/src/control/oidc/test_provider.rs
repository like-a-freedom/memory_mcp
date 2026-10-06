//! Loopback OIDC provider for unit tests.
//!
//! Serves a real discovery document, JWKS and token endpoint on `127.0.0.1`,
//! so tests exercise `OidcClient`'s discovery, code exchange and ID-token
//! validation across a real HTTP boundary — including real RSA signature
//! verification against the published JWKS — instead of a stubbed verifier.

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header::CONTENT_TYPE};
use axum::response::Response;
use axum::routing::{get, post};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};

/// A real 2048-bit RSA key pair, so signatures and JWKS decoding are exercised
/// for real rather than against a stubbed verifier.
pub(crate) const RSA_PRIVATE_PEM: &str =
    include_str!("../../../tests/fixtures/oauth_test_rsa_key.pem");
/// The matching public modulus, base64url-encoded as JWKS requires.
pub(crate) const RSA_N: &str = "sWmwOq_tLQb96bfMzWSNEjJRQhd2ZNcZGVBHdGfBu9LJg36Y4tJ62GNLbnMDtQBjiumnN8owIeIOZjypySedttnX6MfvQh9cFkfLxa6d2A2ADuJXkyQNpte0QC1qGKGBjTdqYNX_cfjrwRIlk7HlK2VINjlFpvrcYYJYArVqCMjvCq3L7srhDNeT4L7Ec9ch-cUBhZQFRexdSGAWorBRlMazlQnIJLYpZSydOnnwgX_P1I3W9KXiew5bO3SmV2mo73vf95TayWnc4CWbtOEp3Q_JFgCG8Q0PZYUAx7X7atP26-6dBXeyXBdJ2dNP6BOAzRpiNiuhKT3Fkdslxm8pIQ";
pub(crate) const RSA_E: &str = "AQAB";
pub(crate) const KEY_ID: &str = "key-1";

/// What the mock's token endpoint answers.
#[derive(Clone)]
pub(crate) enum TokenAnswer {
    /// 5xx, the way a provider refuses a code exchange.
    Refused,
    /// 200 with a body that carries no `id_token`.
    NoIdToken,
    /// 200 carrying this `id_token`.
    IdToken(String),
}

/// How the mock's discovery document and token endpoint answer.
pub(crate) struct MockProviderConfig {
    /// The `issuer` the discovery document publishes. `None` publishes the
    /// provider's own base URL.
    pub(crate) published_issuer: Option<String>,
    /// `id_token_signing_alg_values_supported`. An empty vector omits the
    /// field, which a provider that does not advertise its algorithms does.
    pub(crate) algorithms: Vec<String>,
    pub(crate) token: TokenAnswer,
}

impl Default for MockProviderConfig {
    fn default() -> Self {
        Self {
            published_issuer: None,
            algorithms: vec!["RS256".to_string()],
            token: TokenAnswer::Refused,
        }
    }
}

struct ProviderState {
    base: String,
    published_issuer: String,
    algorithms: Vec<String>,
    token: TokenAnswer,
}

/// A loopback identity provider the test process owns; dropping it stops it.
pub(crate) struct MockProvider {
    base_url: String,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for MockProvider {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl MockProvider {
    pub(crate) async fn spawn(config: MockProviderConfig) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("loopback address");
        let base = format!("http://{addr}");
        let published_issuer = config
            .published_issuer
            .clone()
            .unwrap_or_else(|| base.clone());
        let state = Arc::new(ProviderState {
            base: base.clone(),
            published_issuer,
            algorithms: config.algorithms,
            token: config.token,
        });
        let app: Router = Router::new()
            .route("/.well-known/openid-configuration", get(discovery))
            .route("/jwks", get(jwks))
            .route("/token", post(token_endpoint))
            .with_state(state);
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        // Let the listener accept before the first request, so discovery never
        // races the bind.
        tokio::time::sleep(Duration::from_millis(20)).await;
        Self {
            base_url: base,
            task,
        }
    }

    /// The provider's own URL — the value a client configures as its issuer,
    /// and, unless overridden, what discovery publishes back.
    pub(crate) fn base_url(&self) -> &str {
        &self.base_url
    }
}

async fn discovery(State(state): State<Arc<ProviderState>>) -> Response {
    let mut document = serde_json::json!({
        "issuer": state.published_issuer,
        "authorization_endpoint": format!("{}/auth", state.base),
        "token_endpoint": format!("{}/token", state.base),
        "jwks_uri": format!("{}/jwks", state.base),
    });
    if !state.algorithms.is_empty() {
        document["id_token_signing_alg_values_supported"] = serde_json::json!(state.algorithms);
    }
    json_response(&document)
}

async fn jwks() -> Response {
    json_response(&serde_json::json!({
        "keys": [{ "kid": KEY_ID, "kty": "RSA", "n": RSA_N, "e": RSA_E, "alg": "RS256" }]
    }))
}

async fn token_endpoint(State(state): State<Arc<ProviderState>>) -> Response {
    match &state.token {
        TokenAnswer::Refused => Response::builder()
            .status(StatusCode::SERVICE_UNAVAILABLE)
            .body(Body::empty())
            .expect("refusal response"),
        TokenAnswer::NoIdToken => json_response(&serde_json::json!({ "access_token": "opaque" })),
        TokenAnswer::IdToken(id_token) => {
            json_response(&serde_json::json!({ "id_token": id_token }))
        }
    }
}

fn json_response(value: &serde_json::Value) -> Response {
    let body = serde_json::to_vec(value).expect("serialize json response");
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = StatusCode::OK;
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response
}

/// Sign `claims` with the fixture key under the published `kid`, the way a real
/// provider signs an ID token.
pub(crate) fn sign_id_token(claims: &serde_json::Value) -> String {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(KEY_ID.to_string());
    encode(
        &header,
        claims,
        &EncodingKey::from_rsa_pem(RSA_PRIVATE_PEM.as_bytes()).expect("fixture key parses"),
    )
    .expect("token signs")
}
