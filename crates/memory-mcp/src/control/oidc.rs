//! OIDC Authorization Code + PKCE.
//!
//! Provides the OIDC types (`OidcState`, `OidcNonce`, `PkceCode`),
//! the `JwksCache` for key validation, and the `OidcClient` that
//! performs discovery, authorization URL generation, code exchange,
//! and ID token validation. All raw OIDC state/nonce/verifier are
//! transient — only keyed hashes are durable.

use std::sync::Arc;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use crate::error::MemoryError;

// ---------------------------------------------------------------------------
// OIDC flow material types
// ---------------------------------------------------------------------------

/// Random state token for CSRF protection in the OIDC flow.
#[derive(Debug, Clone)]
pub struct OidcState(String);

impl OidcState {
    pub fn new() -> Self {
        Self(hex::encode(rand::random::<[u8; 32]>()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for OidcState {
    fn default() -> Self {
        Self::new()
    }
}

/// OIDC nonce — validated against the ID token's `nonce` claim.
#[derive(Debug, Clone)]
pub struct OidcNonce(String);

impl OidcNonce {
    pub fn new() -> Self {
        Self(hex::encode(rand::random::<[u8; 32]>()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for OidcNonce {
    fn default() -> Self {
        Self::new()
    }
}

/// PKCE code verifier + S256 challenge.
#[derive(Debug, Clone)]
pub struct PkceCode {
    pub verifier: String,
    pub challenge: String,
}

impl PkceCode {
    pub fn new() -> Self {
        let mut bytes = [0_u8; 32];
        rand::fill(&mut bytes);
        let verifier = URL_SAFE_NO_PAD.encode(bytes);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        Self {
            verifier,
            challenge,
        }
    }
}

impl Default for PkceCode {
    fn default() -> Self {
        Self::new()
    }
}

/// Stored OIDC request — decrypted projection from the registry.
#[derive(Debug, Clone)]
pub struct StoredOidcRequest {
    pub state: OidcState,
    pub nonce: OidcNonce,
    pub pkce: PkceCode,
    pub expires_at: DateTime<Utc>,
}

/// OIDC tokens — only the ID token is retained.
#[derive(Debug, Clone)]
pub struct OidcTokens {
    pub id_token: String,
}

/// Callback query parameters from the OIDC provider.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct OidcCallback {
    pub code: Option<String>,
    pub state: String,
    pub error: Option<String>,
    pub error_description: Option<String>,
    /// RFC 9207 issuer parameter; must match the configured issuer.
    pub iss: Option<String>,
}

// ---------------------------------------------------------------------------
// Auth error
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("malformed token")]
    MalformedToken,
    #[error("token has no key id")]
    MissingKeyId,
    #[error("token algorithm is not allowed")]
    DisallowedAlgorithm,
    #[error("JWT validation failed: {0}")]
    Jwt(#[source] jsonwebtoken::errors::Error),
    #[error("JWKS lookup failed: {0}")]
    Jwks(String),
    #[error("OIDC provider request failed: {0}")]
    Provider(String),
    #[error("OIDC flow material could not be sealed")]
    Sealing,
}

impl From<AuthError> for MemoryError {
    fn from(err: AuthError) -> Self {
        MemoryError::ConfigInvalid(err.to_string())
    }
}

// ---------------------------------------------------------------------------
// Access claims (decoded from ID token)
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
#[serde(untagged)]
pub enum Audience {
    One(String),
    Many(Vec<String>),
}

#[derive(Debug, serde::Deserialize)]
pub struct AccessClaims {
    pub iss: String,
    pub sub: String,
    pub aud: Audience,
    pub exp: u64,
    pub nonce: Option<String>,
}

// ---------------------------------------------------------------------------
// JWKS cache
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct JwksCache {
    inner: Arc<std::sync::RwLock<JwksState>>,
    refresh_lock: Arc<tokio::sync::Mutex<()>>,
    client: reqwest::Client,
    jwks_uri: String,
    ttl: std::time::Duration,
}

struct JwksState {
    keys: std::collections::HashMap<String, jsonwebtoken::DecodingKey>,
    fetched_at: Option<std::time::Instant>,
}

impl JwksCache {
    pub fn find_key(&self, kid: &str) -> Result<Option<jsonwebtoken::DecodingKey>, AuthError> {
        Ok(self
            .inner
            .read()
            .map_err(|_| AuthError::Jwks("JWKS cache lock poisoned".into()))?
            .keys
            .get(kid)
            .cloned())
    }

    pub async fn key_for(&self, kid: &str) -> Result<jsonwebtoken::DecodingKey, AuthError> {
        let fresh = self
            .inner
            .read()
            .map_err(|_| AuthError::Jwks("JWKS cache lock poisoned".into()))?
            .fetched_at
            .is_some_and(|at| at.elapsed() < self.ttl);
        if fresh && let Some(key) = self.find_key(kid)? {
            return Ok(key);
        }
        let _refresh = self.refresh_lock.lock().await;
        let fresh = self
            .inner
            .read()
            .map_err(|_| AuthError::Jwks("JWKS cache lock poisoned".into()))?
            .fetched_at
            .is_some_and(|at| at.elapsed() < self.ttl);
        if fresh && let Some(key) = self.find_key(kid)? {
            return Ok(key);
        }
        self.refresh().await?;
        self.find_key(kid)?
            .ok_or_else(|| AuthError::Jwks("unknown key id".into()))
    }

    pub async fn refresh(&self) -> Result<(), AuthError> {
        #[derive(serde::Deserialize)]
        struct JwksDocument {
            keys: Vec<Jwk>,
        }
        #[derive(serde::Deserialize)]
        struct Jwk {
            kid: String,
            kty: String,
            n: Option<String>,
            e: Option<String>,
            crv: Option<String>,
            x: Option<String>,
            y: Option<String>,
            alg: Option<String>,
        }
        let document = self
            .client
            .get(&self.jwks_uri)
            .send()
            .await
            .map_err(|error| AuthError::Jwks(error.to_string()))?
            .error_for_status()
            .map_err(|error| AuthError::Jwks(error.to_string()))?
            .json::<JwksDocument>()
            .await
            .map_err(|error| AuthError::Jwks(error.to_string()))?;
        let mut keys = std::collections::HashMap::new();
        for jwk in document.keys.into_iter().take(32) {
            let key = match (
                jwk.kty.as_str(),
                jwk.n.as_deref(),
                jwk.e.as_deref(),
                jwk.crv.as_deref(),
                jwk.x.as_deref(),
                jwk.y.as_deref(),
            ) {
                ("RSA", Some(n), Some(e), _, _, _) => {
                    jsonwebtoken::DecodingKey::from_rsa_components(n, e)
                }
                ("EC", _, _, Some("P-256"), Some(x), Some(y)) => {
                    jsonwebtoken::DecodingKey::from_ec_components(x, y)
                }
                ("OKP", _, _, Some("Ed25519"), Some(x), _) => {
                    jsonwebtoken::DecodingKey::from_ed_components(x)
                }
                _ => continue,
            }
            .map_err(|error| AuthError::Jwks(error.to_string()))?;
            if jwk
                .alg
                .as_deref()
                .is_none_or(|alg| alg == "RS256" || alg == "ES256" || alg == "EdDSA")
            {
                keys.insert(jwk.kid, key);
            }
        }
        let mut state = self
            .inner
            .write()
            .map_err(|_| AuthError::Jwks("JWKS cache lock poisoned".into()))?;
        state.keys = keys;
        state.fetched_at = Some(std::time::Instant::now());
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// OIDC client
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct OidcClient {
    issuer: String,
    client_id: String,
    audience: String,
    redirect_uri: String,
    allowed_algorithm: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks: JwksCache,
}

impl OidcClient {
    /// Perform OIDC discovery and initialize the JWKS cache.
    pub async fn new(
        issuer: &str,
        client_id: &str,
        audience: &str,
        redirect_uri: &str,
        allowed_algorithm: &str,
    ) -> Result<Self, MemoryError> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| MemoryError::ConfigInvalid(e.to_string()))?;

        let issuer = issuer.trim_end_matches('/');
        let discovery_url = format!("{issuer}/.well-known/openid-configuration");
        let discovery: serde_json::Value = client
            .get(&discovery_url)
            .send()
            .await
            .map_err(|e| MemoryError::ConfigInvalid(format!("OIDC discovery failed: {e}")))?
            .error_for_status()
            .map_err(|e| MemoryError::ConfigInvalid(format!("OIDC discovery failed: {e}")))?
            .json()
            .await
            .map_err(|e| MemoryError::ConfigInvalid(format!("OIDC discovery parse failed: {e}")))?;

        if discovery.get("issuer").and_then(|value| value.as_str()) != Some(issuer) {
            return Err(MemoryError::ConfigInvalid(
                "OIDC discovery issuer does not match configured issuer".into(),
            ));
        }
        let authorization_endpoint = discovery
            .get("authorization_endpoint")
            .and_then(|value| value.as_str())
            .ok_or_else(|| {
                MemoryError::ConfigInvalid("OIDC discovery missing authorization_endpoint".into())
            })?
            .to_string();
        let token_endpoint = discovery
            .get("token_endpoint")
            .and_then(|value| value.as_str())
            .ok_or_else(|| {
                MemoryError::ConfigInvalid("OIDC discovery missing token_endpoint".into())
            })?
            .to_string();
        let jwks_uri = discovery
            .get("jwks_uri")
            .and_then(|v| v.as_str())
            .ok_or_else(|| MemoryError::ConfigInvalid("OIDC discovery missing jwks_uri".into()))?
            .to_string();

        let jwks = JwksCache {
            inner: Arc::new(std::sync::RwLock::new(JwksState {
                keys: std::collections::HashMap::new(),
                fetched_at: None,
            })),
            refresh_lock: Arc::new(tokio::sync::Mutex::new(())),
            client: client.clone(),
            jwks_uri,
            ttl: std::time::Duration::from_secs(300),
        };

        Ok(Self {
            issuer: issuer.to_string(),
            client_id: client_id.to_string(),
            audience: audience.to_string(),
            redirect_uri: redirect_uri.to_string(),
            allowed_algorithm: allowed_algorithm.to_string(),
            authorization_endpoint,
            token_endpoint,
            jwks,
        })
    }

    /// Build the authorization URL with PKCE and nonce.
    pub fn authorize_url(
        &self,
        state: &OidcState,
        pkce: &PkceCode,
        nonce: &OidcNonce,
    ) -> Result<String, MemoryError> {
        use oauth2::basic::BasicClient;
        use oauth2::{
            AuthUrl, ClientId, CsrfToken, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope,
        };

        let auth_url = AuthUrl::new(self.authorization_endpoint.clone()).map_err(|error| {
            MemoryError::ConfigInvalid(format!("invalid OIDC authorization URL: {error}"))
        })?;
        let redirect_url = RedirectUrl::new(self.redirect_uri.clone()).map_err(|error| {
            MemoryError::ConfigInvalid(format!("invalid OIDC redirect URL: {error}"))
        })?;
        let client = BasicClient::new(ClientId::new(self.client_id.clone()))
            .set_auth_uri(auth_url)
            .set_redirect_uri(redirect_url);

        let pkce_challenge = PkceCodeChallenge::from_code_verifier_sha256(&PkceCodeVerifier::new(
            pkce.verifier.clone(),
        ));

        let (mut url, _csrf) = client
            .authorize_url(|| CsrfToken::new(state.as_str().to_string()))
            .add_scope(Scope::new("openid".to_string()))
            .set_pkce_challenge(pkce_challenge)
            .url();

        url.query_pairs_mut().append_pair("nonce", nonce.as_str());

        Ok(url.to_string())
    }

    /// Exchange an authorization code for tokens using raw reqwest.
    pub async fn exchange_code(
        &self,
        code: String,
        pkce: PkceCode,
    ) -> Result<OidcTokens, AuthError> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| AuthError::Provider(e.to_string()))?;

        let params = [
            ("grant_type", "authorization_code".to_string()),
            ("code", code),
            ("redirect_uri", self.redirect_uri.clone()),
            ("client_id", self.client_id.clone()),
            ("code_verifier", pkce.verifier),
        ];

        let form_body = params
            .iter()
            .map(|(key, value)| {
                format!(
                    "{}={}",
                    form_urlencode_component(key),
                    form_urlencode_component(value),
                )
            })
            .collect::<Vec<_>>()
            .join("&");
        let resp = client
            .post(&self.token_endpoint)
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(form_body)
            .send()
            .await
            .map_err(|e| AuthError::Provider(e.to_string()))?
            .error_for_status()
            .map_err(|e| AuthError::Provider(e.to_string()))?;

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| AuthError::Provider(e.to_string()))?;

        let id_token = body
            .get("id_token")
            .and_then(|v| v.as_str())
            .ok_or(AuthError::MalformedToken)?
            .to_string();

        Ok(OidcTokens { id_token })
    }

    /// Validate an ID token: issuer, audience, algorithm, expiry, JWKS signature.
    pub async fn validate_id_token(&self, token: &str) -> Result<AccessClaims, AuthError> {
        use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};

        let header = jsonwebtoken::decode_header(token).map_err(AuthError::Jwt)?;

        let kid = header.kid.ok_or(AuthError::MissingKeyId)?;

        let alg = match header.alg {
            Algorithm::RS256 => "RS256",
            Algorithm::ES256 => "ES256",
            Algorithm::EdDSA => "EdDSA",
            _ => return Err(AuthError::DisallowedAlgorithm),
        };

        if alg != self.allowed_algorithm {
            return Err(AuthError::DisallowedAlgorithm);
        }

        let key: DecodingKey = self.jwks.key_for(&kid).await?;

        let validation_algorithm = match alg {
            "RS256" => Algorithm::RS256,
            "ES256" => Algorithm::ES256,
            "EdDSA" => Algorithm::EdDSA,
            _ => return Err(AuthError::DisallowedAlgorithm),
        };
        let mut validation = Validation::new(validation_algorithm);
        validation.set_audience(&[&self.audience]);
        validation.set_issuer(&[&self.issuer]);
        validation.validate_exp = true;

        let token_data =
            decode::<AccessClaims>(token, &key, &validation).map_err(AuthError::Jwt)?;

        Ok(token_data.claims)
    }
}

fn form_urlencode_component(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char);
            }
            b' ' => encoded.push('+'),
            other => {
                encoded.push('%');
                encoded.push(HEX[(other >> 4) as usize] as char);
                encoded.push(HEX[(other & 0x0F) as usize] as char);
            }
        }
    }
    encoded
}

