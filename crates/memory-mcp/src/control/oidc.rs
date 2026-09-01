//! OIDC Authorization Code + PKCE (Task 10.1, spec §5.3).
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

/// PKCE code verifier + S256 challenge.
#[derive(Debug, Clone)]
pub struct PkceCode {
    pub verifier: String,
    pub challenge: String,
}

impl PkceCode {
    pub fn new() -> Self {
        use rand::Fill;
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
pub struct AccessClaims {
    pub iss: String,
    pub sub: String,
    pub aud: String,
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
        if fresh {
            if let Some(key) = self.find_key(kid)? {
                return Ok(key);
            }
        }
        let _refresh = self.refresh_lock.lock().await;
        let fresh = self
            .inner
            .read()
            .map_err(|_| AuthError::Jwks("JWKS cache lock poisoned".into()))?
            .fetched_at
            .is_some_and(|at| at.elapsed() < self.ttl);
        if fresh {
            if let Some(key) = self.find_key(kid)? {
                return Ok(key);
            }
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
            jwks,
        })
    }

    /// Build the authorization URL with PKCE and nonce.
    pub fn authorize_url(&self, state: &OidcState, pkce: &PkceCode, nonce: &OidcNonce) -> String {
        use oauth2::basic::BasicClient;
        use oauth2::{
            AuthUrl, ClientId, CsrfToken, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope,
        };

        let client = BasicClient::new(ClientId::new(self.client_id.clone()))
            .set_auth_uri(
                AuthUrl::new(format!("{}/authorize", self.issuer)).expect("valid auth URL"),
            )
            .set_redirect_uri(
                RedirectUrl::new(self.redirect_uri.clone()).expect("valid redirect URL"),
            );

        let pkce_challenge = PkceCodeChallenge::from_code_verifier_sha256(&PkceCodeVerifier::new(
            pkce.verifier.clone(),
        ));

        let (mut url, _csrf) = client
            .authorize_url(|| CsrfToken::new(state.as_str().to_string()))
            .add_scope(Scope::new("openid".to_string()))
            .set_pkce_challenge(pkce_challenge)
            .url();

        url.query_pairs_mut().append_pair("nonce", nonce.as_str());

        url.to_string()
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

        let params = serde_json::json!({
            "grant_type": "authorization_code",
            "code": code,
            "redirect_uri": self.redirect_uri,
            "client_id": self.client_id,
            "code_verifier": pkce.verifier,
        });

        let resp = client
            .post(format!("{}/oauth/token", self.issuer))
            .json(&params)
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

        let mut validation = Validation::new(match alg {
            "RS256" => Algorithm::RS256,
            "ES256" => Algorithm::ES256,
            "EdDSA" => Algorithm::EdDSA,
            _ => unreachable!(),
        });
        validation.set_audience(&[&self.audience]);
        validation.set_issuer(&[&self.issuer]);
        validation.validate_exp = true;

        let token_data =
            decode::<AccessClaims>(token, &key, &validation).map_err(AuthError::Jwt)?;

        Ok(token_data.claims)
    }
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
    use rand::Fill;
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
        expires_at: Utc::now() + chrono::Duration::minutes(10),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
