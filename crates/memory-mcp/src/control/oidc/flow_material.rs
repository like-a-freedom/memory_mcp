//! OIDC flow material types.
//!
//! The OIDC Authorization Code + PKCE flow carries a state token
//! (CSRF protection), a nonce (replay protection), and a PKCE
//! code-verifier/challenge pair through the redirect chain. All
//! three are kept transient — only their keyed hashes live in
//! the durable registry.
//!
//! `AuthError` covers every recoverable failure that the OIDC
//! client and its callers surface to the rest of the crate.

use serde::Deserialize;

/// Random state token for CSRF protection in the OIDC flow.
#[derive(Debug, Clone)]
pub struct OidcState(pub(crate) String);

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
pub struct OidcNonce(pub(crate) String);

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
        use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
        use sha2::{Digest, Sha256};

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
    /// Authoritative expiry enforced by the registry at consume
    /// time; this value is only the decrypted projection used by
    /// callers.
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

/// OIDC tokens — only the ID token is retained.
#[derive(Debug, Clone)]
pub struct OidcTokens {
    pub id_token: String,
}

/// Callback query parameters from the OIDC provider.
#[derive(Debug, Clone, Deserialize)]
pub struct OidcCallback {
    pub state: String,
    pub code: Option<String>,
    pub error: Option<String>,
    /// Optional RFC 9207 issuer check.
    #[serde(default)]
    pub iss: Option<String>,
}

/// Errors during OIDC auth.
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

impl From<AuthError> for crate::error::MemoryError {
    fn from(err: AuthError) -> Self {
        crate::error::MemoryError::ConfigInvalid(err.to_string())
    }
}

/// Decoded ID-token claims.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Audience {
    One(String),
    Many(Vec<String>),
}

#[derive(Debug, Deserialize)]
pub struct AccessClaims {
    pub iss: String,
    pub sub: String,
    pub aud: Audience,
    pub exp: u64,
    pub nonce: Option<String>,
}