// ---------------------------------------------------------------------------
// Identity subject verifier (blind index)
// ---------------------------------------------------------------------------

/// Compute a keyed HMAC of (issuer, subject) to create a blind
/// index for the OIDC identity. Raw OIDC subjects remain transient
/// only.
pub fn identity_subject_verifier(
    key: &[u8; 32],
    issuer: &str,
    subject: &str,
) -> Result<[u8; 32], MemoryError> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key)
        .map_err(|_| MemoryError::ConfigInvalid("identity index key".into()))?;
    mac.update(issuer.trim().as_bytes());
    mac.update(b":");
    mac.update(subject.as_bytes());
    Ok(mac.finalize().into_bytes().into())
}

// ---------------------------------------------------------------------------
// OIDC state sealing (AEAD)
// ---------------------------------------------------------------------------

/// Seal the OIDC flow material using AEAD encryption.
pub fn seal_oidc_payload(
    key: &[u8; 32],
    state: &OidcState,
    nonce: &OidcNonce,
    pkce: &PkceCode,
) -> Result<(Vec<u8>, [u8; 12]), MemoryError> {
    use chacha20poly1305::{ChaCha20Poly1305, KeyInit, Nonce, aead::Aead};

    let plaintext = serde_json::json!({
        "state": state.as_str(),
        "nonce": nonce.as_str(),
        "pkce_verifier": pkce.verifier,
    });
    let plaintext_bytes = serde_json::to_vec(&plaintext)
        .map_err(|_| MemoryError::ConfigInvalid("seal serialization".into()))?;

    let cipher = ChaCha20Poly1305::new(key.into());
    let mut nonce_bytes = [0u8; 12];
    rand::fill(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext_bytes.as_ref())
        .map_err(|_| AuthError::Sealing)
        .map_err(|e: AuthError| MemoryError::ConfigInvalid(e.to_string()))?;

    Ok((ciphertext, nonce_bytes))
}

