#![allow(dead_code)]
use axum::{
    Router,
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::{get, post},
};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{Semaphore, mpsc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderRoute {
    Discovery,
    Jwks,
    Token,
}
impl ProviderRoute {
    fn index(self) -> usize {
        self as usize
    }
}
#[derive(Clone)]
pub struct ProviderReply {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}
impl ProviderReply {
    pub fn json(value: serde_json::Value) -> Self {
        Self {
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            body: serde_json::to_vec(&value).expect("fixture JSON"),
        }
    }
    pub fn status(status: StatusCode) -> Self {
        Self {
            status,
            headers: HeaderMap::new(),
            body: vec![],
        }
    }
}
pub struct ProviderRequest {
    pub route: ProviderRoute,
    pub body: Vec<u8>,
}
struct Inner {
    replies: [ProviderReply; 3],
    counts: [usize; 3],
    gates: [Option<Arc<Semaphore>>; 3],
}
struct Shared {
    inner: Mutex<Inner>,
    requests: mpsc::UnboundedSender<ProviderRequest>,
}
pub struct RequestGate(Arc<Semaphore>);
impl RequestGate {
    pub fn release(self) {
        self.0.add_permits(1);
    }
}
pub struct OidcProviderFixture {
    issuer: String,
    state: Arc<Shared>,
    requests: mpsc::UnboundedReceiver<ProviderRequest>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for OidcProviderFixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl OidcProviderFixture {
    pub async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("loopback fixture");
        let issuer = format!("http://{}", listener.local_addr().expect("address"));
        let discovery = ProviderReply::json(
            serde_json::json!({"issuer":issuer,"authorization_endpoint":format!("{issuer}/authorize"),"token_endpoint":format!("{issuer}/token"),"jwks_uri":format!("{issuer}/jwks"),"id_token_signing_alg_values_supported":["RS256"]}),
        );
        let (tx, requests) = mpsc::unbounded_channel();
        let state = Arc::new(Shared {
            inner: Mutex::new(Inner {
                replies: [
                    discovery,
                    signing_keys(),
                    ProviderReply::json(serde_json::json!({"error":"invalid_grant"})),
                ],
                counts: [0; 3],
                gates: Default::default(),
            }),
            requests: tx,
        });
        let app = Router::new()
            .route("/.well-known/openid-configuration", get(discovery_handler))
            .route("/jwks", get(jwks_handler))
            .route("/token", post(token_handler))
            .with_state(state.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("fixture server");
        });
        Self {
            issuer,
            state,
            requests,
            task,
        }
    }
    pub fn issuer(&self) -> &str {
        &self.issuer
    }
    pub fn set_reply(&self, route: ProviderRoute, reply: ProviderReply) {
        self.state.inner.lock().expect("fixture lock").replies[route.index()] = reply;
    }
    pub fn count(&self, route: ProviderRoute) -> usize {
        self.state.inner.lock().expect("fixture lock").counts[route.index()]
    }
    pub fn hold(&self, route: ProviderRoute) -> RequestGate {
        let gate = Arc::new(Semaphore::new(0));
        self.state.inner.lock().expect("fixture lock").gates[route.index()] = Some(gate.clone());
        RequestGate(gate)
    }
    pub async fn next_request(&mut self, route: ProviderRoute) -> ProviderRequest {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let request = self.requests.recv().await.expect("request channel");
                if request.route == route {
                    return request;
                }
            }
        })
        .await
        .expect("provider request deadline")
    }
}
async fn answer(state: Arc<Shared>, route: ProviderRoute, body: Vec<u8>) -> Response {
    let (reply, gate) = {
        let mut inner = state.inner.lock().expect("fixture lock");
        inner.counts[route.index()] += 1;
        (
            inner.replies[route.index()].clone(),
            inner.gates[route.index()].take(),
        )
    };
    let _ = state.requests.send(ProviderRequest { route, body });
    if let Some(gate) = gate {
        gate.acquire().await.expect("gate").forget();
    }
    let mut response = Response::new(Body::from(reply.body));
    *response.status_mut() = reply.status;
    *response.headers_mut() = reply.headers;
    response
}
async fn discovery_handler(State(s): State<Arc<Shared>>) -> Response {
    answer(s, ProviderRoute::Discovery, vec![]).await
}
async fn jwks_handler(State(s): State<Arc<Shared>>) -> Response {
    answer(s, ProviderRoute::Jwks, vec![]).await
}
async fn token_handler(State(s): State<Arc<Shared>>, body: axum::body::Bytes) -> Response {
    answer(s, ProviderRoute::Token, body.to_vec()).await
}

pub const RSA_N: &str = "sWmwOq_tLQb96bfMzWSNEjJRQhd2ZNcZGVBHdGfBu9LJg36Y4tJ62GNLbnMDtQBjiumnN8owIeIOZjypySedttnX6MfvQh9cFkfLxa6d2A2ADuJXkyQNpte0QC1qGKGBjTdqYNX_cfjrwRIlk7HlK2VINjlFpvrcYYJYArVqCMjvCq3L7srhDNeT4L7Ec9ch-cUBhZQFRexdSGAWorBRlMazlQnIJLYpZSydOnnwgX_P1I3W9KXiew5bO3SmV2mo73vf95TayWnc4CWbtOEp3Q_JFgCG8Q0PZYUAx7X7atP26-6dBXeyXBdJ2dNP6BOAzRpiNiuhKT3Fkdslxm8pIQ";
pub fn signing_keys() -> ProviderReply {
    ProviderReply::json(
        serde_json::json!({"keys":[{"kid":"key-1","kty":"RSA","n":RSA_N,"e":"AQAB","alg":"RS256"}]}),
    )
}
pub fn sign(claims: &serde_json::Value, kid: &str) -> String {
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some(kid.into());
    jsonwebtoken::encode(
        &header,
        claims,
        &jsonwebtoken::EncodingKey::from_rsa_pem(include_bytes!(
            "../fixtures/oauth_test_rsa_key.pem"
        ))
        .expect("fixture key"),
    )
    .expect("fixture signature")
}
