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

/// The ID-token signing algorithms this build accepts — asymmetric only, so a
/// provider can never pick the HMAC path and reach the public key as a secret.
///
/// This is the single source of truth for that decision. `client::resolve_allowed_algorithms`
/// intersects it with what the provider advertises, the JWKS cache filters keys
/// through it, and the HTTP config validator accepts a pin only from it. The
/// three used to be written out separately and had already drifted apart: the
/// cache accepted `{RS256, ES256, EdDSA}` while the resolver and the validator
/// also allowed RS384/RS512, so a legitimately pinned provider had its signing
/// key silently dropped and login failed as 503. Adding an algorithm here is
/// the only edit needed to make the whole path accept it — and the JWKS curves
/// below are the second half of that, so EC entries stay limited to P-256
/// because no other curve is decoded.
pub(crate) const SUPPORTED_ID_TOKEN_ALGORITHMS: [&str; 5] =
    ["RS256", "RS384", "RS512", "ES256", "EdDSA"];

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

/// What a sealed OIDC flow is for (ADR-0057).
///
/// The intent travels inside the AEAD-sealed state payload, so it is bound to
/// the flow by the same key that protects the nonce and the PKCE verifier, and
/// the callback can never be talked into attaching an identity by a request
/// body.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum OidcFlowIntent {
    /// Resolve or create the Account behind the verified identity, and open a
    /// browser session for it.
    #[default]
    SignIn,
    /// Attach the provider-verified identity to this Account. The Account is
    /// named here because the flow began inside that Account's session, not
    /// because a client asserted it at the callback.
    Link { account_id: String },
    /// Attach the provider-verified identity to this Account on the
    /// administrator's invitation (ADR-0057). The Account and the inviting
    /// administrator travel inside the sealed payload; `replace` swaps the
    /// Account's single mis-bound identity instead of adding beside it.
    Invite {
        account_id: String,
        invited_by: String,
        replace: bool,
    },
}

/// Stored OIDC request — decrypted projection from the registry.
///
/// Carries no expiry: the registry rejects a consumed request under its own
/// `expires_at` in the same transaction as the read, so nothing re-derives a
/// deadline here.
#[derive(Debug, Clone)]
pub struct StoredOidcRequest {
    pub state: OidcState,
    pub nonce: OidcNonce,
    pub pkce: PkceCode,
    /// What the flow is for. A payload sealed before this field existed
    /// decodes as [`OidcFlowIntent::SignIn`], so an upgrade does not strand an
    /// in-flight login.
    pub intent: OidcFlowIntent,
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
    /// The provider signed with an algorithm this deployment does not accept.
    ///
    /// Carries the algorithm and key id the token actually arrived with. Both
    /// are public header values, and naming them is the whole difference
    /// between a refusal an operator can act on — "our JWKS has this kid, the
    /// token claims this alg" — and one that has to be guessed at from a
    /// configuration file.
    #[error("token algorithm is not allowed: alg={alg} kid={kid}")]
    DisallowedAlgorithm { alg: String, kid: String },
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Decode a single-string `aud` claim.
    fn one_audience(json: &str) -> Audience {
        serde_json::from_str::<AccessClaims>(json)
            .expect("claims decode")
            .aud
    }

    /// Decode a list `aud` claim.
    fn many_audience(json: &str) -> Audience {
        serde_json::from_str::<AccessClaims>(json)
            .expect("claims decode")
            .aud
    }

    #[test]
    fn a_single_string_audience_decodes_to_one() {
        let observed = one_audience(r#"{"iss":"i","sub":"s","aud":"a","exp":1}"#);

        assert!(matches!(observed, Audience::One(ref value) if value == "a"));
    }

    #[test]
    fn a_list_audience_decodes_to_many() {
        let observed = many_audience(r#"{"iss":"i","sub":"s","aud":["a","b"],"exp":1}"#);

        assert!(matches!(observed, Audience::Many(ref values) if values == &["a", "b"]));
    }

    #[test]
    fn a_claims_payload_without_a_nonce_decodes() {
        let observed =
            serde_json::from_str::<AccessClaims>(r#"{"iss":"i","sub":"s","aud":"a","exp":1}"#)
                .expect("claims decode");

        assert!(observed.nonce.is_none());
    }

    #[test]
    fn a_claims_payload_carries_its_nonce() {
        let observed = serde_json::from_str::<AccessClaims>(
            r#"{"iss":"i","sub":"s","aud":"a","exp":1,"nonce":"n1"}"#,
        )
        .expect("claims decode");

        assert_eq!(observed.nonce.as_deref(), Some("n1"));
    }

    #[test]
    fn an_auth_error_becomes_a_config_invalid_memory_error() {
        let observed = crate::error::MemoryError::from(AuthError::Sealing);

        assert!(
            observed.to_string().contains("could not be sealed"),
            "the operator-facing message must survive the conversion"
        );
    }
}