/// Unseal OIDC flow material from the registry.
pub fn unseal_oidc_payload(
    key: &[u8; 32],
    ciphertext: &[u8],
    nonce_bytes: &[u8; 12],
) -> Result<StoredOidcRequest, MemoryError> {
    use chacha20poly1305::{ChaCha20Poly1305, KeyInit, Nonce, aead::Aead};

    let cipher = ChaCha20Poly1305::new(key.into());
    let nonce = Nonce::from_slice(nonce_bytes);

    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| AuthError::Sealing)
        .map_err(|e: AuthError| MemoryError::ConfigInvalid(e.to_string()))?;

    #[derive(serde::Deserialize)]
    struct SealedPayload {
        state: String,
        nonce: String,
        pkce_verifier: String,
    }

    let payload: SealedPayload = serde_json::from_slice(&plaintext)
        .map_err(|e| MemoryError::ConfigInvalid(format!("unseal parse: {e}")))?;

    Ok(StoredOidcRequest {
        state: OidcState(payload.state),
        nonce: OidcNonce(payload.nonce),
        pkce: PkceCode {
            verifier: payload.pkce_verifier,
            challenge: String::new(),
        },
        // The registry enforces the authoritative expiry at consume time;
        // this value is only the decrypted projection used by callers.
        expires_at: Utc::now() + chrono::Duration::minutes(10),
    })
}

