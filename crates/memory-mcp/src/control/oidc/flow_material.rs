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
#[derive(Debug, Clone)]
pub struct StoredOidcRequest {
    pub state: OidcState,
    pub nonce: OidcNonce,
    pub pkce: PkceCode,
    /// What the flow is for. A payload sealed before this field existed
    /// decodes as [`OidcFlowIntent::SignIn`], so an upgrade does not strand an
    /// in-flight login.
    pub intent: OidcFlowIntent,
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
    fn a_new_state_is_hex_encoded_32_bytes() {
        let observed = OidcState::new().as_str().to_string();

        assert_eq!(observed.len(), 64, "32 bytes render as 64 hex characters");
    }

    #[test]
    fn two_states_never_collide() {
        let observed = OidcState::new().as_str() == OidcState::new().as_str();

        assert!(!observed, "CSRF state must be unpredictable");
    }

    #[test]
    fn a_default_state_is_fresh() {
        let observed = OidcState::default().as_str().len();

        assert_eq!(observed, 64);
    }

    #[test]
    fn a_new_nonce_is_hex_encoded_32_bytes() {
        let observed = OidcNonce::new().as_str().to_string();

        assert_eq!(observed.len(), 64);
    }

    #[test]
    fn two_nonces_never_collide() {
        let observed = OidcNonce::new().as_str() == OidcNonce::new().as_str();

        assert!(!observed, "a replayed nonce must not be accepted");
    }

    #[test]
    fn a_default_nonce_is_fresh() {
        let observed = OidcNonce::default().as_str().len();

        assert_eq!(observed, 64);
    }

    #[test]
    fn a_pkce_verifier_is_43_base64url_characters() {
        // 32 random bytes in unpadded base64url.
        let observed = PkceCode::new().verifier;

        assert_eq!(observed.len(), 43);
    }

    #[test]
    fn a_pkce_verifier_carries_no_padding() {
        let observed = PkceCode::new().verifier;

        assert!(!observed.contains('='), "PKCE uses unpadded base64url");
    }

    #[test]
    fn a_pkce_challenge_is_the_sha256_of_the_verifier() {
        use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
        use sha2::{Digest, Sha256};

        let code = PkceCode::new();
        let expected = URL_SAFE_NO_PAD.encode(Sha256::digest(code.verifier.as_bytes()));

        assert_eq!(code.challenge, expected);
    }

    #[test]
    fn two_pkce_codes_never_share_a_verifier() {
        let observed = PkceCode::new().verifier == PkceCode::new().verifier;

        assert!(!observed, "the verifier is the flow's secret");
    }

    #[test]
    fn a_default_pkce_code_is_fresh() {
        let observed = PkceCode::default().verifier.len();

        assert_eq!(observed, 43);
    }

    #[test]
    fn the_default_flow_intent_is_sign_in() {
        assert_eq!(OidcFlowIntent::default(), OidcFlowIntent::SignIn);
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