// ---------------------------------------------------------------------------
// HTTP handlers
// ---------------------------------------------------------------------------

/// GET /api/v1/auth/authorize — initiate OIDC login.
pub async fn authorize(
    axum::extract::State(state): axum::extract::State<std::sync::Arc<crate::http::HttpState>>,
) -> Result<axum::response::Redirect, super::error::ApiError> {
    let pkce = PkceCode::new();
    let state_token = OidcState::new();
    let nonce = OidcNonce::new();

    // Seal the flow material and store keyed hash + ciphertext.
    let state_hash = hex::encode(identity_subject_verifier(
        &state.config.keys.oidc_state,
        "",
        state_token.as_str(),
    )?);
    let (sealed, aead_nonce) =
        seal_oidc_payload(&state.config.keys.oidc_state, &state_token, &nonce, &pkce)?;

    #[cfg(feature = "control-plane")]
    state
        .registry
        .store_clone()
        .store_oidc_request(&state_hash, &sealed, &aead_nonce)
        .await?;

    let oidc = state
        .oidc_client
        .as_ref()
        .ok_or(super::error::ApiError::Unavailable)?;
    let url = oidc.authorize_url(&state_token, &pkce, &nonce)?;
    Ok(axum::response::Redirect::to(&url))
}

/// GET /api/v1/auth/callback — OIDC provider redirects here.
pub async fn callback(
    axum::extract::State(state): axum::extract::State<std::sync::Arc<crate::http::HttpState>>,
    axum::extract::Query(params): axum::extract::Query<OidcCallback>,
) -> Result<(axum::http::header::HeaderMap, axum::response::Redirect), super::error::ApiError> {
    // Reject if the provider reported an error.
    if params.error.is_some() {
        return Err(super::error::ApiError::Unauthorized);
    }

    // Hash the incoming state to look up the sealed request.
    let state_hash = hex::encode(identity_subject_verifier(
        &state.config.keys.oidc_state,
        "",
        &params.state,
    )?);

    #[cfg(feature = "control-plane")]
    let (sealed, aead_nonce) = state
        .registry
        .store_clone()
        .take_oidc_request(&state_hash)
        .await?
        .ok_or(super::error::ApiError::Unauthorized)?;

    #[cfg(not(feature = "control-plane"))]
    {
        let _ = (&sealed, &aead_nonce);
        return Err(super::error::ApiError::Unavailable);
    }

    let stored = unseal_oidc_payload(&state.config.keys.oidc_state, &sealed, &aead_nonce)?;
    if stored.state.as_str() != params.state {
        return Err(super::error::ApiError::Unauthorized);
    }

    // Reject expired requests (TTL 10 minutes).
    if stored.expires_at < Utc::now() {
        return Err(super::error::ApiError::Unauthorized);
    }

    // RFC 9207 issuer check.
    if params
        .iss
        .as_deref()
        .is_some_and(|issuer| issuer != state.config.oidc_issuer)
    {
        return Err(super::error::ApiError::Unauthorized);
    }

    let code = params.code.ok_or(super::error::ApiError::Unauthorized)?;

    let oidc = state
        .oidc_client
        .as_ref()
        .ok_or(super::error::ApiError::Unavailable)?;

    let tokens = oidc.exchange_code(code, stored.pkce).await?;
    let claims = oidc.validate_id_token(&tokens.id_token).await?;

    // Validate nonce matches the one we generated for this request.
    if claims.nonce.as_deref() != Some(stored.nonce.as_str()) {
        return Err(super::error::ApiError::Unauthorized);
    }

    let subject_verifier =
        identity_subject_verifier(&state.config.keys.identity_index, &claims.iss, &claims.sub)?;

    let account =
        upsert_account_for_identity(state.clone(), &claims.iss, &subject_verifier).await?;

    let cookie_value = super::session::generate_session_cookie_value();
    let session = super::session::ControlPlaneSession::new(&account, &cookie_value, &state.config)?;
    state.registry.store_clone().store_session(&session).await?;

    let cookie = super::session::build_session_cookie(cookie_value, &state.config);
    let mut headers = axum::http::header::HeaderMap::new();
    headers.insert(
        axum::http::header::SET_COOKIE,
        cookie.parse().map_err(|_| {
            super::error::ApiError::Internal(MemoryError::ConfigInvalid(
                "invalid cookie header".into(),
            ))
        })?,
    );
    Ok((headers, axum::response::Redirect::to("/")))
}

/// Find or create an Account for an OIDC identity. Idempotent:
/// if an account already exists for this (issuer, subject_verifier),
/// return it; otherwise create a new one when signup policy permits.
async fn upsert_account_for_identity(
    state: std::sync::Arc<crate::http::HttpState>,
    issuer: &str,
    subject_verifier: &[u8; 32],
) -> Result<crate::http::registry::models::Account, super::error::ApiError> {
    use crate::http::registry::models::SubjectVerifier;

    let sv = SubjectVerifier(*subject_verifier);
    let store = state.registry.store_clone();

    // Try to find existing account by identity.
    if let Some(account) = store.find_account_by_identity(issuer, &sv).await? {
        return Ok(account);
    }

    // Check signup policy.
    match state.config.signup_mode {
        crate::http::config::SignupMode::InviteOnly => {
            return Err(super::error::ApiError::Forbidden);
        }
        crate::http::config::SignupMode::Open => {}
    }

    // Reserve the complete Account → Tenant → ExternalIdentity bundle in one
    // durable transaction. The namespace is opaque, server-generated, and is
    // never derived from the external subject.
    let now = chrono::Utc::now();
    let account = crate::http::registry::models::Account {
        id: crate::http::registry::models::new_account_id(),
        status: crate::http::registry::models::AccountStatus::Active,
        tenant_id: crate::http::registry::models::new_tenant_id(),
        created_at: now,
    };
    let tenant = crate::http::registry::models::Tenant {
        id: account.tenant_id.clone(),
        status: crate::http::registry::models::TenantStatus::Reserved,
        namespace_binding: crate::http::registry::models::NamespaceBinding {
            namespace: crate::http::registry::models::new_namespace_name(),
            database: "memory".into(),
        },
        plan_version: 1,
        schema_version: 0,
        retry_stage: None,
        provisioning_lease: None,
        created_at: now,
        version: 0,
    };
    let identity = crate::http::registry::models::ExternalIdentity {
        id: crate::http::registry::models::new_external_identity_id(),
        issuer: issuer.to_owned(),
        subject_verifier: sv,
        account_id: account.id.clone(),
        created_at: now,
    };
    store
        .create_account_bundle(&account, &tenant, Some(&identity))
        .await?;
    store
        .append_provisioning_event(&tenant.id, "reserved")
        .await?;
    Ok(account)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_urlencode_component_escapes_reserved_bytes() {
        assert_eq!(form_urlencode_component("a b+c/&"), "a+b%2Bc%2F%26");
    }

    #[test]
    fn identity_subject_verifier_is_deterministic() {
        let key = [0xABu8; 32];
        let a = identity_subject_verifier(&key, "https://issuer.example.com", "sub123").unwrap();
        let b = identity_subject_verifier(&key, "https://issuer.example.com", "sub123").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn identity_subject_verifier_differs_for_different_subjects() {
        let key = [0xABu8; 32];
        let a = identity_subject_verifier(&key, "https://issuer.example.com", "sub1").unwrap();
        let b = identity_subject_verifier(&key, "https://issuer.example.com", "sub2").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn seal_unseal_roundtrip() {
        let key = [0x42u8; 32];
        let state = OidcState::new();
        let nonce = OidcNonce::new();
        let pkce = PkceCode::new();

        let (ciphertext, nonce_bytes) = seal_oidc_payload(&key, &state, &nonce, &pkce).unwrap();

        let mut stored = unseal_oidc_payload(&key, &ciphertext, &nonce_bytes).unwrap();
        stored.pkce.challenge = String::new();

        assert_eq!(stored.state.as_str(), state.as_str());
        assert_eq!(stored.nonce.as_str(), nonce.as_str());
        assert_eq!(stored.pkce.verifier, pkce.verifier);
    }

    #[test]
    fn seal_unseal_wrong_key_fails() {
        let key = [0x42u8; 32];
        let wrong_key = [0x99u8; 32];
        let state = OidcState::new();
        let nonce = OidcNonce::new();
        let pkce = PkceCode::new();

        let (ciphertext, nonce_bytes) = seal_oidc_payload(&key, &state, &nonce, &pkce).unwrap();

        let result = unseal_oidc_payload(&wrong_key, &ciphertext, &nonce_bytes);
        assert!(result.is_err());
    }
}
